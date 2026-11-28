//! Direct AppKit ownership, native window services, and Metal presentation.

mod acquisition;
mod application;
mod asks;
mod chrome;
mod drag;
mod editor;
mod input;
mod menus;
mod metal_frame;
mod preferences;
mod services;
mod view;
mod window;

use super::apple_frame_clock::{DisplayClock, retry_on_clock};
use super::{
    Drawable, DrawableAcquisition, FrameClockActivity, HostWake, OsHost, OsWindow, OsWindowContext, PlatformEvent, PlatformSender,
    ServiceCleanup, ShellDriver, WindowAccessibility, WindowAsks, WindowPresentation, WindowTextInput, reply, surface_drawable,
};
use application::{Application, EventQueue, Launch};
use objc2::rc::Retained;
use objc2::{ClassType, MainThreadMarker};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSColor,
    NSRequestUserAttentionType, NSView, NSVisualEffectView, NSWindow, NSWindowOrderingMode, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSPoint, NSRect, NSString, NSURL};
use objc2_quartz_core::CATransaction;
use octara::module::{HostSurface, HostSurfaceKind, RawNativeSurface};
use octara::runtime::WindowId;
use octara::{StatusBarStyle, WindowChrome, WindowGeometry, WindowGeometryRequest, WindowMaterial, WindowOptions};
use octara_model::ask::{Ask, AskId, ClipboardItem, ClipboardKinds, Menu, ServiceError, ServiceId, ServiceOutcome, ServiceRequest};
use octara_model::geometry::WindowViewport;
use octara_model::input::{CursorStyle, DragItem, HostReply, InteractionEvent, PasteEvent};
use octara_model::paint::Image;
use octara_model::theme::ColorScheme;
use octara_model::window::{UserAttention, WindowLevel};
use octara_render::{gpu::OutputColor, wgpu};
use raw_window_handle::{AppKitDisplayHandle, AppKitWindowHandle, RawDisplayHandle, RawWindowHandle};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::{Rc, Weak};
use std::sync::Arc;

pub(super) const SURFACE_KIND: HostSurfaceKind = HostSurfaceKind::MacOs;

type MenuRouting = Rc<RefCell<HashMap<NonNull<NSWindow>, WindowId>>>;

struct MacHost {
    menu_bar: Option<menus::MenuBar>,
    routing: MenuRouting,
    queue: EventQueue,
    launch: Rc<Launch>,
    application: Weak<Application>,
    activate_on_first_regular_window: bool,
}

pub(super) fn create() -> Box<dyn OsHost> {
    let routing = MenuRouting::default();
    let commands = routing.clone();
    let queue = EventQueue::default();
    let sender = queue.sender();
    let menu_bar = menus::MenuBar::new(move |selection| {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let app = NSApplication::sharedApplication(mtm);
        let window = [app.keyWindow(), app.mainWindow()]
            .into_iter()
            .flatten()
            .find_map(|window| commands.borrow().get(&NonNull::from(&*window)).copied());
        let Some(window) = window else { return };
        let response = match selection {
            menus::Selection::Command { name, chord } => HostReply::MenuCommand { name, chord },
            menus::Selection::Paste => HostReply::Paste(PasteEvent { items: services::read_clipboard(ClipboardKinds::all()) }),
        };
        reply(&sender, window, response);
    });
    Box::new(MacHost { menu_bar, routing, queue, launch: Rc::default(), application: Weak::new(), activate_on_first_regular_window: true })
}

impl OsHost for MacHost {
    fn run(self: Box<Self>, driver: Box<dyn ShellDriver>) -> Result<(), String> {
        application::run(self, driver)
    }

    fn wake(&self) -> HostWake {
        self.queue.wake()
    }

    fn take_launch_items(&mut self) -> (Vec<String>, Vec<PathBuf>) {
        self.launch.take_items()
    }

    fn application_active(&self) -> Option<bool> {
        MainThreadMarker::new().map(|main| NSApplication::sharedApplication(main).isActive())
    }

    fn color_scheme(&self) -> Option<ColorScheme> {
        preferences::color_scheme()
    }

    fn reduced_motion(&self) -> Option<bool> {
        Some(NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion())
    }

    fn locale(&self) -> Option<Arc<str>> {
        super::apple_preferences::locale()
    }

    fn high_contrast(&self) -> Option<bool> {
        Some(NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast())
    }

    fn supports_window_vibrancy(&self) -> bool {
        true
    }

