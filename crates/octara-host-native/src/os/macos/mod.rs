//! AppKit window services, native text input and Metal presentation.

mod asks;
mod chrome;
mod editor;
mod lifecycle;
mod menus;
mod metal_frame;
mod native;
mod pacing;
mod preferences;
mod services;

use super::{
    Drawable, DrawableAcquisition, HostEvent, OsHost, OsWindow, OsWindowContext, ServiceCleanup, WindowAsks, WindowHostEvent,
    WindowNativeViews, WindowPresentation, WindowTextInput, reply, surface_drawable,
};
use crate::ShellEvent;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSRequestUserAttentionType, NSView, NSWorkspace};
use objc2_foundation::{NSString, NSURL};
use objc2_quartz_core::CATransaction;
use octara::module::{HostSurfaceKind, RawNativeSurface};
use octara::runtime::{Input, WindowId};
use octara::{TitlebarStyle, WindowChrome, WindowGeometry, WindowGeometryRequest, WindowMaterial, WindowOptions};
use octara_model::ask::{Ask, AskId, ClipboardItem, ClipboardKinds, Menu, ServiceError, ServiceId, ServiceOutcome, ServiceRequest};
use octara_model::geometry::{Point, Size, WindowViewport};
use octara_model::identity::NativeId;
use octara_model::input::{HostReply, PasteEvent};
use octara_model::native::{NativeChange, NativeMeasureRequest, NativeSlot, NativeSnapshotRequest};
use octara_model::paint::Image;
use octara_model::theme::ColorScheme;
use octara_model::window::UserAttention;
use octara_render::{gpu::OutputColor, wgpu};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use winit::event_loop::EventLoopProxy;
use winit::platform::macos::WindowAttributesExtMacOS;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowAttributes};

pub(super) const SURFACE_KIND: HostSurfaceKind = HostSurfaceKind::MacOs;

pub(super) fn raw_module_surface(window: &Window) -> Option<RawNativeSurface> {
    let view = content_view(window).ok()?;
    let window = view.window()?;
    Some(RawNativeSurface::AppKit { ns_window: NonNull::from(&*window).cast(), ns_view: NonNull::from(&*view).cast() })
}

fn content_view(window: &Window) -> Result<Retained<NSView>, String> {
    let _mtm = MainThreadMarker::new().ok_or("AppKit windows must be accessed on the main thread")?;
    let RawWindowHandle::AppKit(handle) = window.window_handle().map_err(|error| error.to_string())?.as_raw() else {
        return Err("winit did not supply an AppKit window".into());
    };
    // SAFETY: The AppKit raw handle is the live winit NSView on its owning thread.
    unsafe { Retained::retain(handle.ns_view.as_ptr().cast()) }.ok_or_else(|| "AppKit content view is unavailable".into())
}

#[derive(Default)]
struct MenuRouting {
    windows: HashMap<isize, WindowId>,
    pending: Vec<HostEvent>,
    proxy: Option<EventLoopProxy<ShellEvent>>,
}

struct MacHost {
    menu_bar: Option<menus::MenuBar>,
    routing: Rc<RefCell<MenuRouting>>,
    preferences: Option<preferences::Observer>,
    lifecycle: Option<lifecycle::ActivationObserver>,
}

pub(super) fn create() -> Box<dyn OsHost> {
    let routing = Rc::<RefCell<MenuRouting>>::default();
    let commands = routing.clone();
    let menu_bar = menus::MenuBar::new(move |selection| {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let app = NSApplication::sharedApplication(mtm);
        let mut routing = commands.borrow_mut();
        let window = [app.keyWindow(), app.mainWindow()]
            .into_iter()
            .flatten()
            .find_map(|window| routing.windows.get(&window.windowNumber()).copied());
        let Some(window) = window else { return };
        let reply = match selection {
            menus::Selection::Command { name, chord } => HostReply::MenuCommand { name, chord },
            menus::Selection::Paste => HostReply::Paste(PasteEvent { items: services::read_clipboard(ClipboardKinds::all()) }),
        };
        routing.pending.push(HostEvent { window, input: Input::Host(reply) });
        if let Some(proxy) = &routing.proxy {
            let _ = proxy.send_event(ShellEvent::Wake);
        }
    });
    Box::new(MacHost { menu_bar, routing, preferences: None, lifecycle: None })
}

impl OsHost for MacHost {
    fn initialize(&mut self, proxy: EventLoopProxy<ShellEvent>) {
        super::scheduling::interactive();
        self.preferences = Some(preferences::Observer::new(proxy.clone()));
        self.lifecycle = lifecycle::ActivationObserver::new(proxy);
        self.set_menus(&[]);
    }

    fn application_active(&self) -> Option<bool> {
        MainThreadMarker::new().map(|main| NSApplication::sharedApplication(main).isActive())
    }

    fn reduced_motion(&self) -> Option<bool> {
        Some(NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion())
    }

