//! Pointer input — mouse, touch and pen — as raw events from a shell, a test or an agent, and
//! the recognizers that turn them into presses, double presses, context menu requests, long
//! presses, drags and touch scrolling (with momentum).
//!
//! One target per pointer (Rune): the topmost element under it that handles the pointer. From
//! the press on, that pointer belongs to the target until it lifts or is cancelled.

use super::event::{InputEvent, WheelPhase};
use super::frame::AfterUpdate;
use super::{GuardTarget, Handler, Modality, ScopedHandler, TargetGuard, Window, WindowState};
use crate::kernel::{HandlerKind, Kernel, Kind, NodeId, State};
use bitflags::bitflags;
use std::panic::Location;
use std::rc::Rc;
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
    /// The changed button for `Down` and `Up`.
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

/// When a coordinated touch delta is offered relative to the viewport's own scroll.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TouchScrollStage {
    Before,
    After,
}

/// A delta belongs to the captured allocation and contact that queued it.
pub(super) struct TouchScrollDelta {
    capture: Rc<()>,
    node: NodeId,
    available: [f32; 2],
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
    /// Clones used during callbacks can only update this same captured press afterward.
    capture: Rc<()>,
    pointer: u64,
    kind: PointerKind,
    down_button: Button,
    /// The action chosen on down, including the Apple Control-click mapping.
    action_button: Button,
    /// The topmost node under the press.
    hit: Option<NodeId>,
    /// The element the press acts on.
    target: Option<NodeId>,
    /// The editor that still owns focus until this touch qualifies as a press.
    editing_owner: Option<NodeId>,
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
    /// Selecting text in this selectable region (the press landed on its text, not on
    /// anything pressable).
    selecting: Option<NodeId>,
    /// A long/context handler ran for this held touch (not merely a spent press).
    long_pressed: bool,
}

/// An inside touch release whose focus handoff can change the layout before activation.
pub(super) struct TouchPress {
    held: Rc<Held>,
    target: TargetGuard,
}

impl TouchPress {
    pub(super) fn resident_nodes(&self, nodes: &mut Vec<NodeId>) {
        self.held.resident_nodes(nodes);
    }
}

impl Held {
    fn matches_pointer(&self, ev: PointerEvent) -> bool {
        self.pointer == ev.id && self.kind == ev.kind
    }

    fn resident_nodes(&self, nodes: &mut Vec<NodeId>) {
        match self.mode {
            Mode::Pending | Mode::Dragging => nodes.extend(self.hit.into_iter().chain(self.target).chain(self.selecting)),
            // An inner scroll container may itself belong to an outer virtual row. Its old
            // hit row is free to leave while this container keeps the touch scroll.
            Mode::Panning(container) => nodes.push(container),
            Mode::Spent => {}
        }
    }

    pub(super) fn unpress(&self, kernel: &mut Kernel) {
        if let Some(target) = self.target.filter(|&target| kernel.is_alive(target)) {
            kernel.set_state(target, State::PRESSED, false);
        }
    }
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

/// One platform wheel gesture. A release remembers the same allocation so the
/// subsequent momentum can use it. Idle drops all events until a new direct beginning.
#[derive(Clone, Copy, Default)]
enum WheelSequence {
    #[default]
    Idle,
    Waiting,
    Direct(NodeId),
    Released(NodeId),
    Momentum(NodeId),
}

impl WheelSequence {
    fn owner(self) -> Option<NodeId> {
        match self {
            Self::Direct(node) | Self::Released(node) | Self::Momentum(node) => Some(node),
            Self::Idle | Self::Waiting => None,
        }
    }

