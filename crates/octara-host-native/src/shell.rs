//! Winit orchestration driven by runtime deadlines and device completion events.

use crate::os::{self, HostEvent, KeyDispatch, OsHost, OsWindowContext};
use crate::window::{Device, FileDrop, HostWindow, transparent_background};
use crate::{NativeAppOptions, ShellConfig, ShellError, ShellHook, trace};
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
use octara_model::geometry::{Point, Size, Vec2};
use octara_model::identity::{NativeId, TargetGuard};
use octara_model::input::{
    Button, ContactTarget, DroppedFile, EditorEvent, HostReply, InteractionEvent, Key, KeyEvent, Modifiers, PointerId, PointerKind,
    TextEdit, WheelPhase,
};
use octara_model::native::{NativeEvent, TypedNativeCommand};
use octara_model::theme::ColorScheme;
use octara_model::window::{SystemPreferences, WindowMaterial};
use octara_pipeline::{NativeEngine, NativeEngineConfig, NativePublication};
use octara_render::wgpu;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key as NativeKey, KeyCode, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId as HostId};

pub enum ShellEvent {
    Wake,
    PlatformReady,
    Lifecycle(HostLifecycleEvent),
    Device { generation: u64, notice: DeviceNotice },
    Input { window: WindowId, input: Input },
    Frame { window: WindowId, target: Instant },
    Native { window: WindowId, id: NativeId, event: NativeEvent },
    RefreshViewport { window: WindowId },
    RefreshReducedMotion,
    SystemPreferencesChanged { color_scheme: Option<ColorScheme>, reduced_motion: Option<bool> },
    Accessibility(accesskit_winit::Event),
}

impl From<accesskit_winit::Event> for ShellEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::Accessibility(event)
    }
}

enum CaptureStage {
    Read(GpuTask<CapturePixels>, PathBuf),
    Write(CaptureOutput<StagedCapture>),
    Publish(CaptureOutput<PathBuf>),
}

pub(crate) struct Shell {
    config: ShellConfig,
    definition: Option<AppDefinition>,
    first_frame: Option<Box<dyn FnOnce(FrameReceipt)>>,
    first_progress: Option<FrameProgress>,
    hook: Option<Box<dyn ShellHook>>,
    hook_deadline: Option<Instant>,
    app: App,
    os: Box<dyn OsHost>,
    proxy: EventLoopProxy<ShellEvent>,
    pending_lifecycle: Vec<HostLifecycleEvent>,
    foreground: Option<bool>,
    focus_changed: bool,
    frame_sources: Rc<RefCell<BTreeMap<WindowId, NativePublication>>>,
    windows: BTreeMap<WindowId, HostWindow>,
    ids: HashMap<HostId, WindowId>,
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
    error: Option<ShellError>,
}

impl Shell {
    pub(crate) fn new(definition: AppDefinition, proxy: EventLoopProxy<ShellEvent>, options: NativeAppOptions) -> Self {
        let NativeAppOptions { config, os_host, surface_renderer, hook, first_frame } = options;
        let config = config.unwrap_or_else(ShellConfig::from_env);
        let frame_sources = Rc::new(RefCell::new(BTreeMap::new()));
        let outputs = frame_sources.clone();
        let mut os = os_host.unwrap_or_else(os::create_os_host);
        let supports_vibrancy = os.supports_window_vibrancy();
        let mut app = App::new(move |id, _| {
            let publication = NativePublication::for_window_host();
            outputs.borrow_mut().insert(id, publication.clone());
            Box::new(NativeEngine::with_publication(
                NativeEngineConfig { system_fonts: true, supports_window_vibrancy: supports_vibrancy, ..Default::default() },
                publication,
            ))
        });
        let thread_setup = os.utility_thread_setup().unwrap_or(|| {});
        app.runtime_mut().set_media_loader(crate::media::NativeMediaLoader { thread_setup });
        app.runtime_mut().set_module_host_kind(os.module_surface_kind());
        let wake = proxy.clone();
        app.runtime_mut().set_wake(Some(Arc::new(move || {
            let _ = wake.send_event(ShellEvent::Wake);
        })));
        os.initialize(proxy.clone());
        Self {
            config,
            definition: Some(definition),
            first_frame,
            first_progress: None,
            hook,
            hook_deadline: None,
            app,
            os,
            proxy,
            pending_lifecycle: Vec::new(),
            foreground: None,
            focus_changed: false,
            frame_sources,
            windows: BTreeMap::new(),
            ids: HashMap::new(),
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
            error: None,
        }
    }

    pub(crate) fn take_error(&mut self) -> Option<ShellError> {
        self.error.take()
    }

