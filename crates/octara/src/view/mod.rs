//! Composable views that stream retained nodes into the current mount transaction.

mod behavior;
mod control;
mod el;
mod list;
mod navigation;
mod pace;
mod prop;
mod style;
#[cfg(test)]
mod tests;
mod text;

pub use behavior::Behavior;
pub use control::{keyed, keyed_or_else, scope, scoped, show};
pub use el::{El, TextInput, canvas, col, el, icon, image, image_source, input, native_view, row, spacer, text, textarea, video};
pub use list::{VirtualOptions, each, virtual_each, virtual_each_with_copy};
pub use navigation::{RouteMount, route_stack};
pub use octara_macros::{Store, tw, ui};
pub use octara_model::semantics::NumericRange;
pub use octara_runtime::window::CopiedText;
pub use octara_tree::{
    focus::FocusGroup,
    tables::{Component, HandlerKind},
};
pub use pace::paced;
pub use prop::{IntoProp, Prop, bind};
pub use style::{FontSize, Length, LineHeight, Radius, Ring, Shadows, State, StylePatch, Styled, TwStyle, When, px, rel, sp};
pub use text::{IntoText, Rich, Span, rich};

use octara_runtime::{MountCx, ReadCx, ScopeId};
use octara_tree::{NodeRef, Retirement, reconcile::stationary_indices};
use rustc_hash::FxHashMap;
use smallvec::{SmallVec, smallvec};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

/// A single-use tree description or structural region.
#[must_use]
pub struct View {
    kind: Kind,
    component: Option<Component>,
}