    fn advance(&mut self, phase: WheelPhase, moving: bool, pick: impl FnOnce() -> Option<NodeId>) -> Option<NodeId> {
        use Phase::{Cancel, Down, Move, Up};
        match (phase, *self) {
            (WheelPhase::Discrete, _) => return moving.then(pick).flatten(),
            (WheelPhase::Direct(Down), _) => *self = Self::Waiting,
            (WheelPhase::Direct(Move), Self::Waiting | Self::Direct(_)) => {}
            (WheelPhase::Direct(Up), Self::Direct(node)) => {
                *self = Self::Released(node);
                return moving.then_some(node);
            }
            (WheelPhase::Momentum(Down), Self::Released(node)) => {
                *self = Self::Momentum(node);
                return moving.then_some(node);
            }
            (WheelPhase::Momentum(Move), Self::Momentum(node)) => return moving.then_some(node),
            (WheelPhase::Momentum(Up), Self::Momentum(node)) => {
                *self = Self::Idle;
                return moving.then_some(node);
            }
            (WheelPhase::Direct(Cancel), Self::Waiting | Self::Direct(_))
            | (WheelPhase::Momentum(Cancel), Self::Released(_) | Self::Momentum(_))
            | (WheelPhase::Direct(Up), Self::Waiting) => {
                *self = Self::Idle;
                return None;
            }
            _ => return None,
        }
        if !moving {
            return None;
        }
        match *self {
            Self::Waiting => {
                let owner = pick();
                *self = owner.map_or(Self::Idle, Self::Direct);
                owner
            }
            Self::Direct(node) => Some(node),
            _ => unreachable!("a direct wheel update"),
        }
    }
}

#[derive(Default)]
pub(crate) struct PointerState {
    pub held: Option<Held>,
    /// Captures being delivered their final callback. Nested input can settle layout during
    /// that callback; ownership ends only when delivery returns.
    finishing: Vec<Rc<Held>>,
    /// Only while a pointer press handler runs; semantic and keyboard presses have no point.
    press_at: Option<(f32, f32)>,
    last_press: Option<LastPress>,
    click_sequence: u64,
    momentum: Option<Momentum>,
    wheel: WheelSequence,
    pub(super) last_touch_scroll: Option<crate::inspect::TouchScrollState>,
    /// Where the mouse (or pen) is, for hover after layout changes.
    pub hover_at: Option<(f32, f32)>,
}

impl PointerState {
    pub(super) fn resident_nodes(&self, nodes: &mut Vec<NodeId>) {
        for held in self.held.iter().chain(self.finishing.iter().map(Rc::as_ref)) {
            held.resident_nodes(nodes);
        }
    }

    pub(super) fn retire(&mut self, within: &impl Fn(NodeId) -> bool) -> Option<Held> {
        if self.momentum.is_some_and(|momentum| within(momentum.node)) {
            self.momentum = None;
        }
        if self.wheel.owner().is_some_and(within) {
            self.wheel = WheelSequence::Idle;
        }
        let mut nodes = Vec::new();
        self.held.as_ref()?.resident_nodes(&mut nodes);
        nodes.into_iter().any(within).then(|| self.held.take()).flatten()
    }
}

impl WindowState {
    pub(crate) fn pointer_position_in(&self, id: &str) -> Option<(f32, f32)> {
        let at = self.interaction.borrow().pointer.press_at?;
        let kernel = self.kernel.borrow();
        let node = kernel.find(id)?;
        let (x, y, _, _) = kernel.rect(node);
        Some((at.0 - x, at.1 - y))
    }
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn remaining(available: [f32; 2], used: [f32; 2]) -> [f32; 2] {
    std::array::from_fn(|i| {
        let delta = available[i];
        let used = if used[i].is_finite() { used[i].clamp(delta.min(0.0), delta.max(0.0)) } else { 0.0 };
        delta - used
    })
}

fn difference(value: [f32; 2], subtrahend: [f32; 2]) -> [f32; 2] {
    [value[0] - subtrahend[0], value[1] - subtrahend[1]]
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

    fn scroll_target(&self, hit: Option<NodeId>, dx: f32, dy: f32) -> Option<NodeId> {
        let k = self.w.kernel.borrow();
        let modal = self.top_modal();
        k.scroller_above(hit, dx, dy).filter(|&id| k.check_interaction_scope(id, modal).is_ok())
    }

    /// A captured field that still owns a live, non-exiting input node.
    fn pointer_field(&self, n: Option<NodeId>) -> Option<NodeId> {
        let n = n?;
        let kernel = self.w.kernel.borrow();
        // A drag handler can remove its allocation while this pointer is still captured.
        // Cancellation and later native events must release it without reading a retired node.
        if !kernel.is_alive(n) || kernel.is_exiting(n) {
            return None;
        }
        (kernel.kind(n) == Kind::TextInput).then_some(n)
    }
    /// Mouse and pen drag a field's selection; touch remains available for scrolling.
    fn field_drag_target(&self, h: &Held) -> Option<NodeId> {
        if h.kind == PointerKind::Touch { None } else { self.pointer_field(h.target) }
    }

    pub(super) fn now_ms(&self) -> f64 {
        self.app.sched.now_ms()
    }

    /// One raw pointer event, as a window delivers it.
    pub(super) fn on_pointer(&mut self, ev: PointerEvent) {
        self.input_frame(|| {
            let before = self.w.interaction_nodes();
            match ev.phase {
                Phase::Down => self.down(ev),
                Phase::Move => self.moved(ev),
                Phase::Up => self.up(ev),
                Phase::Cancel => self.cancel(ev),
            }
            if before != self.w.interaction_nodes() {
                self.w.interaction_changed();
            }
        });
    }

    pub(super) fn cancel_pointer(&self) {
        let held = self.w.interaction.borrow_mut().pointer.held.take();
        if let Some(held) = held {
            self.cancel_held(held);
        }
        let mut interaction = self.w.interaction.borrow_mut();
        interaction.pointer.momentum = None;
        interaction.pointer.wheel = WheelSequence::Idle;
    }

    pub(super) fn restrict_pointer_to_modal(&self, modal: NodeId) {
        let held = {
            let k = self.w.kernel.borrow();
            let outside = |node| k.check_interaction_scope(node, Some(modal)).is_err();
            let mut i = self.w.interaction.borrow_mut();
            if i.pointer.wheel.owner().is_some_and(outside) {
                i.pointer.wheel = WheelSequence::Idle;
            }
            if i.pointer.momentum.is_some_and(|momentum| outside(momentum.node)) {
                i.pointer.momentum = None;
            }
            i.pointer.held.take_if(|held| {
                let panning = match held.mode {
                    Mode::Panning(node) => Some(node),
                    _ => None,
                };
                [held.hit, held.target, held.selecting, panning].into_iter().flatten().any(outside)
            })
        };
        if let Some(held) = held {
            self.cancel_held(held);
            self.w.interaction_changed();
        }
    }

    fn finishing_pointer<R>(&self, held: Rc<Held>, callback: impl FnOnce(&Rc<Held>) -> R) -> R {
        struct Delivery<'a>(&'a WindowState, Rc<Held>);
        impl Drop for Delivery<'_> {
            fn drop(&mut self) {
                self.0.interaction.borrow_mut().pointer.finishing.retain(|held| !Rc::ptr_eq(held, &self.1));
                if !self.0.closed.get() {
                    self.0.interaction_changed();
                }
            }
        }
        let delivery = Delivery(&self.w, held);
        self.w.interaction.borrow_mut().pointer.finishing.push(delivery.1.clone());
        callback(&delivery.1)
    }

