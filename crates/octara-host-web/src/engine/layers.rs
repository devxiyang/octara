//! Native inertness follows logical scope across physically separate portal roots.

use super::{DomEngine, Pending};
use js_sys::Set;
use octara::tree::{NodeId, Tree};
use octara_model::overlay::FocusTrapFeatures;
use rustc_hash::FxHashSet;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{Element, HtmlElement, MutationObserver, MutationObserverInit};

#[derive(Default)]
pub(super) struct ModalScope {
    layers: Vec<(NodeId, bool)>,
    blocked: Vec<NodeId>,
    outside_bridges: Vec<NodeId>,
}

impl ModalScope {
    pub(super) fn new(tree: &Tree, modal: Option<NodeId>) -> Self {
        let Some(modal) = modal else { return Self::default() };
        let mut scope = Self {
            layers: tree
                .modal_layers()
                .filter_map(|node| {
                    let policy = tree.overlay(node)?.layer?;
                    Some((node, policy.focus_trap.contains(FocusTrapFeatures::RESTORE_FOCUS)))
                })
                .collect(),
            ..Self::default()
        };
        let inside = |node| tree.is_in_modal_scope(node, modal);
        // Crossings can enter the modal scope through an owned portal or leave
        // it through a portal physically nested inside an in-scope element.
        let bridges: FxHashSet<_> = tree
            .overlay_nodes()
            .flat_map(|node| [Some(node), tree.overlay(node).and_then(|slot| slot.mount.map(|mount| mount.owner))])
            .flatten()
            .chain([modal])
            .filter(|&node| tree.physical_parent(node).is_some_and(|parent| inside(parent) != inside(node)))
            .flat_map(|node| std::iter::successors(tree.physical_parent(node), |&id| tree.physical_parent(id)))
            .collect();
        let mut branches = vec![tree.root()];
        while let Some(node) = branches.pop() {
            if !tree.is_live(node) {
                continue;
            }
            if bridges.contains(&node) {
                if !inside(node) {
                    scope.outside_bridges.push(node);
                }
                branches.extend(tree.children(node));
            } else if !inside(node) {
                scope.blocked.push(node);
            }
        }
        scope
    }
}

struct Layer {
    node: NodeId,
    restore: bool,
    opener: Option<HtmlElement>,
    previous_focus: Option<NodeId>,
}

pub(super) struct LayerDom {
    scope: ModalScope,
    layers: Vec<Layer>,
    // Record only attributes installed by this engine. An existing inert
    // attribute belongs to the embedding page or the exiting element.
    owned_inert: Set<Element>,
    owned_raw_inert: Set<Element>,
    raw_observer: Option<RawObserver>,
}

impl Default for LayerDom {
    fn default() -> Self {
        Self {
            scope: ModalScope::default(),
            layers: Vec::new(),
            owned_inert: Set::new_empty(),
            owned_raw_inert: Set::new_empty(),
            raw_observer: None,
        }
    }
}

struct RawObserver {
    observer: MutationObserver,
    _callback: Closure<dyn FnMut()>,
}

impl Drop for RawObserver {
    fn drop(&mut self) {
        self.observer.disconnect();
    }
}