type MountRegion = Box<dyn FnOnce(&mut MountCx<'_>, Target)>;

enum Kind {
    Element(El),
    Fragment(Vec<View>),
    Region(MountRegion),
}

impl View {
    pub fn empty() -> Self {
        Self::fragment(std::iter::empty::<Self>())
    }

    pub fn fragment(views: impl IntoIterator<Item = impl IntoView>) -> Self {
        Self { kind: Kind::Fragment(views.into_iter().map(IntoView::into_view).collect()), component: None }
    }

    /// Mounts this view into the current window transaction.
    pub fn mount(self, cx: &mut MountCx<'_>, parent: impl Into<NodeRef>, index: usize) {
        mount_children(cx, parent.into(), index, smallvec![self], None);
    }

    pub(crate) fn element(element: El) -> Self {
        Self { kind: Kind::Element(element), component: None }
    }

    fn region(mount: impl FnOnce(&mut MountCx<'_>, Target) + 'static) -> Self {
        Self { kind: Kind::Region(Box::new(mount)), component: None }
    }

    fn has_region(&self) -> bool {
        match &self.kind {
            Kind::Region(_) => true,
            Kind::Fragment(views) => views.iter().any(Self::has_region),
            Kind::Element(_) => false,
        }
    }
}

/// A value that can be authored directly as a child or converted into a view.
pub trait IntoView: Sized {
    fn into_view(self) -> View;

    /// Attaches provenance at an ordinary Rust component boundary.
    #[doc(hidden)]
    fn with_component(self, _component: Component) -> Self {
        self
    }
}

impl IntoView for View {
    fn into_view(self) -> View {
        self
    }

    fn with_component(mut self, component: Component) -> Self {
        self.component.get_or_insert(component);
        self
    }
}

impl IntoView for &str {
    #[track_caller]
    fn into_view(self) -> View {
        text(self).into_view()
    }
}

impl IntoView for String {
    #[track_caller]
    fn into_view(self) -> View {
        text(self).into_view()
    }
}

impl<T: Into<Rich> + 'static> IntoView for Prop<T> {
    #[track_caller]
    fn into_view(self) -> View {
        text(self).into_view()
    }
}

impl<T: Into<Rich>, F: FnMut(&mut ReadCx<'_>) -> T + 'static> IntoView for F {
    #[track_caller]
    fn into_view(self) -> View {
        text(self).into_view()
    }
}

impl<V: IntoView> IntoView for Option<V> {
    fn into_view(self) -> View {
        self.map_or_else(View::empty, IntoView::into_view)
    }

    fn with_component(self, component: Component) -> Self {
        self.map(|view| view.with_component(component))
    }
}

impl<T: IntoView> IntoView for Vec<T> {
    fn into_view(self) -> View {
        View::fragment(self)
    }

    fn with_component(mut self, component: Component) -> Self {
        self = self.into_iter().map(|view| view.with_component(component)).collect();
        self
    }
}

impl IntoView for () {
    fn into_view(self) -> View {
        View::empty()
    }
}

pub(crate) fn mount_children(
    cx: &mut MountCx<'_>,
    parent: NodeRef,
    index: usize,
    children: SmallVec<[View; 4]>,
    component: Option<Component>,
) {
    if children.iter().any(View::has_region) {
        let siblings = Rc::new(Siblings {
            parent: node_handle(cx, parent),
            owner: cx.scope(),
            index,
            root: SlotList::default(),
            order: RefCell::default(),
            scheduled: Cell::default(),
        });
        let target = Target { slots: siblings.root.clone(), siblings, component };
        for view in children {
            target.mount(cx, view);
        }
    } else {
        let mut index = index;
        for view in children {
            mount_static(cx, parent, &mut index, view, component);
        }
    }
}

fn mount_static(cx: &mut MountCx<'_>, parent: NodeRef, index: &mut usize, view: View, component: Option<Component>) {
    let component = view.component.or(component);
    match view.kind {
        Kind::Element(element) => {
            let node = el::build_el(cx, element, component, None, None);
            cx.place(parent, *index, node);
            *index += 1;
        }
        Kind::Fragment(views) => {
            for view in views {
                mount_static(cx, parent, index, view, component);
            }
        }
        Kind::Region(_) => unreachable!("dynamic regions require ordered slots"),
    }
}

type NodeHandle = Rc<Cell<NodeRef>>;

fn node_handle(cx: &mut MountCx<'_>, node: NodeRef) -> NodeHandle {
    let handle = Rc::new(Cell::new(node));
    if let NodeRef::Pending(pending) = node {
        let handle = handle.clone();
        let tree = cx.tree_id();
        cx.after_commit(move |receipt| {
            if let Some(id) = receipt.node(pending) {
                handle.set(NodeRef::Existing { tree, id });
            }
        });
    }
    handle
}

type SlotList = Rc<RefCell<Vec<Slot>>>;

enum Slot {
    Node(NodeHandle),
    Group(SlotList),
}

fn flatten(slots: &[Slot], nodes: &mut Vec<NodeHandle>) {
    for slot in slots {
        match slot {
            Slot::Node(node) => nodes.push(node.clone()),
            Slot::Group(children) => flatten(&children.borrow(), nodes),
        }
    }
}

struct Siblings {
    parent: NodeHandle,
    owner: ScopeId,
    index: usize,
    root: SlotList,
    order: RefCell<Vec<NodeHandle>>,
    scheduled: Cell<bool>,
}

impl Siblings {
    fn schedule(self: &Rc<Self>, cx: &mut MountCx<'_>) {
        if self.scheduled.replace(true) {
            return;
        }
        let siblings = self.clone();
        cx.before_commit_in(self.owner, move |cx| {
            siblings.scheduled.set(false);
            let mut next = Vec::new();
            flatten(&siblings.root.borrow(), &mut next);
            let mut order = siblings.order.borrow_mut();
            let ranks: FxHashMap<_, _> = order.iter().enumerate().map(|(rank, node)| (node.get(), rank)).collect();
            let stationary = stationary_indices(next.iter().map(|node| ranks.get(&node.get()).copied()));
            // The tree detaches all placed children before inserting them in final rank order.
            for (index, (node, stationary)) in next.iter().zip(stationary).enumerate() {
                if !stationary {
                    cx.place(siblings.parent.get(), siblings.index + index, node.get());
                }
            }
            *order = next;
        });
    }
}

#[derive(Clone)]
struct Target {
    siblings: Rc<Siblings>,
    slots: SlotList,
    component: Option<Component>,
}

impl Target {
    fn mount(&self, cx: &mut MountCx<'_>, view: View) {
        let target = Self { component: view.component.or(self.component), ..self.clone() };
        match view.kind {
            Kind::Element(element) => {
                let node = el::build_el(cx, element, target.component, None, None);
                target.add(cx, node);
            }
            Kind::Fragment(views) => {
                for view in views {
                    target.mount(cx, view);
                }
            }
            Kind::Region(mount) => mount(cx, target.group()),
        }
    }

    fn group(&self) -> Self {
        let group = Self { slots: SlotList::default(), ..self.clone() };
        self.slots.borrow_mut().push(Slot::Group(group.slots.clone()));
        group
    }

    fn add(&self, cx: &mut MountCx<'_>, node: NodeRef) -> NodeHandle {
        let node = node_handle(cx, node);
        self.slots.borrow_mut().push(Slot::Node(node.clone()));
        self.siblings.schedule(cx);
        node
    }

    fn remove(&self, cx: &mut MountCx<'_>, mode: Retirement) -> Vec<NodeHandle> {
        let mut nodes = Vec::new();
        flatten(&self.slots.take(), &mut nodes);
        for node in &nodes {
            cx.retire(node.get(), mode);
        }
        self.siblings.schedule(cx);
        nodes
    }

    fn set_nodes<'a>(&self, cx: &mut MountCx<'_>, nodes: impl IntoIterator<Item = &'a NodeHandle>) {
        self.slots.replace(nodes.into_iter().cloned().map(Slot::Node).collect());
        self.siblings.schedule(cx);
    }
}
