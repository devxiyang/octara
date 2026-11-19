//! Mounted virtual regions shared by document selection and keyed reveal.

use crate::identity::{RowKey, RowNamespace};
use crate::kernel::NodeId;
use crate::reactive::graph::{ExecutionOwner, Signal};
use crate::reactive::seq::Seq;
use std::any::TypeId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Builds a row off screen: its root node, and what disposes it.
type BuildRow = Rc<dyn Fn(&RowKey) -> Option<(NodeId, Box<dyn FnOnce()>)>>;

pub(crate) struct RowReveal {
    pub key: RowKey,
    pub owner: ExecutionOwner,
    pub active: bool,
}

pub(crate) struct ListOrder {
    pub region: NodeId,
    pub key_type: TypeId,
    pub namespace: Rc<RowNamespace>,
    pub keys: Rc<RefCell<Seq<RowKey>>>,
    pub build: BuildRow,
    pub reveal: Rc<RefCell<Option<RowReveal>>>,
    pub revision: Signal<u64>,
}

#[derive(Default)]
pub(crate) struct VirtualLists {
    by_viewport: HashMap<NodeId, Vec<ListOrder>>,
    pending_reveal: bool,
}

impl VirtualLists {
    pub fn is_empty(&self) -> bool {
        self.by_viewport.is_empty()
    }

    pub fn register(&mut self, viewport: NodeId, list: ListOrder) {
        self.by_viewport.entry(viewport).or_default().push(list);
    }

    pub fn remove(&mut self, viewport: NodeId, region: NodeId) -> Option<ListOrder> {
        let lists = self.by_viewport.get_mut(&viewport)?;
        let index = lists.iter().position(|list| list.region == region)?;
        let retired = lists.remove(index);
        if lists.is_empty() {
            self.by_viewport.remove(&viewport);
        }
        Some(retired)
    }

    pub fn unique(&self, viewport: NodeId) -> Option<&ListOrder> {
        match self.by_viewport.get(&viewport)?.as_slice() {
            [list] => Some(list),
            _ => None,
        }
    }

    pub fn request_reveal(&mut self, viewport: NodeId, region: NodeId, key: RowKey, owner: ExecutionOwner) -> bool {
        let Some(list) = self.unique(viewport).filter(|list| list.region == region) else { return false };
        list.reveal.replace(Some(RowReveal { key, owner, active: false }));
        self.pending_reveal = true;
        true
    }

    /// Activate requests only after the current source and layout settle. This gives the
    /// virtual region a separate measurement pass before the window resolves requested focus.
    pub fn activate_reveals(&mut self) -> Vec<Signal<u64>> {
        if !std::mem::take(&mut self.pending_reveal) {
            return Vec::new();
        }
        let mut revisions = Vec::new();
        for lists in self.by_viewport.values() {
            let [list] = lists.as_slice() else {
                for list in lists {
                    list.reveal.borrow_mut().take();
                }
                continue;
            };
            if let Some(request) = list.reveal.borrow_mut().as_mut().filter(|request| !request.active) {
                request.active = true;
                revisions.push(list.revision);
            }
        }
        revisions
    }

    pub fn clear_reveals(&mut self) {
        self.pending_reveal = false;
        for list in self.by_viewport.values().flatten() {
            list.reveal.borrow_mut().take();
        }
    }

    // Selection has one logical order per scroll container.
    pub fn get(&self, viewport: &NodeId) -> Option<&ListOrder> {
        self.by_viewport.get(viewport)?.last()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&NodeId, &ListOrder)> {
        self.by_viewport.iter().filter_map(|(viewport, lists)| Some((viewport, lists.last()?)))
    }
}
