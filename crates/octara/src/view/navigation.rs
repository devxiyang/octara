//! Retained or fresh page scopes for application navigation.

use octara_model::style::Display;
use octara_runtime::{BuildCx, HandlerOutcome, Navigator, ScopeId};
use octara_tree::Retirement;
use rustc_hash::{FxHashMap, FxHashSet};

use super::{IntoView, NodeHandle, Styled, View, col, el::build_el, node_handle};

/// Controls how inactive pages are mounted by [`route_stack`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RouteMount {
    /// Keep the page scope, editor, and scroll state for each visited history entry.
    KeepAlive,
    /// Give each active page a fresh scope on entry.
    #[default]
    Unmount,
}

/// Mounts the active route. Retained pages are hidden with `Display::None`.
///
/// The visible page exposes native Back when the navigator has a previous entry. Leave guards
/// declared inside a page are scoped to that entry, including when its view is kept alive.
#[track_caller]
pub fn route_stack<R: Clone + 'static, V: IntoView + 'static>(
    nav: Navigator<R>,
    mode: RouteMount,
    mut page: impl FnMut(&mut BuildCx<'_>, &R) -> V + 'static,
) -> View {
    View::region(move |cx, target| {
        let mut mounted = FxHashMap::<u64, (ScopeId, NodeHandle)>::default();
        cx.with_scope(|cx| {
            cx.structure(
                move |r| {
                    let active = nav.active_page(r);
                    let keys = match mode {
                        RouteMount::KeepAlive => nav.page_keys(r),
                        RouteMount::Unmount => vec![active],
                    };
                    (active, keys)
                },
                move |cx, (id, keys)| {
                    let retained: FxHashSet<_> = keys.iter().copied().collect();
                    mounted.retain(|id, (scope, node)| {
                        if retained.contains(id) {
                            return true;
                        }
                        cx.retire(node.get(), Retirement::Destroy);
                        cx.retire_scope(*scope);
                        false
                    });
                    let id = *id;
                    mounted.entry(id).or_insert_with(|| {
                        cx.with_named_scope(&format!("route:{id}"), |cx| {
                            let (view, back) = cx.build(|b| {
                                let (route, back) = nav.page(b, id).expect("route entry in history");
                                nav.provide_page(b, id);
                                (page(b, &route), back)
                            });
                            let mut container = col().child(view);
                            if mode == RouteMount::KeepAlive {
                                container = container.style_with(move |r, style| {
                                    style.display(if nav.active_page(r) == id { Display::Flex } else { Display::None })
                                });
                            }
                            let node = build_el(cx, container, target.component, None, None);
                            if back {
                                cx.handle_command(node, "navigation.back", move |w| HandlerOutcome {
                                    consumed: w.read(|r| nav.active_page(r) == id) && nav.pop(w),
                                    ..Default::default()
                                });
                            }
                            node_handle(cx, node)
                        })
                    });
                    target.set_nodes(cx, keys.iter().filter_map(|id| mounted.get(id).map(|(_, node)| node)));
                },
            );
        });
    })
}
