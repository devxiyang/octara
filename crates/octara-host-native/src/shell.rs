//! Native orchestration driven by platform events, runtime deadlines, and GPU completion.

mod frame_input;

use self::frame_input::FrameInput;
use crate::os::{
    self, FileDropPhase, HostEvent, LoopState, OsHost, OsWindowContext, PlatformEvent, PlatformKey, PlatformSender, PointerPhase,
    ShellDriver,
};
use crate::window::{Device, FileDrop, HostWindow, create_surface, transparent_background};
use crate::{NativeAppOptions, ShellConfig, ShellHook, trace};
use octara::module::HostLifecycleEvent;
use octara::runtime::cycle::AppError;
use octara::runtime::{HostCommand, Input, RestartState, WindowId};
use octara::tree::editor::EditorSession;
use octara::{App, AppDefinition, WindowOptions};
use octara_gpu::{
    CaptureOutput, CapturePixels, CaptureWriter, DeviceGeneration, DeviceNotice, FrameProgress, GpuTask, StagedCapture, SurfaceLease,
    SurfaceRendererFactory,
};
use octara_model::ask::{Answer, ServiceError, ServiceOutcome, ServiceRequest};
use octara_model::capture::FrameReceipt;
use octara_model::geometry::{Point, Size};
use octara_model::identity::{NativeId, TargetGuard};
use octara_model::input::{
    Button, ContactTarget, DroppedFile, EditorEvent, HostReply, InteractionEvent, Key, Modifiers, PointerId, PointerKind, TextEdit,
};
use octara_model::native::{NativeEvent, TypedNativeCommand};
use octara_model::theme::ColorScheme;
use octara_model::window::{SystemPreferences, WindowMaterial};
use octara_pipeline::{NativeEngine, NativeEngineConfig, NativePublication};
use octara_render::wgpu;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
enum CaptureStage {
    Read(GpuTask<CapturePixels>, PathBuf),
    Write(CaptureOutput<StagedCapture>),
    Publish(CaptureOutput<PathBuf>),
}

struct Shell {
    config: ShellConfig,
    definition: Option<AppDefinition>,
    first_frame: Option<Box<dyn FnOnce(FrameReceipt)>>,
    first_progress: Option<FrameProgress>,
    hook: Option<Box<dyn ShellHook>>,
    hook_deadline: Option<Instant>,
    app: App,
    sender: PlatformSender,
    pending_lifecycle: Vec<HostLifecycleEvent>,
    foreground: Option<bool>,
    focus_changed: bool,
    frame_sources: Rc<RefCell<BTreeMap<WindowId, NativePublication>>>,
    windows: BTreeMap<WindowId, HostWindow>,
    retired: Vec<HostWindow>,
    pending_windows: BTreeSet<WindowId>,
    frame_input: FrameInput,
    instance: wgpu::Instance,
    device: Option<Device>,
    surface_renderer: Option<SurfaceRendererFactory>,
    generation: u64,
    drain: Option<GpuTask<SurfaceLease>>,
    started: Instant,
    frames: u64,
    capture_writer: Option<CaptureWriter>,
    capture_stage: Option<CaptureStage>,
    capture_finished: bool,
    terminating: bool,
}

impl Shell {
    fn new(definition: AppDefinition, host: &dyn OsHost, sender: PlatformSender, options: NativeAppOptions) -> Self {
        let NativeAppOptions { config, surface_renderer, hook, first_frame, .. } = options;
        let config = config.unwrap_or_else(ShellConfig::from_env);
        let frame_sources = Rc::new(RefCell::new(BTreeMap::new()));
        let outputs = frame_sources.clone();
        let supports_vibrancy = host.supports_window_vibrancy();
        let mut app = App::new(move |id, _| {
            let publication = NativePublication::for_window_host();
            outputs.borrow_mut().insert(id, publication.clone());
            Box::new(NativeEngine::with_publication(
                NativeEngineConfig { system_fonts: true, supports_window_vibrancy: supports_vibrancy, ..Default::default() },
                publication,
            ))
        });
        let thread_setup = host.utility_thread_setup().unwrap_or(|| {});
        app.runtime_mut().set_media_loader(crate::media::NativeMediaLoader { thread_setup });
        app.runtime_mut().set_module_host_kind(host.module_surface_kind());
        app.runtime_mut().set_wake(Some(host.wake()));
        Self {
            config,
            definition: Some(definition),
            first_frame,
            first_progress: None,
            hook,
            hook_deadline: None,
            app,
            sender,
            pending_lifecycle: Vec::new(),
            foreground: None,
            focus_changed: false,
            frame_sources,
            windows: BTreeMap::new(),
            retired: Vec::new(),
            pending_windows: BTreeSet::new(),
            frame_input: FrameInput::default(),
            instance: wgpu::Instance::default(),
            device: None,
            surface_renderer,
            generation: 0,
            drain: None,
            started: Instant::now(),
            frames: 0,
            capture_writer: None,
            capture_stage: None,
            capture_finished: false,
            terminating: false,
        }
    }

    fn create_device(&mut self, os: &dyn OsHost, surface: &wgpu::Surface<'static>) -> Result<(), String> {
        self.generation = self.generation.checked_add(1).expect("device generation exhausted");
        let sender = self.sender.clone();
        let generation = Arc::new(DeviceGeneration::new(
            self.generation,
            Some(Arc::new(move |generation, notice| {
                sender.send_event(PlatformEvent::Device { generation, notice });
            })),
        ));
        self.device = Some(Device::create(
            &self.instance,
            surface,
            generation,
            self.config.trace_gpu,
            self.surface_renderer.as_ref(),
            os.frame_thread_setup().unwrap_or(|| {}),
        )?);
        Ok(())
    }