    fn update_held(&self, held: Held) {
        let mut interaction = self.w.interaction.borrow_mut();
        if interaction.pointer.held.as_ref().is_some_and(|current| Rc::ptr_eq(&current.capture, &held.capture)) {
            interaction.pointer.held = Some(held);
        }
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

    /// One independently routed wheel delta. The nearest viewport supporting its axis takes it.
    pub fn wheel(&mut self, x: f32, y: f32, dx: f32, dy: f32) {
        self.event(InputEvent::Wheel { x, y, dx, dy, phase: WheelPhase::Discrete });
    }
    pub(super) fn on_wheel(&mut self, x: f32, y: f32, dx: f32, dy: f32, phase: WheelPhase) {
        self.input_frame(|| {
            let mut sequence = {
                let mut interaction = self.w.interaction.borrow_mut();
                interaction.pointer.momentum = None;
                interaction.pointer.wheel
            };
            let moving = [dx, dy] != [0.0; 2] && dx.is_finite() && dy.is_finite();
            let target = sequence.advance(phase, moving, || self.scroll_target(self.hit_at(x, y), dx, dy));
            let modal = self.top_modal();
            let valid = target.filter(|node| self.w.kernel.borrow().check_interaction_scope(*node, modal).is_ok());
            if target != valid {
                sequence = WheelSequence::Idle;
            }
            self.w.interaction.borrow_mut().pointer.wheel = sequence;
            if let Some(n) = valid {
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

    /// The pointer currently held by this window. A host can avoid starting synthetic input
    /// while a person's pointer is already down, and release only the pointer it submitted.
    pub fn held_pointer(&self) -> Option<(u64, PointerKind)> {
        self.w.interaction.borrow().pointer.held.as_ref().map(|h| (h.pointer, h.kind))
    }

    /// Whether the current held pointer actually invoked a long-press or context-menu handler.
    /// Cancellation, a stopped glide and a disabled control do not count as recognition.
    pub fn pointer_long_pressed(&self, pointer: u64) -> bool {
        self.w.interaction.borrow().pointer.held.as_ref().is_some_and(|h| h.pointer == pointer && h.long_pressed)
    }

    /// How long a host should wait before starting a new, independent mouse double click.
    /// This reads the recognizer's existing click history without changing it or moving time.
    pub fn pointer_click_cooldown(&self) -> Duration {
        let i = self.w.interaction.borrow();
        let Some(last) = i.pointer.last_press else { return Duration::ZERO };
        let remaining = last.at + self.tuning().double_click.as_secs_f64() * 1000.0 - self.now_ms();
        if remaining < 0.0 {
            Duration::ZERO
        } else {
            Duration::try_from_secs_f64(remaining / 1000.0).unwrap_or(Duration::MAX).saturating_add(Duration::from_millis(1))
        }
    }

    /// A local sequence number for counted pointer presses. Hosts can detect a completed
    /// intervening click between phases of synthetic input. Reloading the tree resets it.
    pub fn pointer_click_sequence(&self) -> u64 {
        self.w.interaction.borrow().pointer.click_sequence
    }

    /// The pointer's next deadline (ms): a long press coming due, or momentum's next frame.
    pub(super) fn pointer_deadline(&self) -> Option<f64> {
        if self.w.interaction.borrow().pointer.momentum.is_some() {
            return Some(self.now_ms() + FRAME_MS);
        }
        self.pointer_due()
    }

    /// When something the pointer started comes due: a long press, a tooltip.
    pub(super) fn pointer_due(&self) -> Option<f64> {
        self.long_press_due().into_iter().chain(self.tip_due()).min_by(f64::total_cmp)
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
        self.tip_time();
        if self.w.interaction.borrow().pointer.momentum.is_some() {
            self.glide();
        }
    }

    pub(super) fn long_press_due(&self) -> Option<f64> {
        let i = self.w.interaction.borrow();
        let h = i.pointer.held.as_ref()?;
        (h.kind == PointerKind::Touch && h.mode == Mode::Pending && h.action_button == Button::Primary)
            .then(|| crate::task::whole_ns(h.down_at + self.app.tuning.get().long_press.as_secs_f64() * 1000.0))
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
        // Dismiss at most one layer. A nonmodal dismissal can keep the original hit, but its
        // callback can also retire the hit or the entire window.
        let raw = self.w.kernel.borrow().hit_test(ev.x, ev.y);
        let outside = self.pressed_outside(raw);
        if self.is_closed() || self.app.shutdown.get() {
            return;
        }
        let modal = self.top_modal();
        let (hit, target, button) = {
            let k = self.w.kernel.borrow();
            let hit = raw.filter(|&node| !outside && k.check_interaction_scope(node, modal).is_ok());
            let control_click = ev.button == Button::Primary
                && ev.kind == PointerKind::Mouse
                && ev.modifiers.contains(Modifiers::CTRL)
                && self.app.platform.get().is_apple();
            let button = if control_click { Button::Secondary } else { ev.button };
            let target = k.press_target(hit, button).filter(|&node| k.check_interaction_scope(node, modal).is_ok());
            (hit, target, button)
        };
        if ev.kind == PointerKind::Touch {
            self.w.interaction.borrow_mut().pointer.hover_at = None;
            self.set_hover(None);
        }
        let preserve_focus = {
            let k = self.w.kernel.borrow();
            k.up_from(hit, |n| k.preserves_focus_on_press(n)).is_some()
        };
        let mut editing_owner = None;
        if button == Button::Primary && !outside && !preserve_focus {
            let (focus, defer) = {
                let k = self.w.kernel.borrow();
                let focus = target.filter(|&n| k.is_focusable(n));
                let defer = ev.kind == PointerKind::Touch
                    && !stopped
                    && target.is_some_and(|n| k.kind(n) != Kind::TextInput && k.check_activation(n, modal).is_ok());
                (focus, defer)
            };
            if defer {
                // Blurring an editor can move targets when the software keyboard changes size.
                editing_owner = self.pointer_field(self.focused_node());
            }
            if editing_owner.is_none() {
                // Fields take focus at contact so the caret can be placed immediately.
                let _ = self.set_focus(focus);
            }
        }
        // A focus/blur handler can synchronously rebuild the tree. The press still belongs
        // to the original hit; never dereference it or adopt a replacement allocation.
        let (hit, target) = {
            let k = self.w.kernel.borrow();
            let live = |node: Option<NodeId>| node.filter(|node| k.is_alive(*node) && !k.is_exiting(*node));
            (live(hit), live(target))
        };
        let enabled = target.is_some_and(|n| !self.w.kernel.borrow().state(n).contains(State::DISABLED));
        // A press on a selectable region's text, not on anything pressable, selects.
        // (A target around the region, such as the scroll container it is, does not stop it.)
        let selecting = (button == Button::Primary && !outside && !stopped)
            .then(|| {
                let k = self.w.kernel.borrow();
                let modal = self.top_modal();
                hit.and_then(|h| k.region_of(h))
                    .filter(|&r| k.check_interaction_scope(r, modal).is_ok())
                    .filter(|r| target.is_none_or(|t| k.is_within(*r, t)))
            })
            .flatten();
        let can_pan = !enabled
            && !stopped
            && ev.kind == PointerKind::Touch
            && button == Button::Primary
            && self.scroll_target(hit, 1.0, 1.0).is_some();
        let target = target.filter(|_| enabled || !can_pan);
        let mode = if selecting.is_some() || can_pan || (!stopped && enabled) { Mode::Pending } else { Mode::Spent };
        // The run of presses (double click) counts from the press (AppKit, Rune).
        let clicks = match target.or(selecting) {
            Some(n) if mode == Mode::Pending && button == Button::Primary => self.count_press(n, ev.kind, (ev.x, ev.y)),
            _ => 1,
        };
        match selecting {
            Some(region) => self.select_press(region, &ev, clicks),
            // A press anywhere else ends a document selection (Shift keeps it to extend).
            None if button == Button::Primary && !ev.modifiers.contains(Modifiers::SHIFT) => {
                self.clear_doc_selection();
            }
            None => {}
        }
        if mode == Mode::Pending
            && button == Button::Primary
            && let Some(n) = target
        {
            self.w.kernel.borrow_mut().set_state(n, State::PRESSED, true);
        }
        let now = self.now_ms();
        self.w.interaction.borrow_mut().pointer.held = Some(Held {
            capture: Rc::new(()),
            pointer: ev.id,
            kind: ev.kind,
            down_button: ev.button,
            action_button: button,
            hit,
            target,
            editing_owner,
            start: (ev.x, ev.y),
            at: (ev.x, ev.y),
            down_at: now,
            inside: true,
            mode,
            clicks,
            samples: vec![(now, ev.x, ev.y)],
            selecting,
            long_pressed: false,
        });
        // A field places its caret on the primary press.
        if mode == Mode::Pending
            && button == Button::Primary
            && let Some(n) = self.pointer_field(target)
        {
            self.press_in_field(n, (ev.x, ev.y), clicks, ev.modifiers.contains(Modifiers::SHIFT));
        }
    }

    fn moved(&self, ev: PointerEvent) {
        if ev.kind != PointerKind::Touch {
            self.w.interaction.borrow_mut().pointer.hover_at = Some((ev.x, ev.y));
            let hit = self.hit_at(ev.x, ev.y);
            self.w.interaction.borrow_mut().hover_held = false;
            self.set_hover(hit);
        }
        let Some(mut h) = self.w.interaction.borrow().pointer.held.as_ref().filter(|h| h.matches_pointer(ev)).cloned() else { return };
        // Selecting: the focus follows the pointer.
        if let Some(region) = h.selecting
            && matches!(h.mode, Mode::Pending | Mode::Dragging)
        {
            if let Some(p) = self.point_at(region, ev.x, ev.y)
                && let Some(sel) = self.w.selection.borrow_mut().as_mut().filter(|s| s.region == region)
            {
                sel.focus = p;
            }
            h.at = (ev.x, ev.y);
            if h.mode == Mode::Pending {
                // A drag ends a run of clicks.
                self.w.interaction.borrow_mut().pointer.last_press = None;
            }
            h.mode = Mode::Dragging;
            self.update_held(h);
            return;
        }
        let prev = h.at;
        h.at = (ev.x, ev.y);
        let now = self.now_ms();
        h.samples.retain(|s| now - s.0 <= VELOCITY_WINDOW_MS);
        h.samples.push((now, ev.x, ev.y));
        let tuning = self.app.tuning.get();
        let slop = if h.kind == PointerKind::Touch { tuning.touch_slop } else { tuning.mouse_slop };
        if h.mode == Mode::Pending && distance(h.start, h.at) > slop {
            // Field selection takes a mouse/pen drag before the application's `on_drag`.
            let field = self.field_drag_target(&h);
            let drags = h.action_button == Button::Primary
                && (field.is_some() || h.target.is_some_and(|n| self.w.kernel.borrow().handler(n, HandlerKind::Drag).is_some()));
            if drags {
                h.mode = Mode::Dragging;
                // A drag ends a run of clicks.
                self.w.interaction.borrow_mut().pointer.last_press = None;
                match field {
                    Some(n) => self.drag_in_field(n, h.at),
                    None => self.send_drag(&h, DragPhase::Start),
                }
                self.update_held(h);
                return;
            } else if h.kind == PointerKind::Touch {
                let (dx, dy) = (h.start.0 - h.at.0, h.start.1 - h.at.1);
                // The finger's main direction picks the container (a carousel inside a list).
                let (ax, ay) = if dx.abs() > dy.abs() { (dx, 0.0) } else { (0.0, dy) };
                let scroller = self.scroll_target(h.hit, ax, ay).or_else(|| self.scroll_target(h.hit, dx, dy));
                if let Some(c) = scroller {
                    h.mode = Mode::Panning(c);
                    self.unpress(&h);
                    self.w.interaction.borrow_mut().pointer.last_press = None;
                    // The pan starts from the press, so the slop is not lost.
                    self.pan_by(&h, c, dx, dy);
                    self.update_held(h);
                    return;
                }
            }
        }
        match h.mode {
            Mode::Pending => {
                // Leaving the element un-presses it; coming back presses it again (AppKit).
                let inside = {
                    let k = self.w.kernel.borrow();
                    h.target.is_some() && k.press_target(self.hit_at(ev.x, ev.y), h.action_button) == h.target
                };
                if inside != h.inside {
                    h.inside = inside;
                    if let Some(n) = h.target
                        && h.action_button == Button::Primary
                    {
                        self.w.kernel.borrow_mut().set_state(n, State::PRESSED, inside);
                    }
                }
            }
            Mode::Dragging => match self.field_drag_target(&h) {
                Some(n) => self.drag_in_field(n, h.at),
                None => self.send_drag(&h, DragPhase::Move),
            },
            Mode::Panning(c) => self.pan_by(&h, c, prev.0 - h.at.0, prev.1 - h.at.1),
            Mode::Spent => {}
        }
        self.update_held(h);
    }

    fn up(&self, ev: PointerEvent) {
        let held = self.w.interaction.borrow_mut().pointer.held.take_if(|h| h.matches_pointer(ev) && h.down_button == ev.button);
        let Some(mut h) = held else { return };
        h.at = (ev.x, ev.y);
        self.unpress(&h);
        self.finishing_pointer(Rc::new(h), |h| match h.mode {
            Mode::Pending => {
                let inside = {
                    let k = self.w.kernel.borrow();
                    k.press_target(self.hit_at(ev.x, ev.y), h.action_button) == h.target
                };
                let Some(n) = h.target.filter(|_| inside) else { return };
                if h.action_button == Button::Secondary {
                    self.run(n, &[HandlerKind::SecondaryPress], None);
                } else if h.action_button == Button::Primary {
                    if let Some(editor) = h.editing_owner
                        && self.focused_node() == Some(editor)
                    {
                        let modal = self.top_modal();
                        let focus = {
                            let k = self.w.kernel.borrow();
                            if k.check_activation(n, modal).is_err() {
                                return;
                            }
                            k.is_focusable(n).then_some(n)
                        };
                        let target = TargetGuard::new(self, GuardTarget::Node(n));
                        if self.set_focus(focus).is_ok() && !self.is_closed() {
                            // The release already qualified. Let blur and focus callbacks settle
                            // before running the original target's press.
                            self.w
                                .interaction
                                .borrow_mut()
                                .after_update
                                .push_back(AfterUpdate::TouchPress(TouchPress { held: h.clone(), target }));
                        }
                    } else {
                        self.primary_press(n, h);
                    }
                }
            }
            Mode::Dragging => match self.field_drag_target(h) {
                Some(n) => self.drag_in_field(n, h.at),
                None => self.send_drag(h, DragPhase::End),
            },
            Mode::Panning(c) if !self.coordinates_touch_scroll(c) => self.start_momentum(c, h),
            Mode::Panning(_) | Mode::Spent => {}
        });
    }

    pub(super) fn deliver_touch_press(&self, press: TouchPress) {
        let TouchPress { held, target } = press;
        self.finishing_pointer(held, |h| {
            let Some(node) = target.node().filter(|_| target.is_current(self)) else { return };
            let modal = self.top_modal();
            if h.hit.is_none_or(|hit| self.w.kernel.borrow().check_interaction_scope(hit, modal).is_ok()) {
                self.primary_press(node, h);
            }
        });
    }

    fn primary_press(&self, node: NodeId, held: &Held) {
        self.w.interaction.borrow_mut().pointer.press_at = Some(held.at);
        self.run(node, &[HandlerKind::Press, HandlerKind::Toggle], None);
        self.w.interaction.borrow_mut().pointer.press_at = None;
        if held.clicks == 2 {
            self.run(node, &[HandlerKind::DoublePress], None);
        }
    }

    fn cancel(&self, ev: PointerEvent) {
        let held = self.w.interaction.borrow_mut().pointer.held.take_if(|h| h.matches_pointer(ev));
        if let Some(held) = held {
            self.cancel_held(held);
        }
    }

    fn cancel_held(&self, held: Held) {
        self.unpress(&held);
        self.finishing_pointer(Rc::new(held), |h| {
            if h.mode == Mode::Dragging && self.field_drag_target(h).is_none() {
                self.send_drag(h, DragPhase::Cancel);
            }
        });
    }

    fn unpress(&self, h: &Held) {
        h.unpress(&mut self.w.kernel.borrow_mut());
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
        i.pointer.click_sequence = i.pointer.click_sequence.wrapping_add(1);
        count
    }

    fn send_drag(&self, h: &Held, phase: DragPhase) {
        let Some(n) = h.target else { return };
        let handler = {
            let k = self.w.kernel.borrow();
            if !k.is_alive(n) || k.state(n).contains(State::DISABLED) {
                return;
            }
            k.handler(n, HandlerKind::Drag).and_then(|id| self.w.handlers.borrow().get(&id.0).cloned())
        };
        let Some(handler) = handler else { return };
        // A drag is direct manipulation: what it moves follows the pointer at once.
        if matches!(phase, DragPhase::Start | DragPhase::Move) {
            self.w.kernel.borrow_mut().set_instant(true);
        }
        let (x, y, _, _) = self.w.kernel.borrow().rect(n);
        let d = Drag { phase, x: h.at.0 - x, y: h.at.1 - y, dx: h.at.0 - h.start.0, dy: h.at.1 - h.start.1 };
        handler.run(&self.app.rt, |handler| {
            if let Handler::Drag(f) = handler {
                f(&d);
            }
        });
    }

    fn coordinates_touch_scroll(&self, node: NodeId) -> bool {
        self.w.kernel.borrow().handler(node, HandlerKind::TouchScroll).is_some()
    }

    fn pan_by(&self, held: &Held, node: NodeId, dx: f32, dy: f32) {
        if self.coordinates_touch_scroll(node) {
            if [dx, dy] != [0.0; 2] && dx.is_finite() && dy.is_finite() {
                let pan = TouchScrollDelta { capture: held.capture.clone(), node, available: [dx, dy] };
                self.w.interaction.borrow_mut().after_update.push_back(AfterUpdate::TouchScroll(pan));
            }
        } else {
            self.scroll_by(node, dx, dy);
        }
    }

    fn touch_scroll_handler(&self, pan: &TouchScrollDelta) -> Option<ScopedHandler> {
        if self.is_closed() || self.app.shutdown.get() {
            return None;
        }
        let owns_pan = |held: &Held| Rc::ptr_eq(&held.capture, &pan.capture) && held.mode == Mode::Panning(pan.node);
        if !self.w.interaction.borrow().pointer.held.as_ref().is_some_and(owns_pan) {
            return None;
        }
        let handler = {
            let k = self.w.kernel.borrow();
            // Binding updates can open a modal during the first stage of this delta.
            k.check_activation(pan.node, self.top_modal())
                .ok()
                .and_then(|()| k.handler(pan.node, HandlerKind::TouchScroll))
                .and_then(|id| self.w.handlers.borrow().get(&id.0).cloned())
                .filter(|handler| handler.is_live(&self.app.rt))
        };
        if handler.is_none() {
            let held = self.w.interaction.borrow_mut().pointer.held.take_if(|held| owns_pan(held));
            if let Some(held) = held {
                self.cancel_held(held);
                self.w.interaction_changed();
            }
        }
        handler
    }

    pub(super) fn apply_touch_scroll(&self, pan: TouchScrollDelta, at: &'static Location<'static>) -> Result<bool, ()> {
        let Some(before) = self.touch_scroll_handler(&pan) else { return Ok(false) };
        let (target, origin) = {
            let mut k = self.w.kernel.borrow_mut();
            k.set_instant(true);
            k.stop_glide(pan.node);
            (crate::inspect::node_target(&k, pan.node).expect("live touch viewport"), k.scroll_offset(pan.node))
        };
        let available = self.offer_touch_scroll(before, TouchScrollStage::Before, pan.available);
        // Use the clamped offset from this layout as the baseline for content consumption.
        self.settle_layout(at)?;
        let Some(after) = self.touch_scroll_handler(&pan) else { return Ok(true) };
        let baseline = self.w.kernel.borrow().scroll_offset(pan.node);
        let viewport = if available == [0.0; 2] { [0.0; 2] } else { self.scroll_by(pan.node, available[0], available[1]) };
        let unused = remaining(available, viewport);
        let left = if unused == [0.0; 2] { unused } else { self.offer_touch_scroll(after, TouchScrollStage::After, unused) };
        self.w.interaction.borrow_mut().pointer.last_touch_scroll = Some(crate::inspect::TouchScrollState {
            target,
            input: pan.available,
            before: difference(pan.available, available),
            layout_correction: [baseline.0 - origin.0, baseline.1 - origin.1],
            viewport: difference(available, unused),
            after: difference(unused, left),
            remaining: left,
        });
        Ok(true)
    }

    fn offer_touch_scroll(&self, handler: ScopedHandler, stage: TouchScrollStage, available: [f32; 2]) -> [f32; 2] {
        let used = handler.run(&self.app.rt, |handler| {
            let Handler::TouchScroll(consume) = handler else { unreachable!("touch scroll handler") };
            consume(stage, available)
        });
        remaining(available, used.unwrap_or_default())
    }

    fn scroll_by(&self, n: NodeId, dx: f32, dy: f32) -> [f32; 2] {
        let modal = self.top_modal();
        let mut k = self.w.kernel.borrow_mut();
        if k.check_interaction_scope(n, modal).is_err() {
            return [0.0; 2];
        }
        // The user's own scrolling takes over from a glide.
        k.stop_glide(n);
        let (sx, sy) = k.scrollable(n);
        let (ox, oy) = k.scroll_offset(n);
        k.scroll_to(n, ox + if sx { dx } else { 0.0 }, oy + if sy { dy } else { 0.0 });
        let (nx, ny) = k.scroll_offset(n);
        [nx - ox, ny - oy]
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
            self.w.interaction_changed();
        }
    }

    fn long_press_fire(&self) {
        let Some(h) = self.w.interaction.borrow().pointer.held.clone() else { return };
        let current_hit = self.hit_at(h.at.0, h.at.1);
        let found = {
            let k = self.w.kernel.borrow();
            let live = |n: Option<NodeId>| n.filter(|n| k.is_alive(*n) && !k.is_exiting(*n));
            let target = live(h.target);
            // A timer can remove or cover the target in the same clock step that reaches
            // this deadline. Check here as well as in a live host's next polling turn.
            let inside = target.is_some_and(|target| current_hit.is_some_and(|hit| k.is_within(hit, target)));
            inside
                .then(|| {
                    k.handler_above(target, HandlerKind::LongPress)
                        .map(|n| (n, HandlerKind::LongPress))
                        .or_else(|| k.handler_above(live(h.hit), HandlerKind::SecondaryPress).map(|n| (n, HandlerKind::SecondaryPress)))
                })
                .flatten()
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
            self.finishing_pointer(Rc::new(h), |h| {
                if self.run(n, &[kind], None)
                    && let Some(held) = self.w.interaction.borrow_mut().pointer.held.as_mut()
                    && Rc::ptr_eq(&held.capture, &h.capture)
                {
                    held.long_pressed = true;
                }
            });
        }
    }

    fn start_momentum(&self, node: NodeId, h: &Held) {
        if self.w.kernel.borrow().check_interaction_scope(node, self.top_modal()).is_err() {
            return;
        }
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
        let modal = self.top_modal();
        let mut k = self.w.kernel.borrow_mut();
        if k.check_interaction_scope(m.node, modal).is_err() {
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

impl Window {
    /// A press that selects in `region`: a caret-less start, a word (double), a text (triple),
    /// or, with Shift, the existing selection extended to here.
    fn select_press(&self, region: NodeId, ev: &PointerEvent, clicks: u32) {
        let Some(p) = self.point_at(region, ev.x, ev.y) else { return };
        let extend = ev.modifiers.contains(Modifiers::SHIFT) && self.w.selection.borrow().as_ref().is_some_and(|s| s.region == region);
        if extend {
            if let Some(sel) = self.w.selection.borrow_mut().as_mut() {
                sel.focus = p;
            }
            return;
        }
        let (anchor, focus) = match clicks {
            1 => (p.clone(), p),
            2 => self.unit_at(region, &p, false).unwrap_or((p.clone(), p)),
            _ => self.unit_at(region, &p, true).unwrap_or((p.clone(), p)),
        };
        self.set_doc_selection(region, anchor, focus);
    }
}
