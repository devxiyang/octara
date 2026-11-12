//! Pointer input — mouse, touch and pen — as raw events from a shell, a test or an agent, and
//! the recognizers that turn them into presses, double presses, context menu requests, long
//! presses, drags and touch scrolling (with momentum).
//!
//! One target per pointer (Rune): the topmost element under it that handles the pointer. From
//! the press on, that pointer belongs to the target until it lifts or is cancelled.

use super::behavior::{Behavior, behavior};
use super::event::InputEvent;
use super::{Handler, Modality, Window};
use crate::kernel::{HandlerKind, Kernel, NodeId, State};
use bitflags::bitflags;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerKind {
    #[default]
    Mouse,
    Touch,
    Pen,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Down,
    Move,
    Up,
    /// The system took the pointer (a touch became a system gesture, the window lost focus).
    Cancel,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    #[default]
    Primary,
    Secondary,
    Middle,
}

bitflags! {
    /// Modifier keys held during an input.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, serde::Serialize, serde::Deserialize)]
    pub struct Modifiers: u8 {
        const SHIFT = 1;
        const CTRL = 2;
        const ALT = 4;
        /// Command on Apple platforms, the Windows key elsewhere.
        const META = 8;
    }
}

/// One raw pointer event, in logical pixels from the window's top-left corner.
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct PointerEvent {
    /// Tells pointers apart: one per finger; the mouse is 0.
    pub id: u64,
    pub kind: PointerKind,
    pub phase: Phase,
    pub x: f32,
    pub y: f32,
    pub button: Button,
    pub modifiers: Modifiers,
}

impl PointerEvent {
    pub fn mouse(phase: Phase, x: f32, y: f32) -> Self {
        PointerEvent { id: 0, kind: PointerKind::Mouse, phase, x, y, button: Button::Primary, modifiers: Modifiers::empty() }
    }
    pub fn touch(id: u64, phase: Phase, x: f32, y: f32) -> Self {
        PointerEvent { id, kind: PointerKind::Touch, phase, x, y, button: Button::Primary, modifiers: Modifiers::empty() }
    }
    pub fn button(self, button: Button) -> Self {
        PointerEvent { button, ..self }
    }
    pub fn modifiers(self, modifiers: Modifiers) -> Self {
        PointerEvent { modifiers, ..self }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DragPhase {
    Start,
    Move,
    End,
    Cancel,
}

/// A drag on an element (see `on_drag`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Drag {
    pub phase: DragPhase,
    /// The pointer in the element's own coordinates.
    pub x: f32,
    pub y: f32,
    /// Travel since the press, in window coordinates.
    pub dx: f32,
    pub dy: f32,
}

/// Thresholds and timings of the recognizers (GPUI's and Rune's defaults).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Tuning {
    /// Mouse and pen movement before a press becomes a drag (Rune: 4 pt).
    pub mouse_slop: f32,
    /// Touch movement before a tap becomes a drag or a scroll (8 pt).
    pub touch_slop: f32,
    /// The longest gap between the clicks of a double click (500 ms; shells pass the system's).
    pub double_click: Duration,
    /// The longest gap between the taps of a double tap (400 ms).
    pub double_tap: Duration,
    /// How far the second tap may land from the first (16 pt; clicks: the mouse slop).
    pub double_tap_slop: f32,
    /// How long a touch stays still to be a long press (500 ms).
    pub long_press: Duration,
    /// Momentum after a touch scroll: the velocity kept per millisecond (iOS: 0.998).
    pub momentum_decay: f32,
    /// Momentum stops below this speed, and does not start below it (points per second).
    pub momentum_stop: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning {
            mouse_slop: 4.0,
            touch_slop: 8.0,
            double_click: Duration::from_millis(500),
            double_tap: Duration::from_millis(400),
            double_tap_slop: 16.0,
            long_press: Duration::from_millis(500),
            momentum_decay: 0.998,
            momentum_stop: 10.0,
        }
    }
}