    fn create_window(&mut self, os: &mut dyn OsHost, id: WindowId) -> Result<(), String> {
        let Some(runtime) = self.app.runtime().window(id) else { return Ok(()) };
        let options = runtime.options().clone();
        let presentation = runtime.presentation();
        let transparent = transparent_background(&presentation.theme)
            || (os.supports_window_vibrancy() && matches!(presentation.material, WindowMaterial::Vibrancy(_)));
        let initial_fullscreen = runtime.window_geometry().fullscreen;
        let backend = self.device.as_ref().map_or(wgpu::Backends::all(), |device| device.adapter.get_info().backend.into());
        let mut window =
            os.create_window(id, &options, OsWindowContext { backend, config: &self.config, sender: self.sender.clone(), transparent })?;
        window.configure_chrome(&options);
        window.apply_presentation(
            &presentation.title,
            presentation.theme.scheme,
            presentation.material,
            presentation.status_bar,
            presentation.level,
        );
        let raw = window.host_surface().raw_native();
        let surface = create_surface(&self.instance, window.as_ref())?;
        if self.device.is_none() {
            self.create_device(os, &surface)?;
        }

        let publication = self.frame_sources.borrow().get(&id).cloned().ok_or("missing native publication")?;
        let mut host = HostWindow::new(id, window, surface, publication, transparent);
        if let Some(active) = os.platform_accessibility_active()
            && let Some(runtime) = self.app.runtime().window(id)
        {
            host.set_accessibility_active(runtime, active);
        }
        host.initial_fullscreen = initial_fullscreen;
        self.windows.insert(id, host);
        if let Some(raw) = raw {
            // SAFETY: The attached HostWindow owns the handles and clears the registration before drop.
            unsafe { self.app.runtime_mut().set_module_native_surface(id, raw) };
        }
        self.lifecycle(HostLifecycleEvent::WindowAttached { window: id })?;
        self.refresh_viewport(id)
    }

    fn detach_window(&mut self, id: WindowId) -> Result<(), String> {
        self.pending_windows.remove(&id);
        self.frame_sources.borrow_mut().remove(&id);
        self.app.runtime_mut().clear_module_native_surface(id);
        if let Some(host) = self.windows.remove(&id) {
            // A canceled draw can still own a surface texture on the GPU worker. Keep the window
            // under shell ownership before calling any native cleanup that may unwind.
            self.retired.push(host);
            self.retired.last_mut().expect("retiring window").retire();
            self.lifecycle(HostLifecycleEvent::WindowDetached { window: id })?;
        }
        Ok(())
    }