    fn create_window(&mut self, id: WindowId, options: &WindowOptions, context: OsWindowContext<'_>) -> Result<Box<dyn OsWindow>, String> {
        let mtm = MainThreadMarker::new().ok_or("AppKit windows must be created on the main thread")?;
        let OsWindowContext { backend, config, sender, transparent } = context;
        let window = window::create(mtm, options);
        let content_size = window.contentRectForFrameRect(window.frame()).size;
        let application = self.application.clone();
        let input_sender = sender.clone();
        let dispatch: Rc<view::Dispatch> = Rc::new(move |event, drag| {
            if matches!(event, PlatformEvent::CursorMoved { .. } | PlatformEvent::Interaction { event: InteractionEvent::Wheel { .. }, .. })
            {
                input_sender.send_event(event);
                return false;
            }
            application.upgrade().is_some_and(|app| app.dispatch(event, drag))
        });
        let view = view::OctaraView::new(mtm, NSRect::new(NSPoint::ZERO, content_size), id, dispatch, sender.wake());
        let container = window::attach(&window, &view);
        let editor = editor::Editor::new(&view);
        let accessibility = view::Accessibility::new(&view, sender.clone());
        let fullscreen = Rc::default();
        let closed = Rc::default();
        let delegate = window::Delegate::install(
            &window,
            window::DelegateState {
                window: id,
                view: view.clone(),
                sender: sender.clone(),
                fullscreen: Rc::clone(&fullscreen),
                closed: Rc::clone(&closed),
            },
        );
        let chrome = chrome::Chrome::new(window.clone(), fullscreen);
        self.routing.borrow_mut().insert(NonNull::from(&*window), id);
        let metal = backend.contains(wgpu::Backends::METAL);
        let clock = (metal && !config.manual_clock && config.metal_acquire_delay.is_zero())
            .then(|| DisplayClock::new(&view.clone().into_super(), id, sender.clone()));
        let acquisition = metal.then(|| {
            let frame_sender = sender.clone();
            acquisition::Acquisition::new(config.metal_acquire_delay, move |target| {
                frame_sender.send_event(PlatformEvent::Frame { window: id, target });
            })
        });
        let mut window = MacWindow {
            routing: self.routing.clone(),
            id,
            sender,
            window,
            container,
            view,
            _delegate: delegate,
            closed,
            chrome,
            editor,
            accessibility,
            vibrancy: None,
            asks: Rc::default(),
            clock,
            acquisition,
            transactional: false,
            activate_on_show: !options.non_activating_panel && std::mem::take(&mut self.activate_on_first_regular_window),
        };
        window.set_transparent(transparent);
        Ok(Box::new(window))
    }

    fn set_menus(&mut self, menus: &[Menu]) {
        if let Some(bar) = &mut self.menu_bar {
            bar.set(menus);
        }
    }

    fn set_app_badge(&mut self, badge: Option<&str>) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let label = badge.map(NSString::from_str);
        NSApplication::sharedApplication(mtm).dockTile().setBadgeLabel(label.as_deref());
    }

    fn request_user_attention(&mut self, _: Option<&mut dyn OsWindow>, attention: UserAttention) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        NSApplication::sharedApplication(mtm).requestUserAttention(match attention {
            UserAttention::Informational => NSRequestUserAttentionType::InformationalRequest,
            UserAttention::Critical => NSRequestUserAttentionType::CriticalRequest,
        });
    }

    fn open_url(&mut self, url: &str) {
        if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
            NSWorkspace::sharedWorkspace().openURL(&url);
        }
    }

    fn read_clipboard(&mut self, accept: ClipboardKinds) -> Option<Vec<ClipboardItem>> {
        Some(services::read_clipboard(accept))
    }

    fn write_clipboard(&mut self, items: &[ClipboardItem]) -> Result<(), ServiceError> {
        services::write_clipboard(items)
    }

    fn write_clipboard_image(&mut self, image: &Image) {
        services::write_image(image);
    }

    fn frame_thread_setup(&self) -> Option<fn()> {
        Some(super::scheduling::interactive)
    }

    fn utility_thread_setup(&self) -> Option<fn()> {
        Some(super::scheduling::utility)
    }

    fn background_thread_setup(&self) -> Option<fn()> {
        Some(super::scheduling::background)
    }
}

struct MacWindow {
    routing: MenuRouting,
    id: WindowId,
    sender: PlatformSender,
    window: Retained<NSWindow>,
    container: Retained<NSView>,
    view: Retained<view::OctaraView>,
    _delegate: Retained<window::Delegate>,
    closed: Rc<Cell<bool>>,
    chrome: chrome::Chrome,
    editor: editor::Editor,
    accessibility: view::Accessibility,
    vibrancy: Option<Retained<NSVisualEffectView>>,
    asks: Rc<asks::Shown>,
    clock: Option<DisplayClock>,
    acquisition: Option<acquisition::Acquisition>,
    transactional: bool,
    activate_on_show: bool,
}

