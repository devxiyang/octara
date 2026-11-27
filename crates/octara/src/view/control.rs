//! View regions whose factories run in explicit child scopes.

use super::style::{Styled, rel};
use super::{Component, El, IntoView, Kind, NodeHandle, Prop, View, el, el::build_el, node_handle};
use octara_model::{CacheLine, geometry::Placement, overlay::AnchorTarget, style::Position};
use octara_runtime::{BuildCx, MountCx, ReadCx, ScopeId};
use octara_tree::{NodeChange, NodeRef, Retirement, transaction::OverlaySpec};
use std::{
    fmt::Display,
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) fn next_region() -> u64 {
    static NEXT: CacheLine<AtomicU64> = CacheLine(AtomicU64::new(1));
    NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1)).expect("region identifier exhausted")
}

/// Builds one child scope with its own state and cleanup lifetime.
#[track_caller]
pub fn scope<V: IntoView + 'static>(key: impl Display, factory: impl FnOnce(&mut BuildCx<'_>) -> V + 'static) -> View {
    let key = key.to_string();
    View::region(move |cx, target| {
        cx.with_named_scope(&key, |cx| {
            let view = cx.build(factory).into_view();
            target.mount(cx, view);
        });
    })
}

/// Builds one anonymous child scope with its own state and cleanup lifetime.
#[track_caller]
pub fn scoped<V: IntoView + 'static>(factory: impl FnOnce(&mut BuildCx<'_>) -> V + 'static) -> View {
    View::region(move |cx, target| {
        cx.with_scope(|cx| {
            let view = cx.build(factory).into_view();
            target.mount(cx, view);
        });
    })
}