    fn supports_window_vibrancy(&self) -> bool {
        true
    }

    fn window_attributes(&self, attributes: WindowAttributes, options: &WindowOptions) -> WindowAttributes {
        // The outer position is applied after creation, before AppKit records its zoom restore frame.
        let attributes = attributes.with_maximized(false);
        match options.titlebar {
            TitlebarStyle::System => attributes,
            TitlebarStyle::HiddenInset { .. } => {
                attributes.with_fullsize_content_view(true).with_titlebar_transparent(true).with_title_hidden(true)
            }
        }
    }

    fn create_window(&mut self, context: OsWindowContext<'_>) -> Result<Box<dyn OsWindow>, String> {
        let OsWindowContext { window, id, backend, config, proxy } = context;
        let mtm = MainThreadMarker::new().ok_or("AppKit windows must be created on the main thread")?;
        let view = content_view(window)?;
        let ns_window = view.window().ok_or("AppKit content view has no window")?;
        let number = ns_window.windowNumber();
        let chrome = chrome::Chrome::new(ns_window, proxy.clone(), id);
        {
            let mut routing = self.routing.borrow_mut();
            routing.windows.insert(number, id);
            routing.proxy = Some(proxy.clone());
        }
        window.set_ime_allowed(false);
        let editor = editor::Editor::new(&view, mtm, wake(proxy.clone()));
        let native = native::Views::new(view.clone(), mtm, backend == wgpu::Backend::Metal, wake(proxy.clone()));
        let frame_wake = proxy.clone();
        let pacing = (backend == wgpu::Backend::Metal).then(|| {
            pacing::Pacing::new(mtm, !config.manual_clock, config.metal_acquire_delay, config.metal_frame_latency, move |target| {
                let _ = frame_wake.send_event(ShellEvent::Frame { window: id, target });
            })
        });
        Ok(Box::new(MacWindow {
            routing: self.routing.clone(),
            number,
            id,
            proxy,
            view,
            chrome,
            editor,
            native,
            asks: Rc::default(),
            pacing,
        }))
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

    fn request_user_attention(&mut self, _: Option<&Window>, attention: UserAttention) {
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

    fn write_clipboard_text(&mut self, text: &str) {
        services::write_text(text);
    }

    fn write_clipboard_image(&mut self, image: &Image) {
        services::write_image(image);
    }

    fn drain_platform_events(&mut self, out: &mut Vec<HostEvent>) {
        out.append(&mut self.routing.borrow_mut().pending);
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
    routing: Rc<RefCell<MenuRouting>>,
    number: isize,
    id: WindowId,
    proxy: EventLoopProxy<ShellEvent>,
    view: Retained<NSView>,
    chrome: chrome::Chrome,
    editor: editor::Editor,
    native: native::Views,
    asks: Rc<asks::Shown>,
    pacing: Option<pacing::Pacing>,
}

impl OsWindow for MacWindow {
    fn text_input(&mut self) -> &mut dyn WindowTextInput {
        &mut self.editor
    }

    fn configure_chrome(&mut self, window: &Window, options: &WindowOptions) {
        self.chrome.configure(window, options);
    }

    fn apply_presentation(&mut self, window: &Window, title: &str, color_scheme: ColorScheme, material: WindowMaterial) {
        window.set_title(title);
        window.set_theme(Some(match color_scheme {
            ColorScheme::Light => winit::window::Theme::Light,
            ColorScheme::Dark => winit::window::Theme::Dark,
        }));
        self.native.set_vibrancy(match material {
            WindowMaterial::Opaque => None,
            WindowMaterial::Vibrancy(material) => Some(material),
        });
    }

    fn window_is_vibrant(&self) -> bool {
        self.native.has_vibrancy()
    }

    fn window_chrome(&mut self) -> WindowChrome {
        self.chrome.metrics(&self.view)
    }

    fn window_geometry(&mut self, window: &Window, viewport: WindowViewport) -> WindowGeometry {
        self.chrome.geometry(window, viewport)
    }

    fn prepare_window_metrics(&mut self, window: &Window) {
        self.chrome.apply_pending(window);
    }

    fn set_window_geometry(&mut self, window: &Window, geometry: WindowGeometryRequest) {
        self.chrome.set_geometry(window, geometry);
    }

    fn toggle_maximize(&mut self, window: &Window) {
        self.chrome.toggle_maximize(window);
    }

    fn minimize_window(&mut self, _window: &Window) {
        self.chrome.minimize();
    }

    fn start_window_drag(&mut self, window: &Window) {
        self.chrome.start_drag(window);
    }

    fn asks(&mut self) -> Option<&mut dyn WindowAsks> {
        Some(self)
    }

    fn native_views(&mut self) -> Option<&mut dyn WindowNativeViews> {
        Some(self)
    }

    fn presentation(&mut self) -> Option<&mut dyn WindowPresentation> {
        Some(self)
    }

    fn request_service(&mut self, id: ServiceId, request: ServiceRequest) -> Option<ServiceCleanup> {
        let (proxy, window) = (self.proxy.clone(), self.id);
        let send = move |outcome| reply(&proxy, window, HostReply::ServiceReply { id, outcome });
        match request {
            ServiceRequest::ReadClipboard { accept } => send(ServiceOutcome::Clipboard(services::read_clipboard(accept))),
            ServiceRequest::WriteClipboardText { text } => send(if services::write_text(&text) {
                ServiceOutcome::ClipboardWritten
            } else {
                ServiceOutcome::Error(ServiceError::Failed)
            }),
        }
        Some(Box::new(|| {}))
    }

    fn show(&mut self, window: &Window) {
        window.set_visible(true);
        window.focus_window();
        self.chrome.did_show(window);
        if let Some(pacing) = &mut self.pacing {
            pacing.did_show();
        }
    }

    fn file_drop_position(&self) -> Option<Point> {
        let window = self.view.window()?;
        let point = self.view.convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None);
        let y = if self.view.isFlipped() { point.y } else { self.view.bounds().size.height - point.y };
        Some(Point::new(point.x as f32, y as f32))
    }
}

impl WindowAsks for MacWindow {
    fn show_ask(&mut self, id: AskId, ask: Ask) -> Option<Ask> {
        let (proxy, window) = (self.proxy.clone(), self.id);
        self.asks.show(&self.view, id, ask, Rc::new(move |id, answer| reply(&proxy, window, HostReply::AskAnswer { id, answer })));
        None
    }

    fn withdraw_ask(&mut self, id: AskId) {
        self.asks.withdraw(id);
    }
}

impl WindowNativeViews for MacWindow {
    fn sync_native_views(&mut self, changes: &[NativeChange]) {
        self.native.sync(changes);
    }

    fn shows_native(&self, kind: &str, slot: NativeSlot) -> bool {
        self.native.shows(kind, slot)
    }

    fn measure_native(&mut self, request: NativeMeasureRequest) -> Option<Size> {
        self.native.measure(request.ticket.id())
    }

    fn snapshot_native(&mut self, request: NativeSnapshotRequest) {
        if let Err(error) = self.native.snapshot(request) {
            reply(&self.proxy, self.id, HostReply::NativeSnapshot { request, snapshot: Err(error) });
        }
    }

    fn native_busy(&self, id: NativeId) -> Option<String> {
        self.native.busy(id)
    }

    fn has_underlay(&self) -> bool {
        self.native.has_underlay()
    }

    fn drain_native_events(&mut self, out: &mut Vec<WindowHostEvent>) {
        out.extend(
            self.native
                .inbox()
                .take_snapshots()
                .into_iter()
                .map(|(request, snapshot)| Input::Host(HostReply::NativeSnapshot { request, snapshot })),
        );
        for (id, event) in self.native.inbox().take() {
            let _ = self.proxy.send_event(ShellEvent::Native { window: self.id, id, event });
        }
    }

    fn inspect_native_views(&self) -> serde_json::Value {
        self.native.inspect()
    }
}

impl WindowPresentation for MacWindow {
    fn externally_paced(&self) -> bool {
        self.pacing.as_ref().is_some_and(pacing::Pacing::externally_paced)
    }

    fn set_frame_clock_active(&mut self, active: bool) {
        if let Some(pacing) = &mut self.pacing {
            pacing.set_active(active);
        }
    }

    fn surface_will_reconfigure(&mut self) -> bool {
        self.native.set_layer(None);
        self.pacing.as_mut().is_none_or(pacing::Pacing::reset)
    }

    fn try_acquire_drawable(&mut self, surface: &Arc<wgpu::Surface<'static>>, color: OutputColor) -> DrawableAcquisition {
        let Some(pacing) = &mut self.pacing else { return surface_drawable(surface, color) };
        let acquired = pacing.acquire(surface, color);
        self.native.set_layer(pacing.layer());
        acquired
    }

    fn present(&mut self, drawable: Box<dyn Drawable>, queue: &wgpu::Queue) -> Result<(), String> {
        self.present_with_native_views(drawable, queue, &mut || {})
    }

    fn present_with_native_views(
        &mut self,
        drawable: Box<dyn Drawable>,
        queue: &wgpu::Queue,
        commit_native_views: &mut dyn FnMut(),
    ) -> Result<(), String> {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        let _transaction = std::mem::DropGuard::new((), |()| CATransaction::commit());
        drawable.present(queue)?;
        self.native.commit();
        commit_native_views();
        Ok(())
    }
}

impl Drop for MacWindow {
    fn drop(&mut self) {
        self.asks.close();
        self.routing.borrow_mut().windows.remove(&self.number);
    }
}

fn wake(proxy: EventLoopProxy<ShellEvent>) -> impl Fn() {
    move || {
        let _ = proxy.send_event(ShellEvent::Wake);
    }
}
