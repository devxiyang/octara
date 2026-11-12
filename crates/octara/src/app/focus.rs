//! Focus movement and modals (Rune's model): Tab order, arrow keys in focus groups, and a
//! stack of open modals that trap focus, make the rest of the window inert, and give focus back
//! when they close.

use super::{App, Modality};
use crate::kernel::{Arrow, HandlerKind, NodeId};

/// An open modal and the element focused before it opened.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OpenModal {
    pub node: NodeId,
    restore: Option<NodeId>,
}

impl App {
    /// The open modal input is confined to.
    pub(crate) fn top_modal(&self) -> Option<NodeId> {
        self.win().interaction.borrow().modals.last().map(|m| m.node)
    }

    /// `hit`, unless an open modal makes it inert.
    pub(crate) fn live(&self, hit: Option<NodeId>) -> Option<NodeId> {
        match (hit, self.top_modal()) {
            (Some(h), Some(m)) if !self.win().ui.kernel.borrow().is_within(h, m) => None,
            _ => hit,
        }
    }

    /// A press landed outside the open modal: it reaches nothing, and dismisses the modal if
    /// it allows that.
    pub(crate) fn pressed_outside(&self, hit: Option<NodeId>) -> bool {
        let Some(m) = self.top_modal() else { return false };
        if hit.is_some_and(|h| self.win().ui.kernel.borrow().is_within(h, m)) {
            return false;
        }
        if self.win().ui.kernel.borrow().modal_click_outside(m) {
            self.run(m, &[HandlerKind::Dismiss], None);
        }
        true
    }

    /// Brings the modal stack in line with the modals shown: a closed one gives focus back to
    /// what had it before; a new one takes focus. Returns whether anything changed.
    pub(crate) fn sync_modals(&self) -> bool {
        let shown = self.win().ui.kernel.borrow().modals();
        let mut changed = false;
        let mut restore = None;
        {
            let mut i = self.win().interaction.borrow_mut();
            let mut closed = false;
            i.modals.retain(|m| {
                let open = shown.contains(&m.node);
                if !open && !closed {
                    // The outermost modal that closed restores focus.
                    closed = true;
                    restore = Some(m.restore);
                }
                open
            });
            changed |= closed;
        }
        if let Some(r) = restore {
            let k = self.win().ui.kernel.borrow();
            let r = r.filter(|n| k.is_alive(*n) && k.is_focusable(*n));
            drop(k);
            let _ = self.set_focus(r);
        }
        for m in shown {
            if self.win().interaction.borrow().modals.iter().any(|o| o.node == m) {
                continue;
            }
            changed = true;
            let focused = self.win().interaction.borrow().focused;
            self.win().interaction.borrow_mut().modals.push(OpenModal { node: m, restore: focused });
            // Focus already inside (a hot reload restored it) stays.
            let inside =
                focused.is_some_and(|f| self.win().ui.kernel.borrow().is_alive(f) && self.win().ui.kernel.borrow().is_within(f, m));
            if !inside {
                let first = self.win().ui.kernel.borrow().initial_focus(m);
                let _ = self.set_focus(first);
            }
        }
        changed
    }

    /// Tab (`forward`) or Shift+Tab: the next stop in the Tab order of the open modal (else the
    /// window), wrapping.
    pub(crate) fn tab(&self, forward: bool) {
        self.set_modality(Modality::Keyboard);
        let focused = self.win().interaction.borrow().focused;
        let order = self.win().ui.kernel.borrow().focus_order(self.top_modal(), focused);
        if order.is_empty() {
            return;
        }
        let at = focused.and_then(|f| order.iter().position(|n| *n == f));
        let next = match (at, forward) {
            (None, true) => 0,
            (None, false) => order.len() - 1,
            (Some(i), true) => (i + 1) % order.len(),
            (Some(i), false) => (i + order.len() - 1) % order.len(),
        };
        let _ = self.set_focus(Some(order[next]));
    }

    /// An arrow (or Home / End) key: moves focus inside the focused element's focus group.
    /// Returns whether it moved.
    pub(crate) fn arrow(&self, arrow: Arrow) -> bool {
        self.set_modality(Modality::Keyboard);
        let Some(f) = self.win().interaction.borrow().focused else { return false };
        let to = self.win().ui.kernel.borrow().group_step(f, arrow);
        match to {
            Some(n) => self.set_focus(Some(n)).is_ok(),
            None => false,
        }
    }

    /// Escape: dismisses the open modal, else drops focus.
    pub(crate) fn escape(&self) {
        match self.top_modal() {
            Some(m) => {
                self.run(m, &[HandlerKind::Dismiss], None);
            }
            None => {
                let _ = self.set_focus(None);
            }
        }
    }
}