    fn dispatch(&mut self, id: WindowId, input: Input) -> Result<bool, String> {
        self.flush_frame_input(None)?;
        let resync_editor = matches!(
            input,
            Input::Editor(_) | Input::Interaction(InteractionEvent::WindowActive(_)) | Input::Host(HostReply::FocusChanged { .. })
        );
        let service = match &input {
            Input::Host(HostReply::ServiceReply { id, .. }) => Some(*id),
            _ => None,
        };
        if let Some(host) = self.windows.get_mut(&id) {
            self.app.runtime_mut().set_keyboard_open(id, host.os.scene().is_some_and(|scene| scene.keyboard_open()));
            if let Some(request) = service {
                host.service_cleanup.remove(&request);
            }
        }
        if let Some(hook) = &mut self.hook {
            hook.before_input(&self.app, id, &input);
        }
        let receipt = self.app.dispatch(id, input);
        if resync_editor
            && let Some(host) = self.windows.get_mut(&id)
            && let Some(runtime) = self.app.runtime().window(id)
        {
            host.synchronize_editor(runtime);
        }
        match receipt {
            Ok(receipt) => Ok(receipt.handled),
            Err(AppError::UnknownWindow(_)) => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    }

    fn drain_commands(&mut self, os: &mut dyn OsHost) -> Result<(), String> {
        let ready = os.is_ready();
        if ready {
            for id in std::mem::take(&mut self.pending_windows) {
                self.create_window(os, id)?;
            }
        }
        let mut native_commands = Vec::new();
        for command in self.app.runtime_mut().take_host_commands() {
            match command {
                HostCommand::Opened(id) if ready => self.create_window(os, id)?,
                HostCommand::Opened(id) => {
                    self.pending_windows.insert(id);
                }
                HostCommand::Closed(id) => self.detach_window(id)?,
                HostCommand::Menus(menus) => os.set_menus(&menus),
                HostCommand::UpdateWindowPresentation { window, title, color_scheme, material, status_bar, level } => {
                    if let Some(host) = self.windows.get_mut(&window) {
                        host.os.apply_presentation(&title, color_scheme, material, status_bar, level);
                    }
                }
                HostCommand::SetAppBadge { badge } => os.set_app_badge(badge.as_deref()),
                HostCommand::RequestUserAttention { window, attention } => {
                    let id = window.filter(|id| self.windows.contains_key(id)).or_else(|| {
                        self.windows.iter().find(|(_, host)| host.os.is_focused()).or_else(|| self.windows.iter().next()).map(|(&id, _)| id)
                    });
                    let window = id.and_then(|id| self.windows.get_mut(&id));
                    if let Some(host) = window {
                        os.request_user_attention(Some(&mut *host.os), attention);
                    } else {
                        os.request_user_attention(None, attention);
                    }
                }
                HostCommand::SetWindowGeometry { window, mut geometry } => {
                    if let Some(host) = self.windows.get_mut(&window)
                        && let Some(runtime) = self.app.runtime().window(window)
                    {
                        geometry.size = geometry.size.map(|size| constrained_size(size, runtime.options()));
                        if geometry.fullscreen.is_some() {
                            host.initial_fullscreen = false;
                        }
                        host.os.set_window_geometry(geometry);
                        self.refresh_viewport(window)?;
                    }
                }
                HostCommand::FocusWindow { window } => {
                    if let Some(host) = self.windows.get_mut(&window) {
                        host.os.focus();
                    }
                }
                HostCommand::MinimizeWindow { window } => {
                    if let Some(host) = self.windows.get_mut(&window) {
                        host.os.minimize();
                    }
                }
                HostCommand::ToggleMaximizeWindow { window } => {
                    if let Some(host) = self.windows.get_mut(&window) {
                        host.os.toggle_maximize();
                        self.refresh_viewport(window)?;
                    }
                }
                HostCommand::StartWindowDrag { window } => {
                    if let Some(host) = self.windows.get_mut(&window) {
                        host.os.start_window_drag();
                    }
                }
                HostCommand::StartDragOut { window, contact, items } => {
                    if self.app.runtime().window(window).is_some()
                        && let Some(host) = self.windows.get_mut(&window)
                        && host.os.start_drag_out(items)
                        && let Some(id) = contact
                    {
                        // The native drag now owns the pointer, including the eventual release.
                        self.dispatch(window, Input::Interaction(InteractionEvent::PointerCancel { id }))?;
                    }
                }
                HostCommand::Ask { window, id, ask } => {
                    if let Some(host) = self.windows.get_mut(&window) {
                        let unhandled = match host.os.asks() {
                            Some(asks) => asks.show_ask(id, ask),
                            None => Some(ask),
                        };
                        if let Some(ask) = unhandled {
                            os::reply(&self.sender, window, HostReply::AskAnswer { id, answer: Answer::dismissed(&ask) });
                        }
                    }
                }
                HostCommand::WithdrawAsk { window, id } => {
                    if let Some(asks) = self.windows.get_mut(&window).and_then(|host| host.os.asks()) {
                        asks.withdraw_ask(id);
                    }
                }
                command @ (HostCommand::NativeCommand { .. } | HostCommand::TypedNativeCommand { .. }) => native_commands.push(command),
                HostCommand::OpenUrl { url } => os.open_url(&url),
                HostCommand::Service { window, id, request } => {
                    let Some(host) = self.windows.get_mut(&window) else { continue };
                    if let Some(cleanup) = host.os.request_service(id, request.clone()) {
                        host.service_cleanup.insert(id, cleanup);
                        continue;
                    }
                    let outcome = match request {
                        ServiceRequest::ReadClipboard { accept } => {
                            os.read_clipboard(accept).map_or(ServiceOutcome::Error(ServiceError::Unavailable), ServiceOutcome::Clipboard)
                        }
                        ServiceRequest::WriteClipboard { items } => {
                            os.write_clipboard(&items).map_or_else(ServiceOutcome::Error, |()| ServiceOutcome::ClipboardWritten)
                        }
                    };
                    os::reply(&self.sender, window, HostReply::ServiceReply { id, outcome });
                }
            }
        }
        self.synchronize(os)?;
        for command in native_commands {
            match command {
                HostCommand::NativeCommand { window, command } => {
                    if let Some(native) = self.live_native_host(window, command.id).and_then(|host| host.os.native_views()) {
                        native.send_native_command(command);
                    }
                }
                HostCommand::TypedNativeCommand { window, command } => self.send_typed_native_command(window, command),
                _ => unreachable!("queued native commands only"),
            }
        }
        Ok(())
    }

    fn live_native_host(&mut self, window: WindowId, id: NativeId) -> Option<&mut HostWindow> {
        let tree = self.app.runtime().window(window)?.tree();
        let node = id.node();
        let live = tree.is_live(node) && tree.native(node).is_some_and(|native| native.live);
        if live { self.windows.get_mut(&window) } else { None }
    }

    fn send_typed_native_command(&mut self, window: WindowId, command: TypedNativeCommand) {
        let dispatcher = self.app.runtime().module_dispatcher();
        if let Some(host) = self.live_native_host(window, command.id) {
            host.native_mounts.command(command, &dispatcher);
        }
    }

    fn refresh_viewport(&mut self, window: WindowId) -> Result<(), String> {
        if let Some(host) = self.windows.get_mut(&window) {
            host.os.prepare_window_metrics();
            let viewport = host.viewport();
            let geometry = host.os.window_geometry(viewport);
            let chrome = host.os.window_chrome();
            let chrome_changed = std::mem::replace(&mut host.chrome, chrome) != chrome;
            if self.app.runtime().window(window).is_some_and(|runtime| runtime.window_geometry() != geometry) {
                self.dispatch(window, Input::Host(HostReply::GeometryChanged(geometry)))?;
            }
            if self.app.runtime().window(window).is_some_and(|runtime| runtime.options().viewport != viewport) {
                self.dispatch(window, Input::Host(HostReply::ViewportChanged(viewport)))?;
            }
            if chrome_changed {
                self.dispatch(window, Input::Host(HostReply::ChromeChanged(chrome)))?;
            }
        }
        Ok(())
    }

    fn refresh_system_preferences(&mut self, os: &dyn OsHost, color_scheme: Option<ColorScheme>) -> Result<(), String> {
        self.refresh_platform_accessibility(os);
        let mut next = self.app.runtime().system_preferences();
        next.color_scheme = color_scheme.or_else(|| os.color_scheme()).unwrap_or(next.color_scheme);
        next.reduced_motion = os.reduced_motion().unwrap_or(next.reduced_motion);
        next.locale = os.locale().unwrap_or(next.locale);
        next.text_scale = os.text_scale().unwrap_or(next.text_scale);
        next.high_contrast = os.high_contrast().unwrap_or(next.high_contrast);
        self.apply_system_preferences(next)
    }

    fn apply_system_preferences(&mut self, next: SystemPreferences) -> Result<(), String> {
        if next == self.app.runtime().system_preferences() {
            return Ok(());
        }
        let window = self.app.runtime().windows().next().map(|(&id, _)| id);
        if let Some(window) = window {
            self.dispatch(window, Input::Host(HostReply::SystemPreferencesChanged(next)))?;
        } else {
            self.app.runtime_mut().set_system_preferences(next).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn native_reply(&mut self, window: WindowId, id: NativeId, event: NativeEvent) -> Result<(), String> {
        let Some(runtime) = self.app.runtime().window(window) else { return Ok(()) };
        let tree = runtime.tree();
        let node = id.node();
        if tree.is_live(node) && tree.native(node).is_some() {
            let target = tree.guard(node);
            self.dispatch(window, Input::Host(HostReply::NativeEvent { target, event }))?;
        }
        Ok(())
    }

    fn synchronize(&mut self, os: &mut dyn OsHost) -> Result<(), String> {
        let mut upcoming = Vec::new();
        os.drain_platform_events(&mut upcoming);
        let runtime = self.app.runtime_mut();
        let dispatcher = runtime.module_dispatcher();
        for host in self.windows.values_mut() {
            if runtime.window(host.id).is_none() {
                continue;
            }
            if let Some(frame) = host.publication.latest().filter(|frame| Some(frame.number) != host.synchronized) {
                host.synchronize_native(runtime, &dispatcher);
                let window = runtime.window(host.id).expect("attached runtime window");
                host.synchronize(window, &frame);
                let tree = window.tree();
                let mut built_in = host.os.native_views();
                for id in tree.native_nodes() {
                    let native = tree.native(id).expect("indexed native");
                    let live = if native.typed_props.is_some() {
                        host.native_mounts.contains(id.into())
                    } else {
                        built_in.as_ref().is_some_and(|views| views.shows_native(native.kind, native.slot))
                    };
                    if live != native.live {
                        upcoming.push(HostEvent {
                            window: host.id,
                            input: Input::Host(HostReply::NativeLive { target: tree.guard(id), live }),
                        });
                    }
                }
                for request in &frame.measurements {
                    let size = if request.typed_props.is_some() {
                        host.native_mounts.measure(request)
                    } else {
                        built_in.as_deref_mut().and_then(|views| views.measure_native(request.clone()))
                    };
                    if let Some(size) = size {
                        upcoming.push(HostEvent {
                            window: host.id,
                            input: Input::Host(HostReply::NativeMeasurement { ticket: request.ticket, size }),
                        });
                    }
                }
            }
            let mut input = Vec::new();
            if let Some(scene) = host.os.scene() {
                scene.drain_scene_events(&mut input);
            }
            host.os.text_input().drain_text_input(&mut input);
            if let Some(native_views) = host.os.native_views() {
                native_views.drain_native_events(&mut input);
            }
            upcoming.extend(input.into_iter().map(|input| HostEvent { window: host.id, input }));
        }
        if !upcoming.is_empty() {
            for HostEvent { window, input } in upcoming {
                self.dispatch(window, input)?;
            }
            self.sender.send_event(PlatformEvent::UserWake);
        }
        Ok(())
    }

    fn configure_surfaces(&mut self, os: &dyn OsHost, now: Instant) -> Result<(), String> {
        if self.device.as_ref().is_some_and(|device| device.executor.generation().is_lost()) {
            self.drain = None;
            let old = self.device.take().expect("lost device");
            old.executor.shutdown();
            self.retired.clear();
            for host in self.windows.values_mut() {
                host.invalidate_surface(true);
            }
            if let Some(surface) = self.windows.values().next().map(HostWindow::surface) {
                self.create_device(os, &surface)?;
            }
        }
        let Some(device) = &self.device else { return Ok(()) };
        if self.drain.is_none() && (!self.retired.is_empty() || self.windows.values().any(|host| host.can_configure(now))) {
            self.drain = Some(device.executor.drain_surfaces().map_err(|error| error.to_string())?);
        }
        if let Some(result) = self.drain.as_mut().and_then(GpuTask::try_take) {
            self.drain = None;
            let lease = result.map_err(|error| error.to_string())?;
            self.retired.clear();
            for host in self.windows.values_mut().filter(|host| host.can_configure(now)) {
                if host.configure(&self.instance, &device.adapter, lease.device())? {
                    host.request_frame();
                }
            }
            lease.resume();
        }
        Ok(())
    }

    fn finish_frames(&mut self) -> Result<(), String> {
        let Some(device) = &self.device else { return Ok(()) };
        let reduced_motion = self.app.runtime().system_preferences().reduced_motion;
        let mut shown = Vec::new();
        for host in self.windows.values_mut() {
            let Some(runtime) = self.app.runtime().window(host.id) else { continue };
            let first = host.presented.is_none();
            if host.finish_draw(runtime, device, reduced_motion)? {
                if first {
                    shown.push(host.id);
                }
                self.frames = self.frames.checked_add(1).expect("presentation count exhausted");
                if let Some(progress) = &host.presented {
                    if self.first_frame.is_some() && self.first_progress.is_none() {
                        self.first_progress = Some(progress.clone());
                    }
                    if self.config.trace {
                        trace::presented(host.id, progress.receipt(None));
                    }
                }
            }
        }
        for id in shown {
            self.refresh_viewport(id)?;
        }
        if self.first_progress.as_ref().is_some_and(FrameProgress::gpu_complete)
            && let Some(callback) = self.first_frame.take()
            && let Some(progress) = self.first_progress.take()
        {
            callback(progress.receipt(None));
        }
        Ok(())
    }

    fn draw(&mut self, id: WindowId, target: Instant, from_clock: bool) -> Result<(), String> {
        if self.drain.is_none()
            && let (Some(host), Some(runtime), Some(device)) =
                (self.windows.get_mut(&id), self.app.runtime().window(id), self.device.as_ref())
        {
            let sample = if self.config.manual_clock { self.app.runtime().now() } else { target.saturating_duration_since(self.started) };
            host.draw(runtime, device, sample, Instant::now(), from_clock)?;
        }
        Ok(())
    }

    fn capture(&mut self, os: &dyn OsHost) -> Result<(), String> {
        let Some(request) = self.config.capture.as_ref() else { return Ok(()) };
        if self.capture_finished {
            return Ok(());
        }
        let wake = os.wake();
        let notify = move || wake();
        match self.capture_stage.as_mut() {
            Some(CaptureStage::Read(task, path)) => {
                if let Some(capture) = task.try_take() {
                    let pixels = capture.map_err(|error| error.to_string())?;
                    if self.capture_writer.is_none() {
                        let setup = os.background_thread_setup().unwrap_or(|| {});
                        self.capture_writer = Some(CaptureWriter::spawn_with_thread_setup(setup).map_err(|error| error.to_string())?);
                    }
                    let output = self.capture_writer.as_ref().expect("capture writer").stage(path.clone(), move || pixels.finish(), notify);
                    self.capture_stage = Some(CaptureStage::Write(output));
                }
            }
            Some(CaptureStage::Write(task)) => {
                if let Some(result) = task.try_take() {
                    self.capture_stage = Some(CaptureStage::Publish(result?.publish(notify)));
                }
            }
            Some(CaptureStage::Publish(task)) => {
                if let Some(result) = task.try_take() {
                    result?;
                    self.capture_finished = true;
                    self.capture_stage = None;
                }
            }
            None if self.started.elapsed() >= request.after => {
                if let (Some(device), Some(host)) = (&self.device, self.windows.values_mut().next())
                    && let Some(frame) = host.publication.latest()
                {
                    let at = self.app.runtime().now();
                    let draw = frame.scene.as_ref().map_or_else(|| frame.draw.clone(), |scene| scene.sample(at.as_secs_f64()));
                    match device.executor.capture(
                        draw,
                        host.viewport().scale,
                        Some(octara_model::capture::PaintSample { revision: frame.number, sampled_at: at }),
                    ) {
                        Ok(task) => self.capture_stage = Some(CaptureStage::Read(task, request.path.clone())),
                        Err(octara_gpu::GpuError::Busy) => {}
                        Err(error) => return Err(error.to_string()),
                    }
                }
            }
            None => {}
        }
        Ok(())
    }

    fn flush_file_drops(&mut self) -> Result<(), String> {
        let batches: Vec<_> = self.windows.values_mut().filter_map(|host| Some((host.id, host.file_drop.take()?))).collect();
        for (id, batch) in batches {
            if let Some(target) = batch.target {
                self.dispatch(id, Input::Host(HostReply::FilesDropped { target, files: batch.files }))?;
            }
        }
        Ok(())
    }

    fn run_ready(&mut self, os: &mut dyn OsHost) -> Result<(), String> {
        if self.config.manual_clock {
            self.app.runtime_mut().settle().map_err(|error| error.to_string())?;
        } else {
            self.app.advance_to(self.started.elapsed()).map_err(|error| error.to_string())?;
        }
        self.run_ready_after_clock(os)
    }

    fn run_ready_after_clock(&mut self, os: &mut dyn OsHost) -> Result<(), String> {
        self.drain_commands(os)?;
        self.configure_surfaces(os, Instant::now())?;
        self.finish_frames()?;
        self.capture(os)?;
        let now = Instant::now();
        if self.hook_deadline.is_none_or(|deadline| deadline <= now)
            && let Some(hook) = self.hook.as_mut()
        {
            let presented =
                |window| self.windows.get(&window).and_then(|host| host.presented.as_ref().map(|progress| progress.receipt(None)));
            let delay = hook.tick(&mut self.app, &presented).map_err(|error| error.to_string())?;
            self.hook_deadline = delay.and_then(|delay| now.checked_add(delay));
            self.drain_commands(os)?;
        }
        for host in self.windows.values_mut() {
            if host.take_retry(now) {
                host.request_frame();
            }
        }
        // A scene can become ready without a separate redraw notification. Admit the
        // latest settled revision once when the OS has no separate frame clock.
        let ready: Vec<_> = self.windows.iter_mut().filter_map(|(&id, host)| host.needs_unpaced_frame(now).then_some(id)).collect();
        for id in ready {
            self.draw(id, now, false)?;
        }
        Ok(())
    }

    fn schedule(&self) -> LoopState {
        let limit = self.config.exit_after_frames.is_some_and(|count| self.frames >= count);
        if self.terminating
            || self.definition.is_none() && self.windows.is_empty() && self.app.runtime().windows().next().is_none()
            || limit && (self.config.capture.is_none() || self.capture_finished) && self.first_frame.is_none()
            || self.hook.as_ref().is_some_and(|hook| hook.shutdown_requested())
        {
            return LoopState { exit: true, deadline: None };
        }
        let runtime_deadline =
            (!self.config.manual_clock).then(|| self.app.runtime().next_deadline()).flatten().map(|at| self.started + at);
        let capture_deadline = if self.capture_finished || self.capture_stage.is_some() {
            None
        } else {
            self.config.capture.as_ref().map(|capture| (self.started + capture.after).max(Instant::now() + Duration::from_millis(16)))
        };
        let deadline = runtime_deadline
            .into_iter()
            .chain(capture_deadline)
            .chain(self.hook_deadline)
            .chain(self.windows.values().filter_map(HostWindow::deadline))
            .min();
        LoopState { exit: false, deadline }
    }

    fn hit(&self, id: WindowId, at: Point) -> Option<ContactTarget> {
        let runtime = self.app.runtime().window(id)?;
        Some(runtime.hit_test(at).unwrap_or(ContactTarget { hit: TargetGuard::for_window(runtime.tree().id()), action: None }))
    }

    fn pointer(
        &mut self,
        id: WindowId,
        phase: PointerPhase,
        at: Option<Point>,
        contact: (PointerId, PointerKind),
        button: Button,
        mods: Modifiers,
    ) -> Result<bool, String> {
        let Some(host) = self.windows.get_mut(&id) else { return Ok(false) };
        let (pointer, kind) = contact;
        let at = at.or(host.pointer);
        if kind == PointerKind::Mouse && at.is_some() {
            host.pointer = at;
            host.pointer_modifiers = mods;
        }
        let event = match phase {
            PointerPhase::Started => {
                let Some(at) = at else { return Ok(false) };
                if !self.config.manual_clock {
                    self.app.advance_to(self.started.elapsed()).map_err(|error| error.to_string())?;
                }
                let Some(target) = self.hit(id, at) else { return Ok(false) };
                InteractionEvent::PointerDown { id: pointer, kind, target, pos: at, button, mods }
            }
            PointerPhase::Moved => {
                let Some(pos) = at else { return Ok(false) };
                InteractionEvent::PointerMove { id: pointer, pos, mods }
            }
            PointerPhase::Ended => {
                let Some(pos) = at else { return Ok(false) };
                InteractionEvent::PointerUp { id: pointer, target: self.hit(id, pos), pos, mods }
            }
            PointerPhase::Cancelled => InteractionEvent::PointerCancel { id: pointer },
        };
        self.dispatch(id, Input::Interaction(event))
    }

    fn text_key(&mut self, id: WindowId, event: PlatformKey) -> Result<bool, String> {
        let editor = self
            .app
            .runtime()
            .window(id)
            .and_then(|window| EditorSession::active(window.tree()))
            .map(|session| (session.id(), session.editor().document_rev()));
        let handled = match (event.event, editor) {
            (Some(key), Some((session, expected_rev))) => {
                self.dispatch(id, Input::Editor(EditorEvent::KeyIntercept { session, expected_rev, key }))?
            }
            (Some(key), None) => self.dispatch(id, Input::Interaction(InteractionEvent::Key(key)))?,
            (None, _) => false,
        };
        if handled {
            return Ok(true);
        }
        if self.windows.get_mut(&id).is_some_and(|host| host.os.text_input().on_unhandled_key(&event)) {
            return Ok(true);
        }
        if let Some((session, expected_rev)) = editor
            && let Some(key) = event.event
            && self.dispatch(id, Input::Editor(EditorEvent::KeyFallback { session, expected_rev, key }))?
        {
            return Ok(true);
        }
        match fallback_edit(self.app.runtime().window(id), &event) {
            Some(edit) => self.dispatch(id, Input::Editor(edit)),
            None => Ok(false),
        }
    }

    fn cursor(&mut self, id: WindowId, at: Option<Point>, modifiers: Modifiers) -> Result<bool, String> {
        let Some(host) = self.windows.get_mut(&id) else { return Ok(false) };
        if let Some(at) = at
            && host.externally_paced()
        {
            let previous = self.frame_input.latest_cursor().or_else(|| host.pointer.map(|pos| (pos, host.pointer_modifiers)));
            if previous != Some((at, modifiers)) {
                self.queue_frame_input(id, frame_input::cursor(at, modifiers))?;
            }
            return Ok(false);
        }
        self.flush_frame_input(None)?;
        let Some(host) = self.windows.get_mut(&id) else { return Ok(false) };
        host.pointer = at;
        host.pointer_modifiers = modifiers;
        let event = at.map_or(InteractionEvent::PointerLeave, |at| frame_input::cursor(at, modifiers));
        let handled = self.dispatch(id, Input::Interaction(event))?;
        if let (Some(host), Some(runtime)) = (self.windows.get_mut(&id), self.app.runtime().window(id)) {
            host.synchronize_cursor(runtime);
        }
        Ok(handled)
    }

    fn interaction(&mut self, id: WindowId, event: InteractionEvent) -> Result<bool, String> {
        if matches!(event, InteractionEvent::Wheel { .. }) && self.windows.get_mut(&id).is_some_and(HostWindow::externally_paced) {
            self.queue_frame_input(id, event)?;
            return Ok(false);
        }
        self.dispatch(id, Input::Interaction(event))
    }

    fn file_drop(&mut self, id: WindowId, phase: FileDropPhase, position: Option<Point>, paths: Vec<PathBuf>) -> Result<bool, String> {
        if phase != FileDropPhase::Dropped {
            self.flush_file_drops()?;
        }
        let Some(host) = self.windows.get_mut(&id) else { return Ok(false) };
        let target = position.or(host.pointer).and_then(|point| self.app.runtime().window(id)?.file_drop_target(point));
        if phase == FileDropPhase::Dropped {
            let batch = host.file_drop.get_or_insert_with(|| FileDrop { target, files: Vec::new() });
            batch.files.extend(paths.into_iter().map(DroppedFile::Path));
            return Ok(batch.target.is_some());
        }
        Ok(phase == FileDropPhase::Entered && target.is_some())
    }

    fn queue_frame_input(&mut self, id: WindowId, event: InteractionEvent) -> Result<(), String> {
        if let InteractionEvent::Wheel { phase, .. } = event
            && self.frame_input.wheel_phase().is_some_and(|previous| previous != phase)
        {
            self.flush_frame_input(None)?;
        }
        self.windows.get_mut(&id).expect("buffered window").activate_frame_clock();
        self.frame_input.window = Some(id);
        let merge_wheel = if let InteractionEvent::Wheel { pos, .. } = event
            && matches!(self.frame_input.packets.last(), Some(InteractionEvent::Wheel { .. }))
        {
            self.app.runtime().window(id).is_some_and(|runtime| runtime.can_coalesce_wheel(pos))
        } else {
            false
        };
        self.frame_input.push(event, merge_wheel);
        Ok(())
    }

    fn flush_frame_input(&mut self, time: Option<Duration>) -> Result<bool, String> {
        let Some(id) = self.frame_input.window.take() else { return Ok(false) };
        let Some(host) = self.windows.get_mut(&id) else {
            self.frame_input.packets.clear();
            return Ok(false);
        };
        let previous = host.pointer;
        if let Some((position, modifiers)) = self.frame_input.latest_cursor() {
            host.pointer = Some(position);
            host.pointer_modifiers = modifiers;
        }
        self.app.runtime_mut().set_keyboard_open(id, host.os.scene().is_some_and(|scene| scene.keyboard_open()));
        if let Some(hook) = self.hook.as_mut() {
            for &event in &self.frame_input.packets {
                hook.before_input(&self.app, id, &Input::Interaction(event));
            }
        }
        let result = self.app.runtime_mut().dispatch_frame_inputs(id, self.frame_input.packets.drain(..).map(Input::Interaction), time);
        if host.pointer != previous
            && let Some(runtime) = self.app.runtime().window(id)
        {
            host.synchronize_cursor(runtime);
        }
        match result {
            Ok(_) | Err(AppError::UnknownWindow(_)) => Ok(true),
            Err(error) => Err(error.to_string()),
        }
    }

    fn refresh_platform_accessibility(&mut self, os: &dyn OsHost) {
        let Some(active) = os.platform_accessibility_active() else { return };
        let runtime = self.app.runtime();
        for host in self.windows.values_mut() {
            if let Some(window) = runtime.window(host.id) {
                host.set_accessibility_active(window, active);
            }
        }
    }

    fn launch_definition(&mut self, os: &mut dyn OsHost) -> Result<(), String> {
        if !os.is_ready() {
            return Ok(());
        }
        if let Some(definition) = self.definition.take() {
            self.refresh_system_preferences(os, None)?;
            let restart = read_restart(&self.config)?;
            definition.launch(&mut self.app, restart).map_err(|error| error.to_string())?;
            let (urls, files) = os.take_launch_items();
            self.dispatch_lifecycle(HostLifecycleEvent::AppLaunched { urls, files })?;
            for event in std::mem::take(&mut self.pending_lifecycle) {
                self.dispatch_lifecycle(event)?;
            }
        }
        Ok(())
    }

    fn lifecycle(&mut self, event: HostLifecycleEvent) -> Result<(), String> {
        let active = match &event {
            HostLifecycleEvent::AppForeground => Some(true),
            HostLifecycleEvent::AppBackground => Some(false),
            _ => None,
        };
        if let Some(active) = active
            && self.foreground.replace(active) == Some(active)
        {
            return Ok(());
        }
        if self.definition.is_some() {
            self.pending_lifecycle.push(event);
            return Ok(());
        }
        self.dispatch_lifecycle(event)
    }

    fn dispatch_lifecycle(&mut self, event: HostLifecycleEvent) -> Result<(), String> {
        self.app.runtime_mut().dispatch_lifecycle(event).map_err(|error| error.to_string())?;
        Ok(())
    }

    fn resume(&mut self, os: &mut dyn OsHost) -> Result<(), String> {
        self.launch_definition(os)?;
        for host in self.windows.values_mut() {
            host.set_suspended(false);
        }
        self.refresh_platform_accessibility(os);
        if os.application_active().unwrap_or(true) {
            self.lifecycle(HostLifecycleEvent::AppForeground)?;
        }
        self.run_ready(os)
    }

    fn platform_event(&mut self, os: &mut dyn OsHost, event: PlatformEvent) -> Result<bool, String> {
        let buffered = match event {
            PlatformEvent::CursorMoved { window, .. } | PlatformEvent::Interaction { window, event: InteractionEvent::Wheel { .. } } => {
                Some(window)
            }
            _ => None,
        };
        if self.frame_input.window.is_some_and(|window| Some(window) != buffered)
            && !matches!(
                event,
                PlatformEvent::FrameClockTick { .. }
                    | PlatformEvent::Frame { .. }
                    | PlatformEvent::UserWake
                    | PlatformEvent::WindowRedrawRequested(_)
            )
        {
            self.flush_frame_input(None)?;
        }

        match event {
            PlatformEvent::Launched | PlatformEvent::Resumed => self.resume(os)?,
            PlatformEvent::PlatformReady => {
                self.launch_definition(os)?;
                self.run_ready(os)?;
            }
            PlatformEvent::Suspended => {
                for host in self.windows.values_mut() {
                    host.set_suspended(true);
                }
                if os.application_active().is_none() {
                    self.lifecycle(HostLifecycleEvent::AppBackground)?;
                }
            }
            PlatformEvent::WillTerminate => self.terminating = true,
            PlatformEvent::Lifecycle(event) => self.lifecycle(event)?,
            PlatformEvent::WindowCloseRequested(window) => return self.dispatch(window, Input::Host(HostReply::CloseRequested)),
            PlatformEvent::WindowDestroyed(window) => {
                if self.app.runtime().window(window).is_some() {
                    self.app.close_window(window).map_err(|error| error.to_string())?;
                }
            }
            PlatformEvent::WindowResized(window) => {
                if let Some(host) = self.windows.get_mut(&window) {
                    host.invalidate_surface(false);
                    self.refresh_viewport(window)?;
                }
            }
            PlatformEvent::WindowMoved(window) | PlatformEvent::RefreshViewport { window } => self.refresh_viewport(window)?,
            PlatformEvent::WindowRedrawRequested(window) => self.draw(window, Instant::now(), false)?,
            PlatformEvent::WindowFocused { window, active } => {
                self.focus_changed |= os.follows_window_focus();
                if let Some(host) = self.windows.get_mut(&window) {
                    host.os.text_input().focused(active);
                }
                return self.dispatch(window, Input::Interaction(InteractionEvent::WindowActive(active)));
            }
            PlatformEvent::CursorMoved { window, position, modifiers } => return self.cursor(window, Some(position), modifiers),
            PlatformEvent::CursorLeft(window) => return self.cursor(window, None, Modifiers::empty()),
            PlatformEvent::Pointer { window, phase, position, id, kind, button, modifiers } => {
                let handled = self.pointer(window, phase, position, (id, kind), button, modifiers)?;
                if button == Button::Primary
                    && (phase == PointerPhase::Started || (phase == PointerPhase::Moved && kind != PointerKind::Touch))
                {
                    // System dragging may need the initiating native input event.
                    self.drain_commands(os)?;
                }
                return Ok(handled);
            }
            PlatformEvent::Key { window, event } => return self.text_key(window, event),
            PlatformEvent::Editor { window, event } => return self.dispatch(window, Input::Editor(event)),
            PlatformEvent::Interaction { window, event } => return self.interaction(window, event),
            PlatformEvent::FileDrop { window, phase, position, paths } => return self.file_drop(window, phase, position, paths),
            PlatformEvent::HostReply { window, reply } => return self.dispatch(window, Input::Host(reply)),
            PlatformEvent::Frame { window, target } => self.draw(window, target, false)?,
            PlatformEvent::FrameClockTick { window, tick } => {
                if self.config.trace_frame_clock {
                    trace::display_tick(window, tick, self.started);
                }
                self.app.runtime_mut().set_frame_interval(tick.interval);
                let time = (!self.config.manual_clock).then(|| self.started.elapsed());
                if self.frame_input.window == Some(window) && self.flush_frame_input(time)? {
                    self.run_ready_after_clock(os)?;
                } else {
                    self.run_ready(os)?;
                }
                self.draw(window, tick.target, true)?;
                if let Some(host) = self.windows.get_mut(&window) {
                    host.idle_frame_clock_if_clean();
                }
            }
            PlatformEvent::Native { window, id, event } => self.native_reply(window, id, event)?,
            PlatformEvent::RefreshSystemPreferences => self.refresh_system_preferences(os, None)?,
            PlatformEvent::SystemPreferencesChanged { color_scheme, reduced_motion, text_scale, high_contrast } => {
                let mut next = self.app.runtime().system_preferences();
                next.color_scheme = color_scheme.unwrap_or(next.color_scheme);
                next.reduced_motion = reduced_motion.unwrap_or(next.reduced_motion);
                next.text_scale = text_scale.unwrap_or(next.text_scale);
                next.high_contrast = high_contrast.unwrap_or(next.high_contrast);
                self.apply_system_preferences(next)?;
            }
            PlatformEvent::AccessibilityEnabledChanged { window, enabled } => {
                if let (Some(host), Some(runtime)) = (self.windows.get_mut(&window), self.app.runtime().window(window)) {
                    host.set_accessibility_active(runtime, enabled);
                }
            }
            PlatformEvent::AccessKitAction { window, request } => {
                if let (Some(host), Some(runtime)) = (self.windows.get(&window), self.app.runtime().window(window))
                    && let Some(reply) = host.publication.accessibility_host_action(runtime.tree(), request)
                {
                    return self.dispatch(window, Input::Host(reply));
                }
            }
            PlatformEvent::PlatformAccessibilityAction { window, target, action } => {
                if let Some(host) = self.windows.get_mut(&window)
                    && let Some(accessibility) = host.os.accessibility()
                    && accessibility.accepts_action(&host.publication, target, &action)
                {
                    return self.dispatch(window, Input::Host(HostReply::AccessibilityAction { target, action }));
                }
            }
            PlatformEvent::Device { generation, notice: DeviceNotice::Failed(error) } if generation == self.generation => {
                return Err(error);
            }
            PlatformEvent::UserWake | PlatformEvent::Device { .. } => {}
        }
        Ok(false)
    }

    fn idle(&mut self, os: &mut dyn OsHost) -> Result<LoopState, String> {
        self.flush_file_drops()?;
        if std::mem::take(&mut self.focus_changed) {
            let active = self.windows.values().any(|host| host.os.is_focused());
            self.lifecycle(if active { HostLifecycleEvent::AppForeground } else { HostLifecycleEvent::AppBackground })?;
        }
        if !self.terminating {
            self.run_ready(os)?;
        }
        Ok(self.schedule())
    }

    fn stop_rendering(&mut self) {
        self.capture_stage = None;
        self.capture_writer.take();
        self.drain = None;
        if let Some(device) = self.device.take() {
            // GPU work must release all borrowed surfaces before the native windows are dropped.
            device.executor.shutdown();
        }
        self.retired.clear();
    }

    fn shutdown(&mut self) -> Result<(), String> {
        let mut error = None;
        if self.definition.is_none() {
            error = self.dispatch_lifecycle(HostLifecycleEvent::WillTerminate).err();
        }
        let windows: Vec<_> = self.windows.keys().copied().collect();
        for id in windows {
            if let Err(detach_error) = self.detach_window(id) {
                error.get_or_insert(detach_error);
            }
        }
        self.app.shutdown();
        self.stop_rendering();
        error.map_or(Ok(()), Err)
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        self.stop_rendering();
    }
}

pub(crate) struct Driver {
    startup: Option<(AppDefinition, NativeAppOptions)>,
    shell: Option<Shell>,
}

impl Driver {
    pub(crate) fn new(definition: AppDefinition, options: NativeAppOptions) -> Self {
        Self { startup: Some((definition, options)), shell: None }
    }
}

impl ShellDriver for Driver {
    fn start(&mut self, host: &mut dyn OsHost, sender: PlatformSender) -> Result<(), String> {
        let (definition, options) = self.startup.take().ok_or("Native host started twice")?;
        self.shell = Some(Shell::new(definition, host, sender, options));
        Ok(())
    }

    fn event(&mut self, host: &mut dyn OsHost, event: PlatformEvent) -> Result<bool, String> {
        self.shell.as_mut().ok_or("Native host sent an event before startup")?.platform_event(host, event)
    }

    fn idle(&mut self, host: &mut dyn OsHost) -> Result<LoopState, String> {
        self.shell.as_mut().ok_or("Native host entered idle before startup")?.idle(host)
    }

    fn shutdown(&mut self, _: &mut dyn OsHost) -> Result<(), String> {
        self.shell.take().map_or(Ok(()), |mut shell| shell.shutdown())
    }
}

fn read_restart(config: &ShellConfig) -> Result<Option<RestartState>, String> {
    let Some(path) = &config.restart_state else { return Ok(None) };
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map(Some).map_err(|error| error.to_string())
}

fn constrained_size(mut size: Size, options: &WindowOptions) -> Size {
    if size.width.is_finite() && size.height.is_finite() && size.width > 0.0 && size.height > 0.0 {
        if let Some(max) = options.max_size {
            size.width = size.width.min(max.width);
            size.height = size.height.min(max.height);
        }
        if let Some(min) = options.min_size {
            size.width = size.width.max(min.width);
            size.height = size.height.max(min.height);
        }
    }
    size
}

fn fallback_edit(window: Option<&octara::runtime::WindowRuntime>, event: &PlatformKey) -> Option<EditorEvent> {
    if event.event.is_some_and(|key| key.modifiers.intersects(Modifiers::CTRL | Modifiers::META)) {
        return None;
    }
    let session = EditorSession::active(window?.tree())?;
    let editor = session.editor();
    if editor.composition().is_some() {
        return None;
    }
    let edit = if event.event.is_some_and(|key| key.key == Key::Enter) && editor.multiline {
        TextEdit::Insert { text: "\n".into(), replacement: None }
    } else {
        let text = event.text.as_deref().filter(|text| !text.is_empty() && !text.chars().any(char::is_control))?;
        TextEdit::Insert { text: text.into(), replacement: None }
    };
    Some(EditorEvent::ProposeEdit { session: session.id(), expected_rev: editor.document_rev(), edit })
}
