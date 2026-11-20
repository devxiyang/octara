//! Focus movement, layer dismissal and modal focus restoration.

use super::{Modality, Window};
use crate::kernel::{Address, HandlerKind, NodeId};

/// An open modal and the focus targets it can restore, nearest opener first.
#[derive(Debug)]
pub(crate) struct OpenModal {
    pub node: NodeId,
    restore: Vec<NodeId>,
}

/// A reload replaces node handles, but preserves the modal's original focus history.
pub(super) struct SavedModal {
    node: Address,
    restore: Vec<Address>,
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
            .map(|m| SavedModal {
                node: k.address(m.node),
                restore: m.restore.iter().copied().filter(|&n| k.is_alive(n)).map(|n| k.address(n)).collect(),
            })
            .collect()
    }

    /// The open modal input is confined to.
    pub(crate) fn top_modal(&self) -> Option<NodeId> {
        self.w.kernel.borrow().front_modal()
    }

    /// `hit`, unless an open modal makes it inert.
    pub(crate) fn live(&self, hit: Option<NodeId>) -> Option<NodeId> {
        match (hit, self.top_modal()) {
            (Some(h), Some(m)) if !self.w.kernel.borrow().is_in_modal_scope(h, m) => None,
            _ => hit,
        }
    }

    /// Whether a modal blocks the press, and the one layer eligible to dismiss for it.
    pub(super) fn outside_dismissal(&self, hit: Option<NodeId>) -> (bool, Option<NodeId>) {
        let modal = self.top_modal();
        let k = self.w.kernel.borrow();
        let dismiss = k
            .dismissal_layer(modal)
            .filter(|&layer| k.dismiss_on_outside(layer) && hit.is_none_or(|node| !k.is_in_dismissal_scope(node, layer)));
        let blocked = modal.is_some_and(|modal| hit.is_none_or(|node| !k.is_in_modal_scope(node, modal)));
        (blocked, dismiss)
    }

    /// Updates focus restoration history in the same order as the visible modal layers.
    pub(crate) fn sync_modals(&self) -> bool {
        let shown = self.w.kernel.borrow().modals();
        let (previous, saved, focused) = {
            let mut i = self.w.interaction.borrow_mut();
            if i.reload_modals.is_empty() && i.modals.iter().map(|modal| modal.node).eq(shown.iter().copied()) {
                return false;
            }
            (std::mem::take(&mut i.modals), std::mem::take(&mut i.reload_modals), i.focused)
        };
        let reloaded = !saved.is_empty();
        let history: Vec<(Option<NodeId>, Vec<NodeId>)> = if reloaded {
            let k = self.w.kernel.borrow();
            saved
                .into_iter()
                .map(|m| (k.find_address(&m.node), m.restore.iter().filter_map(|address| k.find_address(address)).collect()))
                .collect()
        } else {
            previous.into_iter().map(|m| (Some(m.node), m.restore)).collect()
        };
        let old_top = history.last().and_then(|(node, _)| *node);
        let new_top = shown.last().copied();
        let previous_openers = history.last().map_or(&[][..], |(_, restore)| restore.as_slice());
        let current = shown
            .iter()
            .map(|&node| {
                let restore = history.iter().find(|(old, _)| *old == Some(node)).map(|(_, restore)| restore.clone()).unwrap_or_else(|| {
                    // A lower layer opening behind the active modal inherits its outside
                    // openers. A new front layer can return to the previously focused member.
                    let opener = focused.filter(|_| Some(node) == new_top || history.is_empty());
                    opener.into_iter().chain(previous_openers.iter().copied()).collect()
                });
                OpenModal { node, restore }
            })
            .collect();
        self.w.interaction.borrow_mut().modals = current;
        if old_top != new_top || reloaded {
            let k = self.w.kernel.borrow();
            let eligible = |node| k.check_activation(node, new_top).is_ok() && k.is_focusable(node);
            if focused.is_none_or(|node| !eligible(node)) {
                let restore = history
                    .iter()
                    .rev()
                    .take_while(|(node, _)| node.is_none_or(|node| !shown.contains(&node)))
                    .flat_map(|(_, restore)| restore.iter().copied())
                    .find(|&node| eligible(node));
                let target = restore.or_else(|| new_top.and_then(|modal| k.initial_focus(modal)));
                drop(k);
                let _ = self.set_focus(target);
            }
        }
        if let Some(modal) = self.top_modal().filter(|&modal| Some(modal) != old_top) {
            self.restrict_pointer_to_modal(modal);
        }
        true
    }

    /// Validate focus after layout and restore it if a panel containing it went away.
    pub(super) fn reconcile_focus(&self) -> bool {
        let (focused, retired_layers) = {
            let mut i = self.w.interaction.borrow_mut();
            (i.focused, std::mem::take(&mut i.retired_focus_layers))
        };
        if focused.is_none() && retired_layers.is_empty() {
            return false;
        }
        let modal = self.top_modal();
        let k = self.w.kernel.borrow();
        let eligible = |node| k.check_activation(node, modal).is_ok() && k.is_focusable(node);
        if focused.is_some_and(eligible) {
            return false;
        }
        let layers = focused.map_or(retired_layers, |node| k.anchored_focus_layers(node));
        let restore = layers.into_iter().find_map(|(panel, owner)| (!k.is_shown(panel) && eligible(owner)).then_some(owner));
        if restore.is_none() && focused.is_none() {
            return false;
        }
        let target = restore.or_else(|| modal.and_then(|m| k.initial_focus(m)));
        drop(k);
        let _ = self.set_focus(target);
        true
    }

    /// Tab (`forward`) or Shift+Tab: the next stop in the Tab order of the open modal (else the
    /// window), wrapping.
    pub(super) fn tab_target(&self, forward: bool) -> Option<NodeId> {
        let focused = self.w.interaction.borrow().focused;
        let order = self.w.kernel.borrow().focus_order(self.top_modal(), focused);
        if order.is_empty() {
            return None;
        }
        let at = focused.and_then(|f| order.iter().position(|n| *n == f));
        let next = match (at, forward) {
            (None, true) => 0,
            (None, false) => order.len() - 1,
            (Some(i), true) => (i + 1) % order.len(),
            (Some(i), false) => (i + order.len() - 1) % order.len(),
        };
        Some(order[next])
    }

    /// Moves focus, queuing optional activation after focus feedback has settled.
    pub(super) fn move_command_focus(&self, n: NodeId, activate_in: Option<NodeId>) -> bool {
        self.set_modality(Modality::Keyboard);
        if self.set_focus(Some(n)).is_err() {
            return false;
        }
        if let Some(group) = activate_in {
            self.w.interaction.borrow_mut().after_update.push_front(super::frame::AfterUpdate::FocusActivation { node: n, group });
        }
        true
    }

    pub(super) fn activate_focused_member(&self, node: NodeId, group: NodeId) -> bool {
        // Focus callbacks can replace the item, redirect focus or open a modal.
        let eligible = {
            let k = self.w.kernel.borrow();
            self.w.interaction.borrow().focused == Some(node)
                && k.check_activation(node, self.top_modal()).is_ok()
                && k.is_focusable(node)
                && k.activating_focus_group(node) == Some(group)
        };
        eligible && self.run(node, &[HandlerKind::Press], None)
    }
}
