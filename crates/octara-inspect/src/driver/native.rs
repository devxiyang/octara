use octara::{
    App, AppError,
    module::{HostSurface, HostSurfaceKind, NativeViewMounts},
    runtime::{Input, WindowRuntime, cycle::WINDOW_ROUNDS},
};
use octara_model::{
    identity::{NativeId, WindowId},
    input::HostReply,
    native::NativeChange,
};
use octara_pipeline::{NativeEngine, NativeEngineConfig, NativePublication};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

#[derive(Default)]
pub(super) struct NativeHost {
    publications: Rc<RefCell<BTreeMap<WindowId, NativePublication>>>,
    windows: BTreeMap<WindowId, WindowViews>,
}

#[derive(Default)]
struct WindowViews {
    mounts: NativeViewMounts,
    frame: Option<u64>,
}

impl NativeHost {
    pub(super) fn engine_factory(
        &self,
        config: NativeEngineConfig,
    ) -> impl Fn(WindowId, &octara::WindowOptions) -> Box<dyn octara::HostEngine> + 'static + use<> {
        let publications = self.publications.clone();
        move |window, _| {
            let publication = NativePublication::for_window_host();
            publications.borrow_mut().insert(window, publication.clone());
            Box::new(NativeEngine::with_publication(config, publication))
        }
    }

    pub(super) fn sync(&mut self, app: &mut App) -> Result<(), AppError> {
        for _ in 0..WINDOW_ROUNDS {
            if !self.sync_frame(app)? {
                return Ok(());
            }
        }
        Err(AppError::LayoutLoop)
    }

    fn sync_frame(&mut self, app: &mut App) -> Result<bool, AppError> {
        let runtime = app.runtime_mut();
        self.windows.retain(|&id, _| runtime.window(id).is_some());
        let mut publications = self.publications.borrow_mut();
        publications.retain(|&id, _| runtime.window(id).is_some());
        let dispatcher = runtime.module_dispatcher();
        let mut replies = Vec::new();
        let mut called_view = false;
        for (&id, publication) in publications.iter() {
            let Some(frame) = publication.latest() else { continue };
            let views = self.windows.entry(id).or_default();
            if views.frame == Some(frame.number) {
                continue;
            }
            views.frame = Some(frame.number);
            let surface = HostSurface::new(HostSurfaceKind::Headless, Some(id));
            for change in publication.take_native_changes() {
                let placed = matches!(&change, NativeChange::Place { placements }
                    if placements.iter().any(|placement| views.mounts.contains(placement.id)));
                match views.mounts.apply_change(&change, surface, runtime.native_views_mut(), &dispatcher) {
                    Ok(handled) => called_view |= handled || placed,
                    Err(error) => runtime.report_native_view_mount_error(&error),
                }
            }
            let window = runtime.window(id).expect("live headless window");
            replies.extend(views.live_replies(window).map(|reply| (id, reply)));
            for request in &frame.measurements {
                if views.mounts.contains(request.ticket.id()) {
                    called_view = true;
                    if let Some(size) = views.mounts.measure(request) {
                        replies.push((id, HostReply::NativeMeasurement { ticket: request.ticket, size }));
                    }
                }
            }
        }
        drop(publications);
        for (window, command) in runtime.take_typed_native_commands() {
            if let Some(views) = self.windows.get_mut(&window) {
                called_view |= views.mounts.command(command, &dispatcher);
            }
        }
        if replies.is_empty() && called_view {
            runtime.settle()?;
        }
        let feedback = !replies.is_empty();
        for (window, reply) in replies {
            if runtime.window(window).is_some() {
                runtime.dispatch(window, Input::Host(reply))?;
            }
        }
        Ok(feedback || called_view)
    }
}

impl WindowViews {
    fn live_replies<'a>(&'a self, window: &'a WindowRuntime) -> impl Iterator<Item = HostReply> + 'a {
        let tree = window.tree();
        tree.native_nodes().filter_map(|node| {
            let native = tree.native(node)?;
            native.typed_props.as_ref()?;
            let live = self.mounts.contains(NativeId::from(node));
            (live != native.live).then(|| HostReply::NativeLive { target: tree.guard(node), live })
        })
    }
}
