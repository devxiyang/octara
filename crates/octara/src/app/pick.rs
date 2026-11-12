//! Picking ([core](https://github.com/octoii-works/octara/blob/main/docs/design/core.md)): a
//! person points at an element in a running window to hand it to an agent. While a window is
//! picking (`InputEvent::Picking(true)`), the element under the pointer is outlined, pointer
//! input goes to picking instead of the app, and a click picks: the result
//! goes to the clipboard as one line and to the platform in the next `Frame`, and picking ends.
//! Escape ends it too. Wheels still scroll, to bring the element into view.

use super::{InputEvent, Phase, PointerEvent, Window};
use crate::kernel::NodeId;

/// The picking outline's color: strong on light and dark surfaces alike.
const PICKING: crate::kernel::Color = crate::kernel::rgb(0xe11d48);

/// A window's picking state.
#[derive(Default)]
pub(crate) struct Picking {
    pub(crate) on: bool,
    /// What the pointer is over.
    hover: Option<NodeId>,
    /// Picked, for the next frame.
    picked: Option<crate::inspect::Picked>,
    /// What picking shows changed since the last frame.
    changed: bool,
}

impl Window {
    /// Starts or ends picking (an input, so a recording replays it).
    pub fn set_picking(&mut self, on: bool) {
        self.event(InputEvent::Picking(on));
    }

    /// Whether the window is picking.
    pub fn picking(&self) -> bool {
        self.w.picking.borrow().on
    }

    pub(super) fn on_picking(&mut self, on: bool) {
        self.input_frame(|| {
            let mut p = self.w.picking.borrow_mut();
            if p.on != on {
                p.on = on;
                p.hover = None;
                p.changed = true;
            }
        });
    }

    /// Takes `event` when the window is picking; `false` lets it through.
    pub(super) fn picks(&mut self, event: &InputEvent) -> bool {
        if !self.w.picking.borrow().on {
            return false;
        }
        match event {
            InputEvent::Pointer(PointerEvent { phase, x, y, .. }) => {
                let (phase, x, y) = (*phase, *x, *y);
                self.input_frame(|| {
                    let hit = self.w.kernel.borrow().hit_test(x, y);
                    match phase {
                        Phase::Up => {
                            let picked = self.pick(x, y);
                            if let Some(p) = &picked {
                                self.app.system.borrow_mut().write_text(&p.text);
                            }
                            let mut s = self.w.picking.borrow_mut();
                            *s = Picking { on: false, hover: None, picked, changed: true };
                        }
                        Phase::Cancel => {}
                        Phase::Down | Phase::Move => {
                            let mut s = self.w.picking.borrow_mut();
                            if s.hover != hit {
                                s.hover = hit;
                                s.changed = true;
                            }
                        }
                    }
                });
                true
            }
            InputEvent::PointerLeave => {
                self.input_frame(|| {
                    let mut s = self.w.picking.borrow_mut();
                    s.changed |= s.hover.take().is_some();
                });
                true
            }
            InputEvent::Key(k) if k.key == crate::app::Key::Escape => {
                self.on_picking(false);
                true
            }
            _ => false,
        }
    }

    /// What picking draws over the frame: the hovered element's outline, tinted inside.
    pub(super) fn picking_overlay(&self) -> Option<Vec<crate::paint::Item>> {
        let p = self.w.picking.borrow();
        let hover = p.hover.filter(|_| p.on)?;
        let k = self.w.kernel.borrow();
        if !k.is_alive(hover) {
            return None;
        }
        let (x, y, w, h) = k.shown_rect(hover);
        Some(vec![crate::paint::Item::Quad(crate::paint::Quad {
            source: hover,
            rect: crate::paint::Rect::new(x, y, w, h),
            radius: 2.0,
            fill: Some(PICKING.with_alpha(0.12)),
            gradient: None,
            border: [1.5; 4],
            border_color: PICKING,
        })])
    }

    /// What picking adds to the next frame: whether to draw again, and what was picked.
    pub(super) fn picking_frame(&self) -> (bool, Option<crate::inspect::Picked>) {
        let mut p = self.w.picking.borrow_mut();
        (std::mem::take(&mut p.changed), p.picked.take())
    }
}
