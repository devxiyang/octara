//! Mounts module-owned DOM content in the committed element of each typed slot.

use crate::dom::DomHandle;
use octara::module::{HostSurface, HostSurfaceKind, ModuleDispatcher, NativeViewMounts};
use octara::runtime::{Input, WindowId, cycle::WINDOW_ROUNDS};
use octara::{App, AppError};
use octara_model::geometry::Rect;
use octara_model::identity::NativeId;
use octara_model::input::HostReply;
use octara_model::native::{NativeChange, NativePlacement, TypedNativeCommand};
use std::collections::BTreeMap;
use web_sys::{DomRect, HtmlElement};

#[derive(Default)]
pub(crate) struct NativeViews {
    mounts: NativeViewMounts,
    revision: Option<u64>,
    views: BTreeMap<NativeId, ViewState>,
}

struct ViewState {
    props_rev: u64,
    placement: NativePlacement,
}

impl NativeViews {
    pub(crate) fn sync(
        &mut self,
        app: &mut App,
        window: WindowId,
        dom: &DomHandle,
        mount: &HtmlElement,
        dispatcher: &ModuleDispatcher,
    ) -> Result<(), AppError> {
        for _ in 0..WINDOW_ROUNDS {
            let replies = self.sync_frame(app, window, dom, mount, dispatcher);
            if replies.is_empty() {
                return Ok(());
            }
            for reply in replies {
                if app.runtime().window(window).is_none() {
                    return Ok(());
                }
                app.dispatch(window, Input::Host(reply))?;
            }
        }
        Err(AppError::LayoutLoop)
    }

    fn sync_frame(
        &mut self,
        app: &mut App,
        window: WindowId,
        dom: &DomHandle,
        mount: &HtmlElement,
        dispatcher: &ModuleDispatcher,
    ) -> Vec<HostReply> {
        let Some(runtime) = app.runtime().window(window).filter(|runtime| runtime.settled().is_some()) else { return Vec::new() };
        let revision = runtime.observation_revision();
        if self.revision == Some(revision) {
            return Vec::new();
        }
        self.revision = Some(revision);
        let tree = runtime.tree();
        let viewport = runtime.options().viewport;
        let mut origin = None;
        let mut views = BTreeMap::new();
        let mut changes = Vec::new();
        for node in tree.native_nodes() {
            let native = tree.native(node).expect("indexed native slot");
            let Some(props) = &native.typed_props else { continue };
            let Some(element) = dom.element(node) else { continue };
            let id = NativeId::from(node);
            let previous = self.views.get(&id);
            if previous.is_none_or(|view| view.props_rev != native.props_rev) {
                let change = if self.mounts.contains(id) {
                    NativeChange::UpdateTyped { id, props: props.clone() }
                } else {
                    NativeChange::CreateTyped { id, kind: native.kind.to_owned(), slot: native.slot, props: props.clone() }
                };
                changes.push((id, change, element.clone()));
            }

            // CSS applies ancestor clipping and stacking to the slot's DOM children.
            let origin = origin.get_or_insert_with(|| mount.get_bounding_client_rect());
            let rect = bounds(&element, origin);
            let visual = tree.visual(node);
            let clip = viewport.visible_bounds();
            let placement = NativePlacement {
                id,
                slot: native.slot,
                rect,
                radius: rect.corner_radii(visual.radius),
                clip,
                clip_radius: 0.0,
                opacity: visual.opacity,
                visible: tree.is_shown(node) && !rect.intersect(&clip).is_empty() && visual.opacity > 0.0,
                covered: Vec::new(),
            };
            views.insert(id, ViewState { props_rev: native.props_rev, placement });
        }

        let _guard = dom.project_guard();
        let runtime = app.runtime_mut();
        let surface = HostSurface::new(HostSurfaceKind::Web, Some(window));
        for &id in self.views.keys().filter(|&id| !views.contains_key(id)) {
            let _ = self.mounts.apply_change(&NativeChange::Destroy { id }, surface, runtime.native_views_mut(), dispatcher);
        }
        changes.sort_unstable_by_key(|(id, _, _)| *id);
        let mut mounted = Vec::new();
        for (id, change, element) in changes {
            let was_mounted = self.mounts.contains(id);
            match self.mounts.apply_change(&change, surface.with_opaque(&element), runtime.native_views_mut(), dispatcher) {
                Ok(true) if !was_mounted => mounted.push(id),
                Err(error) => runtime.report_native_view_mount_error(&error),
                _ => {}
            }
        }
        let placements = views
            .iter()
            .filter(|&(id, view)| {
                self.views.get(id).is_none_or(|previous| previous.placement != view.placement) || mounted.binary_search(id).is_ok()
            })
            .map(|(_, view)| view.placement.clone())
            .collect::<Vec<_>>();
        if !placements.is_empty() {
            let _ = self.mounts.apply_change(&NativeChange::Place { placements }, surface, runtime.native_views_mut(), dispatcher);
        }
        self.views = views;
        let tree = runtime.window(window).expect("attached browser window").tree();
        self.views
            .keys()
            .filter_map(|&id| {
                let node = id.node();
                let live = self.mounts.contains(id);
                (live != tree.native(node)?.live).then(|| HostReply::NativeLive { target: tree.guard(node), live })
            })
            .collect()
    }

    pub(crate) fn command(&mut self, command: TypedNativeCommand, dispatcher: &ModuleDispatcher) {
        self.mounts.command(command, dispatcher);
    }

    pub(crate) fn clear(&mut self) {
        self.mounts.clear();
        self.views.clear();
        self.revision = None;
    }
}

fn bounds(element: &HtmlElement, origin: &DomRect) -> Rect {
    let rect = element.get_bounding_client_rect();
    Rect::new((rect.x() - origin.x()) as f32, (rect.y() - origin.y()) as f32, rect.width() as f32, rect.height() as f32)
}