    fn create_device(&mut self, surface: &wgpu::Surface<'static>) -> Result<(), String> {
        self.generation = self.generation.checked_add(1).expect("device generation exhausted");
        let proxy = self.proxy.clone();
        let generation = Arc::new(DeviceGeneration::new(
            self.generation,
            Some(Arc::new(move |generation, notice| {
                let _ = proxy.send_event(ShellEvent::Device { generation, notice });
            })),
        ));
        self.device = Some(Device::create(
            &self.instance,
            surface,
            generation,
            self.config.trace_gpu,
            self.surface_renderer.as_ref(),
            self.os.frame_thread_setup().unwrap_or(|| {}),
        )?);
        Ok(())
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop, id: WindowId) -> Result<(), String> {
        let Some(runtime) = self.app.runtime().window(id) else { return Ok(()) };
        let options = runtime.options().clone();
        let presentation = runtime.presentation();
        let size = options.viewport.size;
        let transparent = transparent_background(&presentation.theme)
            || (self.os.supports_window_vibrancy() && matches!(presentation.material, WindowMaterial::Vibrancy(_)));
        let initial_fullscreen = runtime.window_geometry().fullscreen;
        let mut attributes = Window::default_attributes()
            .with_title(presentation.title.as_ref())
            .with_inner_size(LogicalSize::new(size.width, size.height))
            .with_resizable(options.resizable)
            .with_maximized(options.maximized)
            .with_visible(false)
            .with_transparent(transparent);
        if let Some(position) = options.position {
            attributes = attributes.with_position(LogicalPosition::new(position.x, position.y));
        }
        if let Some(size) = options.min_size {
            attributes = attributes.with_min_inner_size(LogicalSize::new(size.width, size.height));
        }
        if let Some(size) = options.max_size {
            attributes = attributes.with_max_inner_size(LogicalSize::new(size.width, size.height));
        }
        let attributes = self.os.window_attributes(attributes, &options);
        let window = Arc::new(event_loop.create_window(attributes).map_err(|error| error.to_string())?);
        let a11y = accesskit_winit::Adapter::with_event_loop_proxy(event_loop, &window, self.proxy.clone());
        self.os.prepare_window(&window)?;
        let surface = Arc::new(self.instance.create_surface(window.clone()).map_err(|error| error.to_string())?);
        if self.device.is_none() {
            self.create_device(&surface)?;
        }
        let backend = self.device.as_ref().expect("initialized device").adapter.get_info().backend;
        let mut os =
            self.os.create_window(OsWindowContext { window: &window, id, backend, config: &self.config, proxy: self.proxy.clone() })?;
        os.configure_chrome(&window, &options);
        let publication = self.frame_sources.borrow().get(&id).cloned().ok_or("missing native publication")?;
        let mut host = HostWindow::new(id, window.clone(), os, a11y, surface, publication, transparent);
        host.initial_fullscreen = initial_fullscreen;
        self.ids.insert(window.id(), id);
        self.windows.insert(id, host);
        self.lifecycle(HostLifecycleEvent::WindowAttached { window: id })?;
        if let Some(theme) = window.theme() {
            self.refresh_system_preferences(Some(color_scheme(theme)), None)?;
        }
        self.refresh_viewport(id)
    }

    fn detach_window(&mut self, id: WindowId) -> Result<(), String> {
        self.frame_sources.borrow_mut().remove(&id);
        if let Some(host) = self.windows.remove(&id) {
            self.ids.remove(&host.window.id());
            drop(host);
            self.lifecycle(HostLifecycleEvent::WindowDetached { window: id })?;
        }
        Ok(())
    }