impl MacWindow {
    fn set_vibrancy(&mut self, material: WindowMaterial) {
        let WindowMaterial::Vibrancy(material) = material else {
            if let Some(effect) = self.vibrancy.take() {
                effect.removeFromSuperview();
            }
            return;
        };
        let material = chrome::visual_effect_material(material);
        if let Some(effect) = &self.vibrancy {
            if effect.material() != material {
                effect.setMaterial(material);
            }
            return;
        }
        let effect = chrome::visual_effect(&self.container, material);
        self.container.addSubview_positioned_relativeTo(&effect, NSWindowOrderingMode::Below, None);
        if let Some(layer) = effect.layer() {
            // Module underlays use z=0 between this backdrop and Metal.
            layer.setZPosition(-1.0);
        }
        self.vibrancy = Some(effect);
    }
}

// SAFETY: MacWindow retains the NSWindow, content container, and render view on AppKit's
// main thread for its entire lifetime, including after close().
unsafe impl OsWindow for MacWindow {
    fn raw_handles(&self) -> Result<(RawDisplayHandle, RawWindowHandle), String> {
        Ok((
            RawDisplayHandle::AppKit(AppKitDisplayHandle::new()),
            RawWindowHandle::AppKit(AppKitWindowHandle::new(NonNull::from(self.view.as_super()).cast())),
        ))
    }

    fn host_surface(&self) -> HostSurface<'_> {
        let surface = HostSurface::new(SURFACE_KIND, Some(self.id));
        let raw =
            RawNativeSurface::AppKit { ns_window: NonNull::from(&*self.window).cast(), ns_view: NonNull::from(&*self.container).cast() };
        // SAFETY: The window and native view container are retained for the surface's borrow.
        unsafe { surface.with_raw_native(raw) }
    }

    fn physical_size(&self) -> [u32; 2] {
        let size = self.view.convertRectToBacking(self.view.bounds()).size;
        [size.width.round() as u32, size.height.round() as u32]
    }

    fn scale_factor(&self) -> f32 {
        self.window.backingScaleFactor() as f32
    }

    fn request_redraw(&mut self) {
        self.sender.send_event(PlatformEvent::WindowRedrawRequested(self.id));
    }

    fn set_cursor(&mut self, style: CursorStyle) {
        self.view.set_cursor(style);
    }

    fn set_transparent(&mut self, transparent: bool) {
        self.window.setOpaque(!transparent);
        let background = if transparent { NSColor::clearColor() } else { NSColor::windowBackgroundColor() };
        self.window.setBackgroundColor(Some(&background));
    }

    fn is_focused(&self) -> bool {
        self.window.isKeyWindow()
    }

    fn text_input(&mut self) -> &mut dyn WindowTextInput {
        &mut self.editor
    }

    fn accessibility(&mut self) -> Option<&mut dyn WindowAccessibility> {
        Some(&mut self.accessibility)
    }

    fn configure_chrome(&mut self, options: &WindowOptions) {
        self.chrome.configure(options);
    }

    fn apply_presentation(
        &mut self,
        title: &str,
        color_scheme: ColorScheme,
        material: WindowMaterial,
        _status_bar: StatusBarStyle,
        level: WindowLevel,
    ) {
        self.window.setTitle(&NSString::from_str(title));
        window::set_level(&self.window, level);
        // SAFETY: These are process-lifetime AppKit appearance names.
        let name = unsafe {
            match color_scheme {
                ColorScheme::Light => NSAppearanceNameAqua,
                ColorScheme::Dark => NSAppearanceNameDarkAqua,
            }
        };
        self.window.setAppearance(NSAppearance::appearanceNamed(name).as_deref());
        self.set_vibrancy(material);
    }

    fn window_is_vibrant(&self) -> bool {
        self.vibrancy.is_some()
    }

    fn window_chrome(&mut self) -> WindowChrome {
        self.chrome.metrics(&self.container)
    }

    fn window_geometry(&mut self, viewport: WindowViewport) -> WindowGeometry {
        self.chrome.geometry(viewport)
    }

    fn prepare_window_metrics(&mut self) {
        self.chrome.apply_pending();
    }

    fn set_window_geometry(&mut self, geometry: WindowGeometryRequest) {
        self.chrome.set_geometry(geometry);
    }

    fn toggle_maximize(&mut self) {
        self.chrome.toggle_maximize();
    }

    fn minimize(&mut self) {
        self.chrome.minimize();
    }

    fn start_window_drag(&mut self) {
        self.chrome.start_drag();
    }

    fn start_drag_out(&mut self, items: Vec<DragItem>) -> bool {
        self.view.start_drag_out(&items)
    }

    fn focus(&mut self) {
        if self.window.isMiniaturized() {
            self.window.deminiaturize(None);
        }
        self.window.makeKeyAndOrderFront(None);
    }

    fn asks(&mut self) -> Option<&mut dyn WindowAsks> {
        Some(self)
    }

    fn presentation(&mut self) -> Option<&mut dyn WindowPresentation> {
        Some(self)
    }

    fn request_service(&mut self, id: ServiceId, request: ServiceRequest) -> Option<ServiceCleanup> {
        let send = |outcome| reply(&self.sender, self.id, HostReply::ServiceReply { id, outcome });
        match request {
            ServiceRequest::ReadClipboard { accept } => send(ServiceOutcome::Clipboard(services::read_clipboard(accept))),
            ServiceRequest::WriteClipboard { items } => {
                send(services::write_clipboard(&items).map_or_else(ServiceOutcome::Error, |()| ServiceOutcome::ClipboardWritten));
            }
        }
        Some(Box::new(|| {}))
    }

    fn show(&mut self) {
        if self.window.styleMask().contains(NSWindowStyleMask::NonactivatingPanel) {
            self.window.orderFrontRegardless();
        } else {
            if std::mem::take(&mut self.activate_on_show) {
                NSApplication::sharedApplication(MainThreadMarker::from(&*self.window)).activate();
            }
            self.focus();
        }
        self.chrome.did_show();
        if let Some(acquisition) = &mut self.acquisition {
            acquisition.did_show();
        }
        if let Some(clock) = &mut self.clock {
            clock.did_show();
        }
    }

    fn close(&mut self) {
        self.editor.focused(false);
        self.asks.close();
        self.set_frame_clock_activity(FrameClockActivity::Paused);
        if !self.closed.replace(true) {
            self.window.close();
        }
    }
}

