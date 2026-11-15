//! Focus movement and modals (Rune's model): Tab order, arrow keys in focus groups, and a
//! stack of open modals that trap focus, make the rest of the window inert, and give focus back
//! when they close.

use super::{Modality, Window};
use crate::kernel::{Address, Arrow, HandlerKind, NodeId, State};

/// An open modal and the element focused before it opened.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OpenModal {
    pub node: NodeId,
    restore: Option<NodeId>,
}

/// A reload replaces node handles, but preserves the modal's original focus history.
pub(super) struct SavedModal {
    node: Address,
    restore: Option<Address>,
}

impl Window {
    pub(super) fn save_modals(&self) -> Vec<SavedModal> {
        let k = self.w.kernel.borrow();
        self.w
            .interaction
            .borrow()
            .modals
            .iter()
            .filter(|m| k.is_alive(m.node))
            .map(|m| SavedModal { node: k.address(m.node), restore: m.restore.filter(|n| k.is_alive(*n)).map(|n| k.address(n)) })
            .collect()
    }

    /// The open modal input is confined to.
    pub(crate) fn top_modal(&self) -> Option<NodeId> {
        self.w.interaction.borrow().modals.last().map(|m| m.node)
    }

    /// `hit`, unless an open modal makes it inert.
    pub(crate) fn live(&self, hit: Option<NodeId>) -> Option<NodeId> {
        match (hit, self.top_modal()) {
            (Some(h), Some(m)) if !self.w.kernel.borrow().is_within(h, m) => None,
            _ => hit,
        }
    }

    /// A press landed outside the open modal: it reaches nothing, and dismisses the modal if
    /// it allows that.
    pub(crate) fn pressed_outside(&self, hit: Option<NodeId>) -> bool {
        let Some(m) = self.top_modal() else { return false };
        if hit.is_some_and(|h| self.w.kernel.borrow().is_within(h, m)) {
            return false;
        }
        if self.w.kernel.borrow().modal_click_outside(m) {
            self.run(m, &[HandlerKind::Dismiss], None);
        }
        true
    }

    /// Brings the modal stack in line with the modals shown: a closed one gives focus back to
    /// what had it before; a new one takes focus. Returns whether anything changed.
    pub(crate) fn sync_modals(&self) -> bool {
        let shown = self.w.kernel.borrow().modals();
        let saved = std::mem::take(&mut self.w.interaction.borrow_mut().reload_modals);
        let reloaded = !saved.is_empty();
        let mut changed = reloaded;
        let mut restore = None;
        if reloaded {
            let k = self.w.kernel.borrow();
            let mut i = self.w.interaction.borrow_mut();
            for m in saved {
                let target = m.restore.and_then(|a| k.find_address(&a));
                match k.find_address(&m.node).filter(|n| shown.contains(n)) {
                    Some(node) => i.modals.push(OpenModal { node, restore: target }),
                    // Removing a modal in the rebuilt view closes it just like an ordinary
                    // unmount. The outermost one that disappeared supplies the focus target.
                    None if restore.is_none() => restore = Some(target),
                    None => {}
                }
            }
        }
        {
            let mut i = self.w.interaction.borrow_mut();
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
            self.restore_modal_focus(r);
        }
        for m in shown {
            if self.w.interaction.borrow().modals.iter().any(|o| o.node == m) {
                continue;
            }
            changed = true;
            let focused = self.w.interaction.borrow().focused;
            self.w.interaction.borrow_mut().modals.push(OpenModal { node: m, restore: focused });
            // Focus already inside (a hot reload restored it) stays.
            let inside = focused.is_some_and(|f| self.w.kernel.borrow().is_alive(f) && self.w.kernel.borrow().is_within(f, m));
            if !inside {
                let first = self.w.kernel.borrow().initial_focus(m);
                let _ = self.set_focus(first);
            }
        }
        // The focused element itself may have disappeared in a reload while its modal
        // survived. Keep focus contained without replacing the modal's original opener.
        if reloaded && let Some(m) = self.top_modal() {
            let focused = self.w.interaction.borrow().focused;
            let k = self.w.kernel.borrow();
            let inside =
                focused.is_some_and(|n| k.is_shown(n) && k.is_focusable(n) && !k.state(n).contains(State::DISABLED) && k.is_within(n, m));
            drop(k);
            if !inside {
                self.restore_modal_focus(None);
            }
        }
        changed
    }

    fn restore_modal_focus(&self, restore: Option<NodeId>) {
        let modal = self.top_modal();
        let k = self.w.kernel.borrow();
        let target = restore
            .filter(|n| {
                k.is_shown(*n) && k.is_focusable(*n) && !k.state(*n).contains(State::DISABLED) && modal.is_none_or(|m| k.is_within(*n, m))
            })
            .or_else(|| modal.and_then(|m| k.initial_focus(m)));
        drop(k);
        let _ = self.set_focus(target);
    }

    /// Tab (`forward`) or Shift+Tab: the next stop in the Tab order of the open modal (else the
    /// window), wrapping.
    pub(crate) fn tab(&self, forward: bool) {
        self.set_modality(Modality::Keyboard);
        let focused = self.w.interaction.borrow().focused;
        let order = self.w.kernel.borrow().focus_order(self.top_modal(), focused);
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
        let Some(f) = self.w.interaction.borrow().focused else { return false };
        let (to, group) = {
            let k = self.w.kernel.borrow();
            let to = k.group_step(f, arrow);
            (to, to.filter(|n| *n != f || k.checked(*n) != Some(true)).and_then(|n| k.radio_group(n)))
        };
        let Some(n) = to else { return false };
        if self.set_focus(Some(n)).is_err() {
            return false;
        }
        if let Some(group) = group {
            // Focus handlers may remove, disable or redirect away from this radio. Let those
            // changes land before choosing it, then check the original allocation and group.
            self.settle_frame();
            let choose = {
                let k = self.w.kernel.borrow();
                self.w.interaction.borrow().focused == Some(n)
                    && k.is_shown(n)
                    && k.is_focusable(n)
                    && !k.state(n).contains(State::DISABLED)
                    && k.radio_group(n) == Some(group)
                    && self.live(Some(n)) == Some(n)
            };
            if choose {
                self.run(n, &[HandlerKind::Press], None);
            }
        }
        true
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