    fn dispatch(&mut self, id: WindowId, input: Input) -> Result<bool, String> {
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

    fn drain_commands(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let mut native_commands = Vec::new();
        for command in self.app.runtime_mut().take_host_commands() {
            match command {
                HostCommand::Opened(id) => self.create_window(event_loop, id)?,
                HostCommand::Closed(id) => self.detach_window(id)?,
                HostCommand::Menus(menus) => self.os.set_menus(&menus),
                HostCommand::UpdateWindowPresentation { window, title, color_scheme, material } => {
                    if let Some(host) = self.windows.get_mut(&window) {
                        host.os.apply_presentation(&host.window, &title, color_scheme, material);
                    }
                }
                HostCommand::SetAppBadge { badge } => self.os.set_app_badge(badge.as_deref()),
                HostCommand::RequestUserAttention { window, attention } => {
                    let host = window
                        .and_then(|id| self.windows.get(&id))
                        .or_else(|| self.windows.values().find(|host| host.window.has_focus()))
                        .or_else(|| self.windows.values().next());
                    self.os.request_user_attention(host.map(|host| host.window.as_ref()), attention);
                }
                HostCommand::SetWindowGeometry { window, mut geometry } => {
                    if let Some(host) = self.windows.get_mut(&window)
                        && let Some(runtime) = self.app.runtime().window(window)
                    {
                        geometry.size = geometry.size.map(|size| constrained_size(size, runtime.options()));
                        if geometry.fullscreen.is_some() {
                            host.initial_fullscreen = false;
                        }
                        host.os.set_window_geometry(&host.window, geometry);
                        self.refresh_viewport(window)?;
                    }
                }
                HostCommand::FocusWindow { window } => {
                    if let Some(host) = self.windows.get_mut(&window) {
                        host.os.focus_window(&host.window);
                    }
                }
                HostCommand::MinimizeWindow { window } => {
                    if let Some(host) = self.windows.get_mut(&window) {
                        host.os.minimize_window(&host.window);
                    }
                }
                HostCommand::ToggleMaximizeWindow { window } => {
                    if let Some(host) = self.windows.get_mut(&window) {
                        host.os.toggle_maximize(&host.window);
                        self.refresh_viewport(window)?;
                    }
                }
                HostCommand::StartWindowDrag { window } => {
                    if let Some(host) = self.windows.get_mut(&window) {
                        host.os.start_window_drag(&host.window);
                    }
                }
                HostCommand::Ask { window, id, ask } => {
                    if let Some(host) = self.windows.get_mut(&window) {
                        let unhandled = match host.os.asks() {
                            Some(asks) => asks.show_ask(id, ask),
                            None => Some(ask),
                        };
                        if let Some(ask) = unhandled {
                            os::reply(&self.proxy, window, HostReply::AskAnswer { id, answer: Answer::dismissed(&ask) });
                        }
                    }
                }
                HostCommand::WithdrawAsk { window, id } => {
                    if let Some(asks) = self.windows.get_mut(&window).and_then(|host| host.os.asks()) {
                        asks.withdraw_ask(id);
                    }
                }
                command @ (HostCommand::NativeCommand { .. } | HostCommand::TypedNativeCommand { .. }) => native_commands.push(command),
                HostCommand::OpenUrl { url } => self.os.open_url(&url),
                HostCommand::Service { window, id, request } => {
                    let Some(host) = self.windows.get_mut(&window) else { continue };
                    if let Some(cleanup) = host.os.request_service(id, request.clone()) {
                        host.service_cleanup.insert(id, cleanup);
                        continue;
                    }
                    let outcome = match request {
                        ServiceRequest::ReadClipboard { accept } => self
                            .os
                            .read_clipboard(accept)
                            .map_or(ServiceOutcome::Error(ServiceError::Unavailable), ServiceOutcome::Clipboard),
                        ServiceRequest::WriteClipboardText { text } => {
                            self.os.write_clipboard_text(&text);
                            ServiceOutcome::ClipboardWritten
                        }
                        ServiceRequest::PickFile { .. } => ServiceOutcome::Error(ServiceError::Unavailable),
                    };
                    os::reply(&self.proxy, window, HostReply::ServiceReply { id, outcome });
                }
            }
        }
        self.synchronize()?;
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
            host.os.prepare_window_metrics(&host.window);
            let viewport = host.viewport();
            let geometry = host.os.window_geometry(&host.window, viewport);
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

    fn refresh_system_preferences(&mut self, color_scheme: Option<ColorScheme>, reduced_motion: Option<bool>) -> Result<(), String> {
        let previous = self.app.runtime().system_preferences();
        let next = SystemPreferences {
            color_scheme: color_scheme.unwrap_or(previous.color_scheme),
            reduced_motion: reduced_motion.unwrap_or(previous.reduced_motion),
        };
        if next == previous {
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

    fn synchronize(&mut self) -> Result<(), String> {
        let mut upcoming = Vec::new();
        self.os.drain_platform_events(&mut upcoming);
        let kind = self.os.module_surface_kind();
        let runtime = self.app.runtime_mut();
        let dispatcher = runtime.module_dispatcher();
        for host in self.windows.values_mut() {
            if runtime.window(host.id).is_none() {
                continue;
            }
            if let Some(frame) = host.publication.latest().filter(|frame| Some(frame.number) != host.synchronized) {
                host.synchronize_native(kind, runtime, &dispatcher);
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
            let _ = self.proxy.send_event(ShellEvent::Wake);
        }
        Ok(())
    }

    fn configure_surfaces(&mut self, now: Instant) -> Result<(), String> {
        if self.device.as_ref().is_some_and(|device| device.executor.generation().is_lost()) {
            self.drain = None;
            let old = self.device.take().expect("lost device");
            old.executor.shutdown();
            for host in self.windows.values_mut() {
                host.invalidate_surface(true);
            }
            if let Some(surface) = self.windows.values().next().map(HostWindow::surface) {
                self.create_device(&surface)?;
            }
        }
        let Some(device) = &self.device else { return Ok(()) };
        if self.drain.is_none() && self.windows.values().any(|host| host.can_configure(now)) {
            self.drain = Some(device.executor.drain_surfaces().map_err(|error| error.to_string())?);
        }
        if let Some(result) = self.drain.as_mut().and_then(GpuTask::try_take) {
            self.drain = None;
            let lease = result.map_err(|error| error.to_string())?;
            for host in self.windows.values_mut().filter(|host| host.can_configure(now)) {
                if host.configure(&self.instance, &device.adapter, lease.device())? {
                    host.window.request_redraw();
                }
            }
            lease.resume();
        }
        Ok(())
    }

    fn finish_frames(&mut self) -> Result<(), String> {
        let Some(device) = &self.device else { return Ok(()) };
        let mut shown = Vec::new();
        for host in self.windows.values_mut() {
            let Some(runtime) = self.app.runtime().window(host.id) else { continue };
            let first = host.presented.is_none();
            if host.finish_draw(runtime, device)? {
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

    fn draw(&mut self, id: WindowId, target: Instant) -> Result<(), String> {
        if self.drain.is_none()
            && let (Some(host), Some(runtime), Some(device)) =
                (self.windows.get_mut(&id), self.app.runtime().window(id), self.device.as_ref())
        {
            let sample = if self.config.manual_clock { self.app.runtime().now() } else { target.saturating_duration_since(self.started) };
            host.draw(runtime, device, sample, Instant::now())?;
        }
        Ok(())
    }

    fn capture(&mut self) -> Result<(), String> {
        let Some(request) = self.config.capture.as_ref() else { return Ok(()) };
        if self.capture_finished {
            return Ok(());
        }
        let wake = self.proxy.clone();
        let notify = move || {
            let _ = wake.send_event(ShellEvent::Wake);
        };
        match self.capture_stage.as_mut() {
            Some(CaptureStage::Read(task, path)) => {
                if let Some(capture) = task.try_take() {
                    let pixels = capture.map_err(|error| error.to_string())?;
                    if self.capture_writer.is_none() {
                        let setup = self.os.background_thread_setup().unwrap_or(|| {});
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

    fn run_ready(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        if self.config.manual_clock {
            self.app.runtime_mut().settle().map_err(|error| error.to_string())?;
        } else {
            self.app.advance_to(self.started.elapsed()).map_err(|error| error.to_string())?;
        }
        self.drain_commands(event_loop)?;
        self.configure_surfaces(Instant::now())?;
        self.finish_frames()?;
        self.capture()?;
        let now = Instant::now();
        if self.hook_deadline.is_none_or(|deadline| deadline <= now)
            && let Some(hook) = self.hook.as_mut()
        {
            let presented =
                |window| self.windows.get(&window).and_then(|host| host.presented.as_ref().map(|progress| progress.receipt(None)));
            let delay = hook.tick(&mut self.app, &presented).map_err(|error| error.to_string())?;
            self.hook_deadline = delay.and_then(|delay| now.checked_add(delay));
            self.drain_commands(event_loop)?;
        }
        for host in self.windows.values_mut() {
            if host.take_retry(now) {
                host.window.request_redraw();
            }
        }
        // A scene can become ready without delivering a winit redraw. Admit the
        // latest settled revision once when the OS has no separate frame clock.
        let ready: Vec<_> = self.windows.iter_mut().filter_map(|(&id, host)| host.needs_unpaced_frame(now).then_some(id)).collect();
        for id in ready {
            self.draw(id, now)?;
        }
        Ok(())
    }

    fn schedule(&mut self, event_loop: &ActiveEventLoop) {
        let limit = self.config.exit_after_frames.is_some_and(|count| self.frames >= count);
        if self.definition.is_none() && self.windows.is_empty() && self.app.runtime().windows().next().is_none()
            || limit && (self.config.capture.is_none() || self.capture_finished) && self.first_frame.is_none()
            || self.hook.as_ref().is_some_and(|hook| hook.shutdown_requested())
        {
            event_loop.exit();
            return;
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
        event_loop.set_control_flow(deadline.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }

    fn hit(&self, id: WindowId, at: Point) -> Option<ContactTarget> {
        let runtime = self.app.runtime().window(id)?;
        Some(runtime.hit_test(at).unwrap_or(ContactTarget { hit: TargetGuard::for_window(runtime.tree().id()), action: None }))
    }

    fn pointer(
        &mut self,
        id: WindowId,
        phase: TouchPhase,
        at: Point,
        pointer: PointerId,
        kind: PointerKind,
        button: Button,
    ) -> Result<(), String> {
        let mods = self.windows[&id].modifiers;
        let event = match phase {
            TouchPhase::Started => {
                if !self.config.manual_clock {
                    self.app.advance_to(self.started.elapsed()).map_err(|error| error.to_string())?;
                }
                let Some(target) = self.hit(id, at) else { return Ok(()) };
                InteractionEvent::PointerDown { id: pointer, kind, target, pos: at, button, mods }
            }
            TouchPhase::Moved => InteractionEvent::PointerMove { id: pointer, pos: at, mods },
            TouchPhase::Ended => InteractionEvent::PointerUp { id: pointer, target: self.hit(id, at), pos: at, mods },
            TouchPhase::Cancelled => InteractionEvent::PointerCancel { id: pointer },
        };
        self.dispatch(id, Input::Interaction(event))?;
        Ok(())
    }

    fn text_key(&mut self, id: WindowId, event: winit::event::KeyEvent) -> Result<(), String> {
        let host = self.windows.get_mut(&id).expect("known host window");
        let mods = host.modifiers;
        match host.os.text_input().on_winit_key(&event) {
            KeyDispatch::Consumed => {}
            KeyDispatch::Input(input) => {
                self.dispatch(id, input)?;
            }
            KeyDispatch::Forward if event.state == ElementState::Pressed => {
                let key = translate_key(&event, mods);
                let editor = self
                    .app
                    .runtime()
                    .window(id)
                    .and_then(|window| EditorSession::active(window.tree()))
                    .map(|session| (session.id(), session.editor().document_rev()));
                let handled = match (key, editor) {
                    (Some(key), Some((session, expected_rev))) => {
                        self.dispatch(id, Input::Editor(EditorEvent::KeyIntercept { session, expected_rev, key }))?
                    }
                    (Some(key), None) => self.dispatch(id, Input::Interaction(InteractionEvent::Key(key)))?,
                    (None, _) => false,
                };
                let os_handled =
                    self.windows.get_mut(&id).is_some_and(|host| !handled && host.os.text_input().on_unhandled_winit_key(&event));
                if !handled
                    && !os_handled
                    && let Some((session, expected_rev)) = editor
                {
                    if let Some(key) = key
                        && self.dispatch(id, Input::Editor(EditorEvent::KeyFallback { session, expected_rev, key }))?
                    {
                        return Ok(());
                    }
                    if let Some(edit) = fallback_edit(self.app.runtime().window(id), &event, mods) {
                        self.dispatch(id, Input::Editor(edit))?;
                    }
                }
            }
            KeyDispatch::Forward => {}
        }
        Ok(())
    }

    fn window_input(&mut self, id: WindowId, event: WindowEvent) -> Result<(), String> {
        let cursor_moved = matches!(event, WindowEvent::CursorMoved { .. } | WindowEvent::CursorLeft { .. });
        match event {
            WindowEvent::CloseRequested => {
                self.dispatch(id, Input::Host(HostReply::CloseRequested))?;
            }
            WindowEvent::Destroyed => self.app.close_window(id).map_err(|error| error.to_string())?,
            WindowEvent::ThemeChanged(theme) => self.refresh_system_preferences(Some(color_scheme(theme)), None)?,
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.windows.get_mut(&id).expect("known window").invalidate_surface(false);
                self.refresh_viewport(id)?;
            }
            WindowEvent::Moved(_) => self.refresh_viewport(id)?,
            WindowEvent::RedrawRequested => self.draw(id, Instant::now())?,
            WindowEvent::Focused(active) => {
                self.focus_changed |= self.os.follows_window_focus();
                self.windows.get_mut(&id).expect("known window").os.text_input().focused(active);
                self.dispatch(id, Input::Interaction(InteractionEvent::WindowActive(active)))?;
            }
            WindowEvent::ModifiersChanged(mods) => {
                let state = mods.state();
                let mut value = Modifiers::empty();
                value.set(Modifiers::SHIFT, state.shift_key());
                value.set(Modifiers::CTRL, state.control_key());
                value.set(Modifiers::ALT, state.alt_key());
                value.set(Modifiers::META, state.super_key());
                self.windows.get_mut(&id).expect("known window").modifiers = value;
            }
            WindowEvent::CursorMoved { position, .. } => {
                let host = self.windows.get_mut(&id).expect("known window");
                let p = position.to_logical::<f32>(host.window.scale_factor());
                let at = Point::new(p.x, p.y);
                host.pointer = Some(at);
                self.pointer(id, TouchPhase::Moved, at, PointerId(0), PointerKind::Mouse, Button::Primary)?;
            }
            WindowEvent::CursorLeft { .. } => {
                self.windows.get_mut(&id).expect("known window").pointer = None;
                self.dispatch(id, Input::Interaction(InteractionEvent::PointerLeave))?;
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let Some(at) = self.windows[&id].pointer else { return Ok(()) };
                let button = match button {
                    MouseButton::Left => Button::Primary,
                    MouseButton::Right => Button::Secondary,
                    MouseButton::Middle => Button::Middle,
                    _ => return Ok(()),
                };
                let phase = if state == ElementState::Pressed { TouchPhase::Started } else { TouchPhase::Ended };
                self.pointer(id, phase, at, PointerId(0), PointerKind::Mouse, button)?;
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let host = &self.windows[&id];
                let Some(pos) = host.pointer else { return Ok(()) };
                let delta = match delta {
                    MouseScrollDelta::LineDelta(x, y) => Vec2::new(-x * 32.0, -y * 32.0),
                    MouseScrollDelta::PixelDelta(point) => {
                        let point: LogicalPosition<f32> = point.to_logical(host.window.scale_factor());
                        Vec2::new(-point.x, -point.y)
                    }
                };
                self.dispatch(
                    id,
                    Input::Interaction(InteractionEvent::Wheel { pos, delta, phase: WheelPhase::Discrete, mods: host.modifiers }),
                )?;
            }
            WindowEvent::Touch(Touch { phase, location, id: touch, .. }) => {
                let scale = self.windows[&id].window.scale_factor();
                let position: LogicalPosition<f32> = location.to_logical(scale);
                self.pointer(
                    id,
                    phase,
                    Point::new(position.x, position.y),
                    PointerId(touch.wrapping_add(1)),
                    PointerKind::Touch,
                    Button::Primary,
                )?;
            }
            WindowEvent::HoveredFile(_) if self.windows[&id].file_drop.is_some() => self.flush_file_drops()?,
            WindowEvent::DroppedFile(path) => {
                let host = self.windows.get_mut(&id).expect("known window");
                let batch = host.file_drop.get_or_insert_with(|| {
                    // Winit emits a separate event for each path and supplies no drag coordinates.
                    let target = host
                        .os
                        .file_drop_position()
                        .or(host.pointer)
                        .and_then(|point| self.app.runtime().window(id)?.file_drop_target(point));
                    FileDrop { target, files: Vec::new() }
                });
                batch.files.push(DroppedFile::Path(path));
            }
            WindowEvent::KeyboardInput { event, .. } => self.text_key(id, event)?,
            WindowEvent::Ime(event) => {
                if let Some(edit) = self.windows.get_mut(&id).and_then(|host| host.os.text_input().on_winit_ime(&event)) {
                    self.dispatch(id, Input::Editor(edit))?;
                }
            }
            _ => {}
        }
        if cursor_moved && let (Some(host), Some(runtime)) = (self.windows.get_mut(&id), self.app.runtime().window(id)) {
            host.synchronize_cursor(runtime);
        }
        Ok(())
    }

    fn accessibility(&mut self, event: accesskit_winit::Event) -> Result<(), String> {
        let Some(&id) = self.ids.get(&event.window_id) else { return Ok(()) };
        let Some(host) = self.windows.get_mut(&id) else { return Ok(()) };
        match event.window_event {
            accesskit_winit::WindowEvent::InitialTreeRequested => {
                if host.publication.latest().is_some() {
                    host.a11y.update_if_active(|| host.publication.snapshot_full().expect("published accessibility tree"));
                }
            }
            accesskit_winit::WindowEvent::ActionRequested(request) => {
                if let Some(runtime) = self.app.runtime().window(id)
                    && let Some(reply) = host.publication.accessibility_host_action(runtime.tree(), request)
                {
                    self.dispatch(id, Input::Host(reply))?;
                }
            }
            accesskit_winit::WindowEvent::AccessibilityDeactivated => {}
        }
        Ok(())
    }

    fn launch_definition(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        if !self.os.is_ready() {
            return Ok(());
        }
        if let Some(definition) = self.definition.take() {
            self.refresh_system_preferences(event_loop.system_theme().map(color_scheme), self.os.reduced_motion())?;
            let restart = read_restart(&self.config)?;
            definition.launch(&mut self.app, restart).map_err(|error| error.to_string())?;
            let (urls, files) = self.os.take_launch_items();
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

    fn handle(&mut self, event_loop: &ActiveEventLoop, f: impl FnOnce(&mut Self) -> Result<(), String>) {
        if self.error.is_some() {
            return;
        }
        if let Err(error) = f(self) {
            self.os.report_error(&error);
            self.error = Some(ShellError(error));
            event_loop.exit();
        }
    }
}

impl ApplicationHandler<ShellEvent> for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.handle(event_loop, |shell| {
            shell.launch_definition(event_loop)?;
            for host in shell.windows.values_mut() {
                host.set_suspended(false);
            }
            if shell.os.application_active().unwrap_or(true) {
                shell.lifecycle(HostLifecycleEvent::AppForeground)?;
            }
            shell.run_ready(event_loop)?;
            shell.schedule(event_loop);
            Ok(())
        });
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        self.handle(event_loop, |shell| {
            for host in shell.windows.values_mut() {
                host.set_suspended(true);
            }
            if shell.os.application_active().is_none() {
                shell.lifecycle(HostLifecycleEvent::AppBackground)?;
            }
            Ok(())
        });
    }

    fn memory_warning(&mut self, event_loop: &ActiveEventLoop) {
        self.handle(event_loop, |shell| shell.lifecycle(HostLifecycleEvent::MemoryWarning));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, raw: HostId, event: WindowEvent) {
        let Some(&id) = self.ids.get(&raw) else { return };
        if let Some(host) = self.windows.get_mut(&id) {
            host.a11y.process_event(&host.window, &event);
        }
        let can_drag = matches!(
            event,
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. }
                | WindowEvent::Touch(Touch { phase: TouchPhase::Started, .. })
        );
        self.handle(event_loop, |shell| {
            shell.window_input(id, event)?;
            if can_drag {
                // Window systems require a drag request from the initiating pointer event.
                shell.drain_commands(event_loop)?;
            }
            Ok(())
        });
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: ShellEvent) {
        self.handle(event_loop, |shell| {
            match event {
                ShellEvent::PlatformReady => {
                    shell.launch_definition(event_loop)?;
                    shell.run_ready(event_loop)?;
                    shell.schedule(event_loop);
                }
                ShellEvent::Lifecycle(event) => shell.lifecycle(event)?,
                ShellEvent::Input { window, input } => {
                    shell.dispatch(window, input)?;
                }
                ShellEvent::Frame { window, target } => shell.draw(window, target)?,
                ShellEvent::Native { window, id, event } => shell.native_reply(window, id, event)?,
                ShellEvent::RefreshViewport { window } => shell.refresh_viewport(window)?,
                ShellEvent::RefreshReducedMotion => shell.refresh_system_preferences(None, shell.os.reduced_motion())?,
                ShellEvent::SystemPreferencesChanged { color_scheme, reduced_motion } => {
                    shell.refresh_system_preferences(color_scheme, reduced_motion)?;
                }
                ShellEvent::Accessibility(event) => shell.accessibility(event)?,
                ShellEvent::Device { generation, notice: DeviceNotice::Failed(error) } if generation == shell.generation => {
                    return Err(error);
                }
                ShellEvent::Wake | ShellEvent::Device { .. } => {}
            }
            Ok(())
        });
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.handle(event_loop, |shell| {
            shell.flush_file_drops()?;
            if std::mem::take(&mut shell.focus_changed) {
                let active = shell.windows.values().any(|host| host.window.has_focus());
                shell.lifecycle(if active { HostLifecycleEvent::AppForeground } else { HostLifecycleEvent::AppBackground })?;
            }
            shell.run_ready(event_loop)?;
            shell.schedule(event_loop);
            Ok(())
        });
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        if self.definition.is_none()
            && let Err(error) = self.dispatch_lifecycle(HostLifecycleEvent::WillTerminate)
            && self.error.is_none()
        {
            self.os.report_error(&error);
            self.error = Some(ShellError(error));
        }
        self.capture_stage = None;
        self.capture_writer.take();
        self.drain = None;
        let windows: Vec<_> = self.windows.keys().copied().collect();
        for id in windows {
            if let Err(error) = self.detach_window(id)
                && self.error.is_none()
            {
                self.os.report_error(&error);
                self.error = Some(ShellError(error));
            }
        }
        self.app.shutdown();
        if let Some(device) = self.device.take() {
            device.executor.shutdown();
        }
    }
}

fn color_scheme(theme: winit::window::Theme) -> ColorScheme {
    match theme {
        winit::window::Theme::Light => ColorScheme::Light,
        winit::window::Theme::Dark => ColorScheme::Dark,
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

fn translate_key(event: &winit::event::KeyEvent, modifiers: Modifiers) -> Option<KeyEvent> {
    let key = match event.logical_key.as_ref() {
        NativeKey::Character(text) => Key::Char(text.chars().next()?),
        NativeKey::Named(name) => match name {
            NamedKey::Tab => Key::Tab,
            NamedKey::Enter => Key::Enter,
            NamedKey::Escape => Key::Escape,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::Space => Key::Space,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::ContextMenu => Key::Menu,
            _ => Key::F(function_key(name)?),
        },
        _ => return None,
    };
    let mut key = KeyEvent::new(key, modifiers);
    key.repeat = event.repeat;
    key.code = physical_character(event.physical_key);
    Some(key)
}

fn function_key(key: NamedKey) -> Option<u8> {
    const KEYS: [NamedKey; 24] = [
        NamedKey::F1,
        NamedKey::F2,
        NamedKey::F3,
        NamedKey::F4,
        NamedKey::F5,
        NamedKey::F6,
        NamedKey::F7,
        NamedKey::F8,
        NamedKey::F9,
        NamedKey::F10,
        NamedKey::F11,
        NamedKey::F12,
        NamedKey::F13,
        NamedKey::F14,
        NamedKey::F15,
        NamedKey::F16,
        NamedKey::F17,
        NamedKey::F18,
        NamedKey::F19,
        NamedKey::F20,
        NamedKey::F21,
        NamedKey::F22,
        NamedKey::F23,
        NamedKey::F24,
    ];
    KEYS.iter().position(|&candidate| candidate == key).map(|index| index as u8 + 1)
}

fn physical_character(key: PhysicalKey) -> Option<char> {
    let PhysicalKey::Code(key) = key else { return None };
    const LETTERS: [KeyCode; 26] = [
        KeyCode::KeyA,
        KeyCode::KeyB,
        KeyCode::KeyC,
        KeyCode::KeyD,
        KeyCode::KeyE,
        KeyCode::KeyF,
        KeyCode::KeyG,
        KeyCode::KeyH,
        KeyCode::KeyI,
        KeyCode::KeyJ,
        KeyCode::KeyK,
        KeyCode::KeyL,
        KeyCode::KeyM,
        KeyCode::KeyN,
        KeyCode::KeyO,
        KeyCode::KeyP,
        KeyCode::KeyQ,
        KeyCode::KeyR,
        KeyCode::KeyS,
        KeyCode::KeyT,
        KeyCode::KeyU,
        KeyCode::KeyV,
        KeyCode::KeyW,
        KeyCode::KeyX,
        KeyCode::KeyY,
        KeyCode::KeyZ,
    ];
    const DIGITS: [KeyCode; 10] = [
        KeyCode::Digit0,
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    if let Some(index) = LETTERS.iter().position(|&candidate| candidate == key) {
        return Some(char::from(b'a' + index as u8));
    }
    if let Some(index) = DIGITS.iter().position(|&candidate| candidate == key) {
        return Some(char::from(b'0' + index as u8));
    }
    Some(match key {
        KeyCode::Backquote => '`',
        KeyCode::Backslash => '\\',
        KeyCode::BracketLeft => '[',
        KeyCode::BracketRight => ']',
        KeyCode::Comma => ',',
        KeyCode::Equal => '=',
        KeyCode::Minus => '-',
        KeyCode::Period => '.',
        KeyCode::Quote => '\'',
        KeyCode::Semicolon => ';',
        KeyCode::Slash => '/',
        _ => return None,
    })
}

fn fallback_edit(
    window: Option<&octara::runtime::WindowRuntime>,
    event: &winit::event::KeyEvent,
    modifiers: Modifiers,
) -> Option<EditorEvent> {
    if modifiers.intersects(Modifiers::CTRL | Modifiers::META) {
        return None;
    }
    let session = EditorSession::active(window?.tree())?;
    let editor = session.editor();
    if editor.composition().is_some() {
        return None;
    }
    let edit = if event.logical_key == NativeKey::Named(NamedKey::Enter) && editor.multiline {
        TextEdit::Insert { text: "\n".into(), replacement: None }
    } else {
        let text = event.text.as_deref().filter(|text| !text.is_empty() && !text.chars().any(char::is_control))?;
        TextEdit::Insert { text: text.into(), replacement: None }
    };
    Some(EditorEvent::ProposeEdit { session: session.id(), expected_rev: editor.document_rev(), edit })
}