impl DomEngine {
    pub(super) fn publish_layer_dom(&mut self, pending: &mut Pending) -> Vec<HtmlElement> {
        let mut openers = Vec::new();
        if let Some(scope) = pending.modal_scope.take() {
            let front_changed = self.modal != pending.modal;
            let active = pending.active_before.as_ref();
            let raw_opener = active
                .filter(|&active| self.dom.node(active).is_none() && !matches!(active.local_name().as_str(), "body" | "html"))
                .and_then(|active| active.dyn_ref::<HtmlElement>());
            let mut previous = std::mem::take(&mut self.layer_dom.layers);
            let new_front = pending.modal.is_some_and(|modal| !previous.iter().any(|layer| layer.node == modal));
            for &(node, restore) in &scope.layers {
                let mut layer = previous.iter().position(|layer| layer.node == node).map_or_else(
                    || Layer { node, restore, opener: raw_opener.cloned(), previous_focus: self.focus },
                    |index| previous.remove(index),
                );
                layer.restore = restore;
                self.layer_dom.layers.push(layer);
            }
            if front_changed && !new_front && pending.focused_in_modal {
                openers.extend(
                    previous
                        .into_iter()
                        .rev()
                        .filter(|layer| layer.restore && (pending.focus.is_none() || pending.focus == layer.previous_focus))
                        .filter_map(|layer| layer.opener)
                        .filter(|element| element.is_connected()),
                );
            }
            self.layer_dom.scope = scope;
            if let Err(error) = self.watch_unmanaged_branches(pending.modal.is_some()) {
                web_sys::console::error_1(&error);
            }
            pending.inert_dirty = true;
        }
        if pending.inert_dirty || !pending.retired.is_empty() {
            self.sync_inert(pending);
        }
        openers
    }

    fn sync_inert(&mut self, pending: &Pending) {
        let scope = &self.layer_dom.scope;
        let desired = Set::<Element>::new_empty();
        for &node in scope.blocked.iter().chain(&self.authored_inert).chain(self.exits.keys()).chain(pending.exits.keys()) {
            if let Some(element) = self.dom.element(node) {
                desired.add(&element);
            }
        }
        sync_inert(&self.layer_dom.owned_inert, &desired);
    }

    fn watch_unmanaged_branches(&mut self, modal: bool) -> Result<(), JsValue> {
        self.layer_dom.raw_observer.take();
        let owned = self.layer_dom.owned_raw_inert.clone();
        if !modal {
            sync_inert(&owned, &Set::new_empty());
            return Ok(());
        }

        let bridges: Vec<_> = std::iter::once(self.surface.clone())
            .chain(self.layer_dom.scope.outside_bridges.iter().filter_map(|&node| self.dom.element(node)))
            .collect();
        let scan = bridges.clone();
        let dom = self.dom.clone();
        let parking = self.parking.clone();
        let update = move || {
            let desired = Set::new_empty();
            for bridge in &scan {
                for child in std::iter::successors(bridge.first_element_child(), Element::next_element_sibling) {
                    if dom.node(&child).is_none() && child != *parking {
                        desired.add(&child);
                    }
                }
            }
            sync_inert(&owned, &desired);
        };
        update();
        let callback = Closure::<dyn FnMut()>::new(update);
        let observer = MutationObserver::new(callback.as_ref().unchecked_ref())?;
        let options = MutationObserverInit::new();
        options.set_child_list(true);
        let watch = RawObserver { observer, _callback: callback };
        for bridge in bridges {
            watch.observer.observe_with_options(&bridge, &options)?;
        }
        self.layer_dom.raw_observer = Some(watch);
        Ok(())
    }
}

fn sync_inert(owned: &Set<Element>, desired: &Set<Element>) {
    desired.for_each(&mut |element, _, _| {
        if !element.has_attribute("inert") {
            if let Err(error) = element.set_attribute("inert", "") {
                web_sys::console::error_1(&error);
            } else {
                owned.add(&element);
            }
        }
    });
    owned.for_each(&mut |element, _, _| {
        if !desired.has(&element) {
            owned.delete(&element);
            clear_inert(&element);
        }
    });
}

fn clear_inert(element: &Element) {
    if element.get_attribute("inert").as_deref() == Some("")
        && let Err(error) = element.remove_attribute("inert")
    {
        web_sys::console::error_1(&error);
    }
}

impl Drop for LayerDom {
    fn drop(&mut self) {
        self.raw_observer.take();
        self.owned_inert.for_each(&mut |element, _, _| clear_inert(&element));
        self.owned_raw_inert.for_each(&mut |element, _, _| clear_inert(&element));
    }
}