/// How long a frame of momentum is, for shells waiting on [`App::next_deadline`].
const FRAME_MS: f64 = 1000.0 / 60.0;
/// Touch samples older than this do not count toward a release's velocity.
const VELOCITY_WINDOW_MS: f64 = 100.0;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    /// Down, not yet decided: a press if it lifts inside.
    Pending,
    Dragging,
    /// A touch scrolling this container.
    Panning(NodeId),
    /// Lifting presses nothing (scrolled, long-pressed, stopped momentum, cancelled).
    Spent,
}

/// The pointer that is down, and what it is doing.
#[derive(Clone, Debug)]
pub(crate) struct Held {
    pointer: u64,
    kind: PointerKind,
    button: Button,
    /// The topmost node under the press.
    hit: Option<NodeId>,
    /// The element the press acts on.
    target: Option<NodeId>,
    start: (f32, f32),
    at: (f32, f32),
    /// Clock time of the press (ms).
    down_at: f64,
    inside: bool,
    mode: Mode,
    /// Which press of a run on the target this is (1, 2 for a double press, …).
    clicks: u32,
    /// Recent touch positions `(time, x, y)` for the release velocity.
    samples: Vec<(f64, f32, f32)>,
}

/// The last press, for counting double presses.
#[derive(Clone, Copy, Debug)]
struct LastPress {
    node: NodeId,
    at: f64,
    pos: (f32, f32),
    count: u32,
}

/// A scroll container gliding after a touch scroll: offset `from + dir · v₀(dᵗ − 1)/ln d`.
#[derive(Clone, Copy, Debug)]
struct Momentum {
    node: NodeId,
    start: f64,
    from: (f32, f32),
    /// Unit direction of the scroll offset's change.
    dir: (f32, f32),
    /// Initial speed (points per ms).
    speed: f32,
}

#[derive(Default)]
pub(crate) struct PointerState {
    pub held: Option<Held>,
    last_press: Option<LastPress>,
    momentum: Option<Momentum>,
    /// Where the mouse (or pen) is, for hover after layout changes.
    pub hover_at: Option<(f32, f32)>,
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

impl Kernel {
    /// Whether a pointer press acts on `id`: it has a pointer handler or takes focus.
    fn takes_pointer(&self, id: NodeId) -> bool {
        HandlerKind::POINTER.iter().any(|h| self.handler(id, *h).is_some()) || self.is_focusable(id)
    }
    /// The element a pointer at the hit node acts on: the hit node or its nearest ancestor that
    /// takes the pointer.
    pub(crate) fn pointer_target(&self, hit: Option<NodeId>) -> Option<NodeId> {
        self.up_from(hit, |id| self.takes_pointer(id))
    }
    /// What a `button` press at the hit node acts on: the nearest context menu for the
    /// secondary button, else the pointer target.
    fn press_target(&self, hit: Option<NodeId>, button: Button) -> Option<NodeId> {
        match button {
            Button::Secondary => self.handler_above(hit, HandlerKind::SecondaryPress),
            _ => self.pointer_target(hit),
        }
    }
    /// The nearest node from `from` up with a `kind` handler.
    pub(crate) fn handler_above(&self, from: Option<NodeId>, kind: HandlerKind) -> Option<NodeId> {
        self.up_from(from, |id| self.handler(id, kind).is_some())
    }
    fn up_from(&self, mut n: Option<NodeId>, ok: impl Fn(NodeId) -> bool) -> Option<NodeId> {
        while let Some(id) = n {
            if ok(id) {
                return Some(id);
            }
            n = self.parent(id);
        }
        None
    }
    /// The nearest scroll container from `from` up that scrolls along `(dx, dy)`.
    pub(crate) fn scroller_above(&self, from: Option<NodeId>, dx: f32, dy: f32) -> Option<NodeId> {
        self.up_from(from, |id| {
            let (sx, sy) = self.scrollable(id);
            (sx && dx != 0.0) || (sy && dy != 0.0)
        })
    }
}

impl Window {
    /// The topmost node under `(x, y)` that input can reach (not behind an open modal).
    fn hit_at(&self, x: f32, y: f32) -> Option<NodeId> {
        self.live(self.w.kernel.borrow().hit_test(x, y))
    }

