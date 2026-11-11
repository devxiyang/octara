//! Driving an app by element, for tests and agents: each call finds the element, checks that a
//! person could reach it, and sends the raw pointer events a person's input would, at its
//! centre, through the same recognizers (Exact's agent `tap`, Rune's keyed scripts).
//!
//! A target is a test id, else an exact accessible name (a label, or a text's own text). A
//! name several elements share, a covered element and one outside the window are errors.

use super::App;
use super::pointer::{Button, Phase, PointerEvent, PointerKind};
use crate::kernel::{Kernel, NodeId};

impl Kernel {
    /// The element `target` names: a test id, else an accessible name (resolved to the element
    /// a press there acts on).
    pub fn resolve_target(&self, target: &str) -> Result<NodeId, String> {
        match self.find_all(target).as_slice() {
            [n] => return Ok(*n),
            [] => {}
            many => return Err(format!("{} elements have test id `{target}`", many.len())),
        }
        let mut found: Vec<NodeId> = Vec::new();
        for n in self.find_by_name(target) {
            let n = self.pointer_target(Some(n)).unwrap_or(n);
            if !found.contains(&n) {
                found.push(n);
            }
        }
        match found.as_slice() {
            [n] => Ok(*n),
            [] => Err(format!("no element with test id or name `{target}`")),
            many => Err(format!(
                "{} elements are named `{target}` ({}): give the one you mean a test id",
                many.len(),
                many.iter().map(|n| self.describe(*n)).collect::<Vec<_>>().join(", ")
            )),
        }
    }

    /// Where a person would touch `n`: its centre, if that point shows `n` (or something inside
    /// it) and lies in the window.
    pub fn reachable_point(&self, n: NodeId) -> Result<(f32, f32), String> {
        let (x, y, w, h) = self.shown_rect(n);
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let (vw, vh) = self.viewport();
        if w <= 0.0 || h <= 0.0 {
            return Err(format!("{} has no size", self.describe(n)));
        }
        if cx < 0.0 || cy < 0.0 || cx >= vw || cy >= vh {
            return Err(format!("{} is outside the window (centre {cx}, {cy}; window {vw}×{vh}): scroll it into view", self.describe(n)));
        }
        let hit = self.hit_test(cx, cy);
        let mut up = hit;
        while let Some(id) = up {
            if id == n {
                return Ok((cx, cy));
            }
            up = self.parent(id);
        }
        match hit {
            Some(other) if self.is_within(n, other) => {
                Err(format!("{} is clipped away at its centre ({cx}, {cy}): scroll it into view", self.describe(n)))
            }
            Some(other) => Err(format!("{} is covered at its centre ({cx}, {cy}) by {}", self.describe(n), self.describe(other))),
            None => Err(format!("{} is not shown at its centre ({cx}, {cy})", self.describe(n))),
        }
    }
}

impl App {
    fn point_of(&self, target: &str) -> Result<(f32, f32), String> {
        let k = self.ui.kernel.borrow();
        let n = k.resolve_target(target)?;
        if let Some(m) = self.top_modal()
            && !k.is_within(n, m)
        {
            return Err(format!("{} is behind the open modal {}", k.describe(n), k.describe(m)));
        }
        k.reachable_point(n)
    }

    fn press_at(&mut self, (x, y): (f32, f32), kind: PointerKind, button: Button) {
        let ev =
            |phase| PointerEvent { kind, button, id: if kind == PointerKind::Mouse { 0 } else { 1 }, ..PointerEvent::mouse(phase, x, y) };
        if kind != PointerKind::Touch {
            self.pointer(ev(Phase::Move));
        }
        self.pointer(ev(Phase::Down));
        self.pointer(ev(Phase::Up));
    }

    /// Clicks `target` with the primary mouse button.
    pub fn tap(&mut self, target: &str) -> Result<(), String> {
        self.tap_with(target, PointerKind::Mouse)
    }
    /// Presses `target` with a mouse, a finger or a pen.
    pub fn tap_with(&mut self, target: &str, kind: PointerKind) -> Result<(), String> {
        let at = self.point_of(target)?;
        self.press_at(at, kind, Button::Primary);
        Ok(())
    }
    /// Two clicks on `target`, within the double-click interval.
    pub fn double_tap(&mut self, target: &str) -> Result<(), String> {
        let at = self.point_of(target)?;
        self.press_at(at, PointerKind::Mouse, Button::Primary);
        self.advance(std::time::Duration::from_millis(50));
        self.press_at(at, PointerKind::Mouse, Button::Primary);
        Ok(())
    }
    /// A right click on `target`.
    pub fn secondary_tap(&mut self, target: &str) -> Result<(), String> {
        let at = self.point_of(target)?;
        self.press_at(at, PointerKind::Mouse, Button::Secondary);
        Ok(())
    }
    /// A finger held on `target` for the long-press duration, then lifted.
    pub fn long_press(&mut self, target: &str) -> Result<(), String> {
        let (x, y) = self.point_of(target)?;
        self.pointer(PointerEvent::touch(1, Phase::Down, x, y));
        self.advance(self.tuning().long_press);
        self.pointer(PointerEvent::touch(1, Phase::Up, x, y));
        Ok(())
    }
    /// A mouse drag from `target`'s centre by `(dx, dy)`, in a few steps.
    pub fn drag(&mut self, target: &str, dx: f32, dy: f32) -> Result<(), String> {
        let (x, y) = self.point_of(target)?;
        self.pointer(PointerEvent::mouse(Phase::Move, x, y));
        self.pointer(PointerEvent::mouse(Phase::Down, x, y));
        const STEPS: u32 = 4;
        for i in 1..=STEPS {
            let f = i as f32 / STEPS as f32;
            self.advance(std::time::Duration::from_millis(16));
            self.pointer(PointerEvent::mouse(Phase::Move, x + dx * f, y + dy * f));
        }
        self.pointer(PointerEvent::mouse(Phase::Up, x + dx, y + dy));
        Ok(())
    }
    /// A finger swiping from `target`'s centre by `(dx, dy)` over `duration`, then lifting
    /// (content follows the finger and may glide on).
    pub fn swipe(&mut self, target: &str, dx: f32, dy: f32, duration: std::time::Duration) -> Result<(), String> {
        let (x, y) = self.point_of(target)?;
        self.pointer(PointerEvent::touch(1, Phase::Down, x, y));
        let steps = ((duration.as_secs_f64() * 60.0).ceil() as u32).max(1);
        for i in 1..=steps {
            let f = i as f32 / steps as f32;
            self.advance(duration / steps);
            self.pointer(PointerEvent::touch(1, Phase::Move, x + dx * f, y + dy * f));
        }
        self.pointer(PointerEvent::touch(1, Phase::Up, x + dx, y + dy));
        Ok(())
    }
}