impl WindowAsks for MacWindow {
    fn show_ask(&mut self, id: AskId, ask: Ask) -> Option<Ask> {
        let (sender, window) = (self.sender.clone(), self.id);
        self.asks.show(&self.view, id, ask, Rc::new(move |id, answer| reply(&sender, window, HostReply::AskAnswer { id, answer })));
        None
    }

    fn withdraw_ask(&mut self, id: AskId) {
        self.asks.withdraw(id);
    }
}

impl WindowPresentation for MacWindow {
    fn externally_paced(&self) -> bool {
        self.clock.as_ref().is_some_and(DisplayClock::externally_paced)
    }

    fn set_frame_clock_activity(&mut self, activity: FrameClockActivity) {
        if let Some(clock) = &mut self.clock {
            clock.set_activity(activity);
        }
    }

    fn surface_will_reconfigure(&mut self) -> bool {
        self.acquisition.as_mut().is_none_or(acquisition::Acquisition::reset)
    }

    fn set_transactional_presentation(&mut self, _native_rev: u64, transactional: bool) {
        self.transactional = transactional;
    }

    fn try_acquire_drawable(&mut self, surface: &Arc<wgpu::Surface<'static>>, color: OutputColor, from_clock: bool) -> DrawableAcquisition {
        let externally_paced = self.externally_paced();
        if externally_paced && !from_clock {
            return DrawableAcquisition::WaitingForClock;
        }
        let Some(acquisition) = &mut self.acquisition else { return surface_drawable(surface, color) };
        let acquired = acquisition.acquire(surface, color, externally_paced, self.transactional);
        retry_on_clock(acquired, externally_paced)
    }

    fn present(&mut self, drawable: Box<dyn Drawable>, queue: &wgpu::Queue) -> Result<(), String> {
        drawable.present(queue)
    }

    fn present_with_native_views(
        &mut self,
        drawable: Box<dyn Drawable>,
        queue: &wgpu::Queue,
        _native_rev: u64,
        commit_native_views: &mut dyn FnMut(),
    ) -> Result<(), String> {
        if !self.transactional {
            return drawable.present(queue);
        }
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        let _transaction = std::mem::DropGuard::new((), |()| CATransaction::commit());
        drawable.present(queue)?;
        commit_native_views();
        Ok(())
    }
}

impl Drop for MacWindow {
    fn drop(&mut self) {
        self.window.setDelegate(None);
        self.close();
        self.routing.borrow_mut().remove(&NonNull::from(&*self.window));
        self.set_vibrancy(WindowMaterial::Opaque);
    }
}