/// Mounts the content while the tracked predicate is true and honors visible exit motion.
/// Each activation owns fresh state.
#[track_caller]
pub fn show<V: IntoView + 'static>(
    condition: impl FnMut(&mut ReadCx<'_>) -> bool + 'static,
    mut factory: impl FnMut(&mut BuildCx<'_>) -> V + 'static,
) -> View {
    let mut condition = condition;
    switch(move |r| condition(r).then_some(()), |_| None, move |cx, _| factory(cx), Retirement::VisibleExit)
}

/// Retains a branch while its key is unchanged and creates a fresh child scope on a key change.
#[track_caller]
pub fn keyed<K: Display + PartialEq + 'static, V: IntoView + 'static>(
    mut key: impl FnMut(&mut ReadCx<'_>) -> K + 'static,
    factory: impl FnMut(&mut BuildCx<'_>, &K) -> V + 'static,
) -> View {
    switch(move |r| Some(key(r)), |key| Some(key.to_string()), factory, Retirement::Destroy)
}

/// Retains the fallback scope while successive keys produce no view.
#[doc(hidden)]
pub fn keyed_or_else<K: Display + PartialEq + 'static, V: IntoView + 'static>(
    key: impl FnMut(&mut ReadCx<'_>) -> K + 'static,
    mut factory: impl FnMut(&mut BuildCx<'_>, &K) -> Option<V> + 'static,
    mut fallback: impl FnMut(&mut BuildCx<'_>) -> V + 'static,
) -> View {
    View::region(move |cx, target| {
        let mut active: Option<(ScopeId, bool)> = None;
        cx.with_scope(|cx| {
            cx.structure(key, move |cx, key| {
                let retire = |cx: &mut MountCx<'_>, scope| {
                    target.remove(cx, Retirement::Destroy);
                    cx.retire_scope(scope);
                };
                let (candidate, mounted) = cx.with_named_scope(&key.to_string(), |cx| {
                    let Some(view) = cx.build(|b| factory(b, key)) else { return false };
                    if let Some((scope, _)) = active.take() {
                        retire(cx, scope);
                    }
                    target.mount(cx, view.into_view());
                    true
                });
                if mounted {
                    active = Some((candidate, false));
                    return;
                }
                cx.retire_scope(candidate);
                if active.is_some_and(|(_, is_fallback)| is_fallback) {
                    return;
                }
                if let Some((scope, _)) = active.take() {
                    retire(cx, scope);
                }
                let (scope, ()) = cx.with_scope(|cx| {
                    let view = cx.build(&mut fallback);
                    target.mount(cx, view.into_view());
                });
                active = Some((scope, true));
            });
        });
    })
}

fn switch<K: PartialEq + 'static, V: IntoView + 'static>(
    key: impl FnMut(&mut ReadCx<'_>) -> Option<K> + 'static,
    name: impl Fn(&K) -> Option<String> + 'static,
    mut factory: impl FnMut(&mut BuildCx<'_>, &K) -> V + 'static,
    retirement: Retirement,
) -> View {
    View::region(move |cx, target| {
        let mut active = None;
        let mut exiting: Vec<NodeHandle> = Vec::new();
        cx.with_scope(|cx| {
            cx.structure(key, move |cx, key| {
                if let Some(previous) = active.take() {
                    exiting = target.remove(cx, retirement);
                    cx.retire_scope(previous);
                }
                if let Some(key) = key {
                    for node in exiting.drain(..) {
                        cx.cancel_exit(node.get());
                    }
                    let build = |cx: &mut MountCx<'_>| {
                        let view = cx.build(|cx| factory(cx, key)).into_view();
                        target.mount(cx, view);
                    };
                    let (scope, ()) = match name(key) {
                        Some(name) => cx.with_named_scope(&name, build),
                        None => cx.with_scope(build),
                    };
                    active = Some(scope);
                }
            })
        });
    })
}

type Open = Box<dyn FnMut(&mut ReadCx<'_>) -> bool>;
type Content = Box<dyn FnMut(&mut BuildCx<'_>) -> View>;

type TargetBinding = Box<dyn FnMut(&mut ReadCx<'_>, OverlaySpec) -> NodeChange>;

pub(crate) struct OverlayBinding {
    pub spec: OverlaySpec,
    pub target: Option<TargetBinding>,
}

pub(crate) struct Portal {
    placement: Option<Placement>,
    target: Option<Prop<Option<AnchorTarget>>>,
    direct: bool,
    open: Open,
    content: Content,
}

impl Portal {
    pub(crate) fn new<V: IntoView + 'static>(
        placement: Option<Placement>,
        open: impl FnMut(&mut ReadCx<'_>) -> bool + 'static,
        mut content: impl FnMut(&mut BuildCx<'_>) -> V + 'static,
    ) -> Self {
        Self { placement, target: None, direct: false, open: Box::new(open), content: Box::new(move |cx| content(cx).into_view()) }
    }

    pub(crate) fn part(
        placement: Option<Placement>,
        target: Option<Prop<Option<AnchorTarget>>>,
        open: impl FnMut(&mut ReadCx<'_>) -> bool + 'static,
        content: impl FnMut(&mut BuildCx<'_>) -> El + 'static,
    ) -> Self {
        Self { target: target.filter(|target| !matches!(target, Prop::Static(None))), direct: true, ..Self::new(placement, open, content) }
    }

    pub(crate) fn mount(self, cx: &mut MountCx<'_>, owner: NodeHandle, component: Option<Component>) {
        let Self { placement, target, direct, open, mut content } = self;
        let target = target.map(|mut target| {
            cx.build(|b| {
                b.memo(move |read| match &mut target {
                    Prop::Static(value) => *value,
                    Prop::Dyn(value) => value(read),
                })
            })
        });
        let mut active: Option<(_, NodeHandle)> = None;
        let order = next_region();
        cx.structure(open, move |cx, &open| {
            if let Some((scope, node)) = active.take() {
                cx.retire(node.get(), Retirement::VisibleExit);
                cx.retire_scope(scope);
            }
            if open {
                active = Some(cx.with_scope(|cx| {
                    let view = cx.build(&mut content);
                    let overlay = OverlaySpec {
                        target: placement.map(|placement| (AnchorTarget::Node(owner.get()), placement)),
                        mount: Some((owner.get(), order)),
                        ..Default::default()
                    };
                    let (element, component) = if direct {
                        let Kind::Element(element) = view.kind else { unreachable!("a portal part returns an element") };
                        (element, view.component.or(component))
                    } else {
                        let mut element = el().child(view);
                        if placement.is_none() {
                            element = element.position(Position::Absolute).inset(0).size(rel(1.0));
                        }
                        (element, component)
                    };
                    let overlay = OverlayBinding {
                        spec: overlay,
                        target: target.map(|target| {
                            let owner = owner.clone();
                            let tree = cx.tree_id();
                            Box::new(move |read: &mut ReadCx<'_>, mut spec: OverlaySpec| {
                                let (_, placement) = spec.target.expect("target bindings have a placement");
                                let target = read.get(target).map_or_else(
                                    || AnchorTarget::Node(owner.get()),
                                    |target| target.map_node(|id| NodeRef::Existing { tree, id }),
                                );
                                spec.target = Some((target, placement));
                                spec.mount = Some((owner.get(), order));
                                NodeChange::Overlay(spec)
                            }) as TargetBinding
                        }),
                    };
                    let node = build_el(cx, element, component, None, Some(overlay));
                    cx.place(cx.overlay_root(), usize::MAX, node);
                    node_handle(cx, node)
                }));
            }
        });
    }
}