    /// The behavior of `n`'s element kind, if it has any.
    fn behavior_of(&self, n: Option<NodeId>) -> Option<(NodeId, &'static dyn Behavior)> {
        let n = n?;
        let kind = self.w.kernel.borrow().kind(n);
        behavior(kind).map(|b| (n, b))
    }
    /// The target's own drag behavior for this pointer, if it has one.
    fn own_drag(&self, h: &Held) -> Option<(NodeId, &'static dyn Behavior)> {
        self.behavior_of(h.target).filter(|(_, b)| b.drags(h.kind))
    }

    pub(super) fn now_ms(&self) -> f64 {
        self.app.sched.now_ms()
    }

    /// One raw pointer event, as a window delivers it.
    pub(super) fn on_pointer(&mut self, ev: PointerEvent) {
        self.input_frame(|| match ev.phase {
            Phase::Down => self.down(ev),
            Phase::Move => self.moved(ev),
            Phase::Up => self.up(ev),
            Phase::Cancel => self.cancel(ev.id),
        });
    }

    /// The mouse moved to `(x, y)` with no button down (or with one down: the press follows).
    pub fn pointer_move(&mut self, x: f32, y: f32) {
        self.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Move, x, y)));
    }
    /// The mouse left the window: nothing is hovered.
    pub(super) fn on_pointer_leave(&mut self) {
        self.input_frame(|| {
            self.w.interaction.borrow_mut().pointer.hover_at = None;
            self.set_hover(None);
        });
    }
    /// The primary mouse button went down at `(x, y)`.
    pub fn pointer_down(&mut self, x: f32, y: f32) {
        self.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Down, x, y)));
    }
    /// The primary mouse button came up at `(x, y)`.
    pub fn pointer_up(&mut self, x: f32, y: f32) {
        self.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Up, x, y)));
    }
    /// A primary click at `(x, y)`: down and up.
    pub fn click_at(&mut self, x: f32, y: f32) {
        self.pointer_down(x, y);
        self.pointer_up(x, y);
    }

    /// A wheel or trackpad scroll of `(dx, dy)` at `(x, y)`: the innermost scroll container
    /// there that scrolls on that axis takes it (no hand-off to outer ones at the edge, as in
    /// Rune). A press being held is cancelled, unless it is dragging.
    pub fn wheel(&mut self, x: f32, y: f32, dx: f32, dy: f32) {
        self.event(InputEvent::Wheel { x, y, dx, dy });
    }
    pub(super) fn on_wheel(&mut self, x: f32, y: f32, dx: f32, dy: f32) {
        self.input_frame(|| {
            self.w.interaction.borrow_mut().pointer.momentum = None;
            let target = {
                let k = self.w.kernel.borrow();
                k.scroller_above(self.hit_at(x, y), dx, dy)
            };
            if let Some(n) = target {
                self.scroll_by(n, dx, dy);
                self.scrolled_under_press();
            }
        });
    }

    /// The recognizers' timing (thresholds, double press, long press, momentum).
    pub fn tuning(&self) -> Tuning {
        self.app.tuning.get()
    }
    pub fn set_tuning(&mut self, tuning: Tuning) {
        self.app.tuning.set(tuning);
    }

    /// The pointer's next deadline (ms): a long press coming due, or momentum's next frame.
    pub(super) fn pointer_deadline(&self) -> Option<f64> {
        if self.w.interaction.borrow().pointer.momentum.is_some() {
            return Some(self.now_ms() + FRAME_MS);
        }
        self.long_press_due()
    }

    /// Whether momentum is gliding (it moves with every clock step).
    pub(super) fn gliding(&self) -> bool {
        self.w.interaction.borrow().pointer.momentum.is_some()
    }

    /// The clock moved: a long press now due fires, momentum glides to the current time.
    pub(super) fn pointer_time(&self) {
        if self.long_press_due().is_some_and(|due| due <= self.now_ms()) {
            self.long_press_fire();
        }
        if self.w.interaction.borrow().pointer.momentum.is_some() {
            self.glide();
        }
    }

    pub(super) fn long_press_due(&self) -> Option<f64> {
        let i = self.w.interaction.borrow();
        let h = i.pointer.held.as_ref()?;
        (h.kind == PointerKind::Touch && h.mode == Mode::Pending && h.button == Button::Primary)
            .then(|| h.down_at + self.app.tuning.get().long_press.as_secs_f64() * 1000.0)
    }

    fn down(&self, ev: PointerEvent) {
        let stopped = {
            let mut i = self.w.interaction.borrow_mut();
            if i.pointer.held.is_some() {
                // One pointer at a time: other fingers (and other buttons) are ignored.
                return;
            }
            // A touch on gliding content stops it, and presses nothing (as on iOS).
            i.pointer.momentum.take().is_some()
        };
        self.set_modality(Modality::Pointer);
        // Outside an open modal a press reaches nothing (and may dismiss the modal).
        let raw = self.w.kernel.borrow().hit_test(ev.x, ev.y);
        let outside = self.pressed_outside(raw);
        let (hit, target, secondary) = {
            let k = self.w.kernel.borrow();
            let hit = if outside { None } else { raw };
            let secondary = ev.button == Button::Secondary
                || (ev.button == Button::Primary
                    && self.app.platform.get().is_apple()
                    && ev.modifiers.contains(Modifiers::CTRL)
                    && ev.kind == PointerKind::Mouse);
            let target = k.press_target(hit, if secondary { Button::Secondary } else { ev.button });
            (hit, target, secondary)
        };
        if ev.kind == PointerKind::Touch {
            self.w.interaction.borrow_mut().pointer.hover_at = None;
            self.set_hover(None);
        }
        let button = if secondary { Button::Secondary } else { ev.button };
        if button == Button::Primary && !outside {
            // A press focuses what it lands on if that takes focus; anything else drops focus.
            let focus = target.filter(|n| self.w.kernel.borrow().is_focusable(*n));
            let _ = self.set_focus(focus);
        }
        let enabled = target.is_some_and(|n| !self.w.kernel.borrow().state(n).contains(State::DISABLED));
        let mode = if stopped || !enabled { Mode::Spent } else { Mode::Pending };
        // The run of presses (double click) counts from the press (AppKit, Rune).
        let clicks = match target {
            Some(n) if mode == Mode::Pending && button == Button::Primary => self.count_press(n, ev.kind, (ev.x, ev.y)),
            _ => 1,
        };
        if mode == Mode::Pending
            && button == Button::Primary
            && let Some(n) = target
        {
            self.w.kernel.borrow_mut().set_state(n, State::PRESSED, true);
        }
        let now = self.now_ms();
        self.w.interaction.borrow_mut().pointer.held = Some(Held {
            pointer: ev.id,
            kind: ev.kind,
            button,
            hit,
            target,
            start: (ev.x, ev.y),
            at: (ev.x, ev.y),
            down_at: now,
            inside: true,
            mode,
            clicks,
            samples: vec![(now, ev.x, ev.y)],
        });
        // An element kind with behavior of its own acts on the press (a field's caret).
        if mode == Mode::Pending
            && button == Button::Primary
            && let Some((n, b)) = self.behavior_of(target)
        {
            b.down(self, n, &ev, clicks);
        }
    }

    fn moved(&self, ev: PointerEvent) {
        if ev.kind != PointerKind::Touch {
            self.w.interaction.borrow_mut().pointer.hover_at = Some((ev.x, ev.y));
            let hit = self.hit_at(ev.x, ev.y);
            self.w.interaction.borrow_mut().hover_held = false;
            self.set_hover(hit);
        }
        let Some(mut h) = self.w.interaction.borrow().pointer.held.clone().filter(|h| h.pointer == ev.id) else { return };
        let prev = h.at;
        h.at = (ev.x, ev.y);
        let now = self.now_ms();
        h.samples.retain(|s| now - s.0 <= VELOCITY_WINDOW_MS);
        h.samples.push((now, ev.x, ev.y));
        let tuning = self.app.tuning.get();
        let slop = if h.kind == PointerKind::Touch { tuning.touch_slop } else { tuning.mouse_slop };
        if h.mode == Mode::Pending && distance(h.start, h.at) > slop {
            // The element's own drag behavior, else its `on_drag`, takes the drag.
            let own = self.own_drag(&h);
            let drags = h.button == Button::Primary
                && (own.is_some() || h.target.is_some_and(|n| self.w.kernel.borrow().handler(n, HandlerKind::Drag).is_some()));
            if drags {
                h.mode = Mode::Dragging;
                // A drag ends a run of clicks.
                self.w.interaction.borrow_mut().pointer.last_press = None;
                match own {
                    Some((n, b)) => b.drag(self, n, h.at),
                    None => self.send_drag(&h, DragPhase::Start),
                }
                self.w.interaction.borrow_mut().pointer.held = Some(h);
                return;
            } else if h.kind == PointerKind::Touch {
                let (dx, dy) = (h.start.0 - h.at.0, h.start.1 - h.at.1);
                // The finger's main direction picks the container (a carousel inside a list).
                let (ax, ay) = if dx.abs() > dy.abs() { (dx, 0.0) } else { (0.0, dy) };
                let scroller = {
                    let k = self.w.kernel.borrow();
                    k.scroller_above(h.hit, ax, ay).or_else(|| k.scroller_above(h.hit, dx, dy))
                };
                if let Some(c) = scroller {
                    h.mode = Mode::Panning(c);
                    if let Some(n) = h.target {
                        self.w.kernel.borrow_mut().set_state(n, State::PRESSED, false);
                    }
                    // The pan starts from the press, so the slop is not lost.
                    self.scroll_by(c, dx, dy);
                    self.w.interaction.borrow_mut().pointer.held = Some(h);
                    return;
                }
            }
        }
        match h.mode {
            Mode::Pending => {
                // Leaving the element un-presses it; coming back presses it again (AppKit).
                let inside = {
                    let k = self.w.kernel.borrow();
                    h.target.is_some() && k.press_target(self.hit_at(ev.x, ev.y), h.button) == h.target
                };
                if inside != h.inside {
                    h.inside = inside;
                    if let Some(n) = h.target
                        && h.button == Button::Primary
                    {
                        self.w.kernel.borrow_mut().set_state(n, State::PRESSED, inside);
                    }
                }
            }
            Mode::Dragging => match self.own_drag(&h) {
                Some((n, b)) => b.drag(self, n, h.at),
                None => self.send_drag(&h, DragPhase::Move),
            },
            Mode::Panning(c) => self.scroll_by(c, prev.0 - h.at.0, prev.1 - h.at.1),
            Mode::Spent => {}
        }
        self.w.interaction.borrow_mut().pointer.held = Some(h);
    }

    fn up(&self, ev: PointerEvent) {
        let held = {
            let mut i = self.w.interaction.borrow_mut();
            if i.pointer.held.as_ref().is_none_or(|h| h.pointer != ev.id) {
                return;
            }
            i.pointer.held.take()
        };
        let Some(mut h) = held else { return };
        h.at = (ev.x, ev.y);
        self.unpress(&h);
        match h.mode {
            Mode::Pending => {
                let inside = {
                    let k = self.w.kernel.borrow();
                    k.press_target(self.hit_at(ev.x, ev.y), h.button) == h.target
                };
                let Some(n) = h.target.filter(|_| inside) else { return };
                if h.button == Button::Secondary {
                    self.run(n, &[HandlerKind::SecondaryPress], None);
                } else if h.button == Button::Primary {
                    self.run(n, &[HandlerKind::Press, HandlerKind::Toggle], None);
                    if h.clicks == 2 {
                        self.run(n, &[HandlerKind::DoublePress], None);
                    }
                }
            }
            Mode::Dragging => match self.own_drag(&h) {
                Some((n, b)) => b.drag(self, n, h.at),
                None => self.send_drag(&h, DragPhase::End),
            },
            Mode::Panning(c) => self.start_momentum(c, &h),
            Mode::Spent => {}
        }
    }

    fn cancel(&self, pointer: u64) {
        let held = {
            let mut i = self.w.interaction.borrow_mut();
            if i.pointer.held.as_ref().is_none_or(|h| h.pointer != pointer) {
                return;
            }
            i.pointer.held.take()
        };
        let Some(h) = held else { return };
        self.unpress(&h);
        if h.mode == Mode::Dragging && self.own_drag(&h).is_none() {
            self.send_drag(&h, DragPhase::Cancel);
        }
    }

    fn unpress(&self, h: &Held) {
        if let Some(n) = h.target {
            let mut k = self.w.kernel.borrow_mut();
            if k.is_alive(n) {
                k.set_state(n, State::PRESSED, false);
            }
        }
    }

    /// Which press of a run a press on `n` at `pos` is (1, then 2 for a double press, …): within
    /// the double-press interval and distance of the previous press on the same element.
    fn count_press(&self, n: NodeId, kind: PointerKind, pos: (f32, f32)) -> u32 {
        let tuning = self.app.tuning.get();
        let (gap, slop) = match kind {
            PointerKind::Touch => (tuning.double_tap, tuning.double_tap_slop),
            _ => (tuning.double_click, tuning.mouse_slop),
        };
        let now = self.now_ms();
        let mut i = self.w.interaction.borrow_mut();
        let count = match i.pointer.last_press {
            Some(l) if l.node == n && now - l.at <= gap.as_secs_f64() * 1000.0 && distance(l.pos, pos) <= slop => l.count + 1,
            _ => 1,
        };
        i.pointer.last_press = Some(LastPress { node: n, at: now, pos, count });
        count
    }

    fn send_drag(&self, h: &Held, phase: DragPhase) {
        let Some(n) = h.target else { return };
        let handler = {
            let k = self.w.kernel.borrow();
            if !k.is_alive(n) {
                return;
            }
            k.handler(n, HandlerKind::Drag).and_then(|id| self.w.handlers.borrow().get(&id.0).cloned())
        };
        let Some(Handler::Drag(f)) = handler else { return };
        // A drag is direct manipulation: what it moves follows the pointer at once.
        if matches!(phase, DragPhase::Start | DragPhase::Move) {
            self.w.kernel.borrow_mut().set_instant(true);
        }
        let (x, y, _, _) = self.w.kernel.borrow().rect(n);
        let d = Drag { phase, x: h.at.0 - x, y: h.at.1 - y, dx: h.at.0 - h.start.0, dy: h.at.1 - h.start.1 };
        self.app.rt.with_frame(crate::reactive::graph::Ctx::Handler, None, Some(None), || f(&d));
    }

    fn scroll_by(&self, n: NodeId, dx: f32, dy: f32) {
        let mut k = self.w.kernel.borrow_mut();
        // The user's own scrolling takes over from a glide.
        k.stop_glide(n);
        let (sx, sy) = k.scrollable(n);
        let (ox, oy) = k.scroll_offset(n);
        k.scroll_to(n, ox + if sx { dx } else { 0.0 }, oy + if sy { dy } else { 0.0 });
    }

    /// Content moved under a held press: the press is cancelled (as in UIScrollView), unless it
    /// is a drag.
    fn scrolled_under_press(&self) {
        let mut i = self.w.interaction.borrow_mut();
        if let Some(h) = i.pointer.held.as_mut()
            && h.mode == Mode::Pending
        {
            h.mode = Mode::Spent;
            let h = h.clone();
            drop(i);
            self.unpress(&h);
        }
    }

    fn long_press_fire(&self) {
        let Some(h) = self.w.interaction.borrow().pointer.held.clone() else { return };
        let found = {
            let k = self.w.kernel.borrow();
            k.handler_above(h.target, HandlerKind::LongPress)
                .map(|n| (n, HandlerKind::LongPress))
                .or_else(|| k.handler_above(h.hit, HandlerKind::SecondaryPress).map(|n| (n, HandlerKind::SecondaryPress)))
        };
        if let Some(held) = self.w.interaction.borrow_mut().pointer.held.as_mut() {
            // Without a handler the touch may still become a tap or a scroll; a long press is
            // only due once either way.
            held.down_at = f64::INFINITY;
            if found.is_some() {
                held.mode = Mode::Spent;
            }
        }
        if let Some((n, kind)) = found {
            self.unpress(&h);
            self.run(n, &[kind], None);
        }
    }

    fn start_momentum(&self, node: NodeId, h: &Held) {
        let now = self.now_ms();
        let samples: Vec<_> = h.samples.iter().filter(|s| now - s.0 <= VELOCITY_WINDOW_MS).collect();
        let (Some(first), Some(last)) = (samples.first(), samples.last()) else { return };
        let dt = last.0 - first.0;
        if dt <= 0.0 {
            return;
        }
        // The content follows the finger: the offset changes against the finger's motion.
        let (vx, vy) = ((first.1 - last.1) / dt as f32, (first.2 - last.2) / dt as f32);
        let speed = (vx * vx + vy * vy).sqrt();
        if speed * 1000.0 < self.app.tuning.get().momentum_stop {
            return;
        }
        let from = self.w.kernel.borrow().scroll_offset(node);
        self.w.interaction.borrow_mut().pointer.momentum = Some(Momentum { node, start: now, from, dir: (vx / speed, vy / speed), speed });
    }

    /// Momentum at the current time: an exponential slowdown, stopping below the stop speed or
    /// at the content's edge.
    fn glide(&self) {
        let Some(m) = self.w.interaction.borrow().pointer.momentum else { return };
        let tuning = self.app.tuning.get();
        let d = tuning.momentum_decay as f64;
        let stop = (tuning.momentum_stop / 1000.0) as f64;
        let t_end = (stop / m.speed as f64).ln() / d.ln();
        let t = (self.now_ms() - m.start).min(t_end);
        let travel = (m.speed as f64 * (d.powf(t) - 1.0) / d.ln()) as f32;
        let mut k = self.w.kernel.borrow_mut();
        if !k.is_alive(m.node) {
            drop(k);
            self.w.interaction.borrow_mut().pointer.momentum = None;
            return;
        }
        let before = k.scroll_offset(m.node);
        k.scroll_to(m.node, m.from.0 + m.dir.0 * travel, m.from.1 + m.dir.1 * travel);
        let after = k.scroll_offset(m.node);
        drop(k);
        let at_edge = before == after && t > 0.0;
        if t >= t_end || at_edge {
            self.w.interaction.borrow_mut().pointer.momentum = None;
        }
    }

    /// After a frame: the mouse hovers whatever is under it now (content moved, appeared or
    /// went away under a still pointer). While layout is moving, hover holds still until the
    /// motion settles or the pointer moves (Rune: rows sliding under the pointer do not each
    /// light up). Returns whether the hover changed.
    pub(crate) fn refresh_hover(&self) -> bool {
        let Some((x, y)) = self.w.interaction.borrow().pointer.hover_at else { return false };
        if self.w.kernel.borrow().layout_in_motion() {
            self.w.interaction.borrow_mut().hover_held = true;
            return false;
        }
        self.w.interaction.borrow_mut().hover_held = false;
        let hit = self.hit_at(x, y);
        if self.w.interaction.borrow().hovered == hit {
            return false;
        }
        self.set_hover(hit);
        true
    }

    /// Hover is being held while layout moves.
    pub(super) fn hover_held(&self) -> bool {
        self.w.interaction.borrow().hover_held
    }
}
