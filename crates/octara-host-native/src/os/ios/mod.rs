//! UIKit scene, native text editing, navigation, and platform services.

mod a11y;
mod back;
mod draft;
mod editor;
mod gestures;
mod scene;

use super::apple_frame_clock::{DisplayClock, retry_on_clock};
use super::apple_metal::{metal_layer, set_transactional};
use crate::ShellEvent;
use crate::os::{
    ActiveEditorView, Drawable, DrawableAcquisition, FrameClockActivity, KeyDispatch, OsHost, OsWindow, OsWindowContext, ServiceCleanup,
    WindowAccessibility, WindowHostEvent, WindowPresentation, WindowScene, WindowTextInput, reply, surface_drawable,
};
use back::BackNavigation;
use editor::TextInput;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_foundation::{NSArray, NSCurrentLocaleDidChangeNotification, NSData, NSDictionary, NSNotificationCenter, NSString, NSURL};
use objc2_quartz_core::{CAMetalLayer, CATransaction};
use objc2_ui_kit::{
    UIAccessibilityDarkerSystemColorsEnabled, UIAccessibilityDarkerSystemColorsStatusDidChangeNotification,
    UIAccessibilityIsReduceMotionEnabled, UIAccessibilityReduceMotionStatusDidChangeNotification,
    UIAccessibilitySpeakScreenStatusDidChangeNotification, UIAccessibilitySwitchControlStatusDidChangeNotification,
    UIAccessibilityVoiceOverStatusDidChangeNotification, UIApplication, UIContentSizeCategoryDidChangeNotification, UIFont,
    UIFontTextStyleBody, UIPasteboard, UIUserInterfaceStyle, UIView,
};
use octara::module::{HostSurfaceKind, RawNativeSurface};
use octara::runtime::{Input, WindowId};
use octara_model::ask::{ClipboardItem, ClipboardKinds, ServiceError, ServiceId, ServiceOutcome, ServiceRequest};
use octara_model::geometry::WindowViewport;
use octara_model::identity::TargetGuard;
use octara_model::input::{HostReply, InteractionEvent};
use octara_model::theme::ColorScheme;
use octara_model::window::{StatusBarStyle, WindowGeometry, WindowGeometryRequest, WindowMaterial};
use octara_render::{gpu::OutputColor, wgpu};
use scene::SceneConnection;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::KeyEvent;
use winit::event_loop::EventLoopProxy;
use winit::keyboard::PhysicalKey;
use winit::platform::ios::{StatusBarStyle as UIKitStatusBarStyle, WindowAttributesExtIOS, WindowExtIOS};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowAttributes};

pub(super) const SURFACE_KIND: HostSurfaceKind = HostSurfaceKind::Ios;

pub(super) fn raw_module_surface(window: &Window) -> Option<RawNativeSurface> {
    let RawWindowHandle::UiKit(handle) = window.window_handle().ok()?.as_raw() else { return None };
    Some(RawNativeSurface::UiKit { ui_view: handle.ui_view, ui_view_controller: handle.ui_view_controller? })
}

pub(super) fn create() -> Box<dyn OsHost> {
    Box::new(IosHost { preferences: None })
}

struct IosHost {
    preferences: Option<super::apple_preferences::Observer>,
}

impl OsHost for IosHost {
    fn initialize(&mut self, proxy: EventLoopProxy<ShellEvent>) {
        super::scheduling::interactive();
        scene::register(proxy.clone());
        // SAFETY: Foundation and UIKit expose these as static notification names.
        let names = unsafe {
            [
                NSCurrentLocaleDidChangeNotification,
                UIContentSizeCategoryDidChangeNotification,
                UIAccessibilityDarkerSystemColorsStatusDidChangeNotification,
                UIAccessibilityReduceMotionStatusDidChangeNotification,
                UIAccessibilityVoiceOverStatusDidChangeNotification,
                UIAccessibilitySwitchControlStatusDidChangeNotification,
                UIAccessibilitySpeakScreenStatusDidChangeNotification,
            ]
        };
        self.preferences = Some(super::apple_preferences::Observer::new(NSNotificationCenter::defaultCenter(), &names, proxy));
    }

    fn reduced_motion(&self) -> Option<bool> {
        Some(UIAccessibilityIsReduceMotionEnabled())
    }

    fn locale(&self) -> Option<Arc<str>> {
        super::apple_preferences::locale()
    }

    fn text_scale(&self) -> Option<f32> {
        // SAFETY: The body style is a UIKit constant and the font is live.
        let point_size = unsafe { UIFont::preferredFontForTextStyle(UIFontTextStyleBody).pointSize() };
        Some(point_size as f32 / 17.0)
    }

    fn high_contrast(&self) -> Option<bool> {
        Some(UIAccessibilityDarkerSystemColorsEnabled())
    }

    fn platform_accessibility_active(&self) -> Option<bool> {
        Some(a11y::platform_accessibility_active())
    }

    fn take_launch_items(&mut self) -> (Vec<String>, Vec<PathBuf>) {
        scene::take_launch_items()
    }

    fn application_active(&self) -> Option<bool> {
        scene::foreground()
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

    fn is_ready(&self) -> bool {
        scene::ready()
    }

    fn window_attributes(&self, mut attributes: WindowAttributes, options: &octara::WindowOptions) -> WindowAttributes {
        // UIKit sizes the window to its scene.
        attributes.inner_size = None;
        let presentation = &options.presentation;
        let (style, hidden) = status_bar_appearance(presentation.status_bar, presentation.theme.scheme);
        attributes.with_preferred_status_bar_style(style).with_prefers_status_bar_hidden(hidden)
    }

    fn prepare_window(&self, window: &Window) -> Result<(), String> {
        scene::prepare_window(window)
    }

    fn report_error(&self, error: &str) {
        unsafe extern "C" {
            fn NSLog(format: *const NSString, ...);
        }
        let message = NSString::from_str(error);
        let format = NSString::from_str("Octara: %@");
        // The Objective-C string operands are retained for the duration of the call.
        unsafe {
            NSLog(&*format, &*message as *const NSString);
        }
    }

    fn create_window(&mut self, context: OsWindowContext<'_>) -> Result<Box<dyn OsWindow>, String> {
        let OsWindowContext { window, id, backend, config, proxy } = context;
        let content = scene::view(window)?;
        let scene_active = Rc::new(Cell::new(true));
        let refresh: Rc<dyn Fn()> = Rc::new({
            let proxy = proxy.clone();
            move || {
                let _ = proxy.send_event(ShellEvent::RefreshViewport { window: id });
            }
        });
        let scene = SceneConnection::new(&content, refresh.clone(), {
            let proxy = proxy.clone();
            let scene_active = scene_active.clone();
            move |active| {
                scene_active.set(active);
                let _ =
                    proxy.send_event(ShellEvent::Input { window: id, input: Input::Interaction(InteractionEvent::WindowActive(active)) });
            }
        })?;
        let editor = TextInput::new(
            content.clone(),
            {
                let proxy = proxy.clone();
                move || {
                    let _ = proxy.send_event(ShellEvent::Wake);
                }
            },
            move || refresh(),
        );
        let back = BackNavigation::new(window, proxy.clone())?;
        let gestures = gestures::Recognizers::new(&content, proxy.clone(), id);
        let clock = (backend == wgpu::Backend::Metal && !config.manual_clock).then(|| DisplayClock::new(&content, id, proxy.clone()));
        let accessibility = a11y::Bridge::new(content.clone(), id, proxy.clone());
        Ok(Box::new(IosWindow {
            accessibility,
            content,
            scene,
            editor,
            back,
            _gestures: gestures,
            proxy,
            id,
            scene_active,
            active: true,
            focused: true,
            clock,
            layer: None,
            transactional: false,
            status_bar: None,
            last_viewport: Cell::new(None),
        }))
    }

    fn open_url(&mut self, url: &str) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else { return };
        // SAFETY: The empty options dictionary has the types required by UIKit.
        unsafe { UIApplication::sharedApplication(mtm).openURL_options_completionHandler(&url, &NSDictionary::new(), None) };
    }

    fn read_clipboard(&mut self, accept: ClipboardKinds) -> Option<Vec<ClipboardItem>> {
        MainThreadMarker::new().map(|_| read_clipboard(accept))
    }

    fn write_clipboard(&mut self, items: &[ClipboardItem]) -> Result<(), ServiceError> {
        write_clipboard(items)
    }
}

struct IosWindow {
    accessibility: a11y::Bridge,
    content: Retained<UIView>,
    scene: SceneConnection,
    editor: TextInput,
    back: BackNavigation,
    _gestures: gestures::Recognizers,
    proxy: EventLoopProxy<ShellEvent>,
    id: WindowId,
    scene_active: Rc<Cell<bool>>,
    active: bool,
    focused: bool,
    clock: Option<DisplayClock>,
    layer: Option<Retained<CAMetalLayer>>,
    transactional: bool,
    status_bar: Option<(UIKitStatusBarStyle, bool)>,
    last_viewport: Cell<Option<WindowViewport>>,
}

impl IosWindow {
    fn refresh_activity(&mut self) {
        let active = self.active && self.focused && self.scene_active.get();
        self.editor.set_active(active);
        self.back.set_active(active);
    }
}

impl OsWindow for IosWindow {
    fn accessibility(&mut self) -> Option<&mut dyn WindowAccessibility> {
        Some(&mut self.accessibility)
    }

    fn surface_size(&self, _: &Window) -> PhysicalSize<u32> {
        // winit's inner size on iOS already excludes the system safe area.
        let bounds = self.content.bounds().size;
        LogicalSize::new(bounds.width, bounds.height).to_physical(self.content.contentScaleFactor())
    }

    fn window_geometry(&mut self, window: &Window, viewport: WindowViewport) -> WindowGeometry {
        WindowGeometry { size: viewport.size, scale: viewport.scale, fullscreen: window.fullscreen().is_some(), ..Default::default() }
    }

    fn set_window_geometry(&mut self, _window: &Window, _geometry: WindowGeometryRequest) {}

    fn apply_presentation(
        &mut self,
        window: &Window,
        title: &str,
        color_scheme: ColorScheme,
        _material: WindowMaterial,
        status_bar: StatusBarStyle,
    ) {
        self.content.setOverrideUserInterfaceStyle(match color_scheme {
            ColorScheme::Light => UIUserInterfaceStyle::Light,
            ColorScheme::Dark => UIUserInterfaceStyle::Dark,
        });
        if let Some(scene) = self.content.window().and_then(|window| window.windowScene()) {
            scene.setTitle(Some(&NSString::from_str(title)));
        }
        let appearance = status_bar_appearance(status_bar, color_scheme);
        let previous = self.status_bar.replace(appearance);
        if previous.map(|(style, _)| style) != Some(appearance.0) {
            window.set_preferred_status_bar_style(appearance.0);
        }
        if previous.map(|(_, hidden)| hidden) != Some(appearance.1) {
            window.set_prefers_status_bar_hidden(appearance.1);
        }
    }

    fn focus_window(&mut self, _window: &Window) {
        if let Some(window) = self.content.window() {
            window.makeKeyWindow();
        }
    }

    fn minimize_window(&mut self, _window: &Window) {}

    fn toggle_maximize(&mut self, _window: &Window) {}

    fn start_window_drag(&mut self, _window: &Window) {}

    fn text_input(&mut self) -> &mut dyn WindowTextInput {
        self
    }

    fn scene(&mut self) -> Option<&mut dyn WindowScene> {
        Some(self)
    }

    fn presentation(&mut self) -> Option<&mut dyn WindowPresentation> {
        Some(self)
    }

    fn show(&mut self, _: &Window) {
        self.scene.show();
        self.editor.focus();
        if let Some(clock) = &mut self.clock {
            clock.did_show();
        }
    }

    fn request_service(&mut self, id: ServiceId, request: ServiceRequest) -> Option<ServiceCleanup> {
        let outcome = match request {
            ServiceRequest::ReadClipboard { accept } => ServiceOutcome::Clipboard(read_clipboard(accept)),
            ServiceRequest::WriteClipboard { items } => {
                write_clipboard(&items).map_or_else(ServiceOutcome::Error, |()| ServiceOutcome::ClipboardWritten)
            }
        };
        reply(&self.proxy, self.id, HostReply::ServiceReply { id, outcome });
        Some(Box::new(|| {}))
    }
}

impl WindowPresentation for IosWindow {
    fn externally_paced(&self) -> bool {
        self.clock.as_ref().is_some_and(DisplayClock::externally_paced)
    }

    fn set_frame_clock_activity(&mut self, activity: FrameClockActivity) {
        if let Some(clock) = &mut self.clock {
            clock.set_activity(activity);
        }
    }

    fn surface_will_reconfigure(&mut self) -> bool {
        if let Some(layer) = self.layer.take() {
            set_transactional(&layer, false);
        }
        true
    }

    fn set_transactional_presentation(&mut self, _: u64, transactional: bool) {
        self.transactional = transactional;
    }

    fn try_acquire_drawable(&mut self, surface: &Arc<wgpu::Surface<'static>>, color: OutputColor, from_clock: bool) -> DrawableAcquisition {
        let externally_paced = self.externally_paced();
        if externally_paced && !from_clock {
            return DrawableAcquisition::WaitingForClock;
        }
        if self.transactional && self.layer.is_none() {
            self.layer = metal_layer(surface);
        }
        if let Some(layer) = &self.layer {
            set_transactional(layer, self.transactional);
        }
        retry_on_clock(surface_drawable(surface, color), externally_paced)
    }

    fn present(&mut self, drawable: Box<dyn Drawable>, queue: &wgpu::Queue) -> Result<(), String> {
        drawable.present(queue)
    }

    fn present_with_native_views(
        &mut self,
        drawable: Box<dyn Drawable>,
        queue: &wgpu::Queue,
        _: u64,
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

impl WindowTextInput for IosWindow {
    fn sync_text_input(&mut self, editor: Option<&ActiveEditorView<'_>>) {
        self.refresh_activity();
        self.editor.sync(editor);
    }

    fn focused(&mut self, focused: bool) {
        self.focused = focused;
        self.refresh_activity();
    }

    fn on_winit_key(&mut self, event: &KeyEvent) -> KeyDispatch {
        if matches!(event.physical_key, PhysicalKey::Unidentified(_)) {
            // UIKit delivers soft keyboard content through the active native responder.
            KeyDispatch::Consumed
        } else {
            KeyDispatch::Forward
        }
    }

    fn on_unhandled_winit_key(&mut self, _: &KeyEvent) -> bool {
        true
    }

    fn drain_text_input(&mut self, out: &mut Vec<WindowHostEvent>) {
        if let Some(input) = self.editor.take() {
            out.push(input);
        }
    }

    fn inspect_text_input(&self) -> serde_json::Value {
        self.editor.inspect()
    }
}

impl WindowScene for IosWindow {
    fn viewport(&self, fallback: WindowViewport) -> WindowViewport {
        let Some(viewport) = scene::view_metrics(&self.content) else {
            // UIKit can briefly clear the root view's bounds while showing the window.
            return self.last_viewport.get().unwrap_or(fallback);
        };
        self.last_viewport.set(Some(viewport));
        viewport
    }

    fn keyboard_open(&self) -> bool {
        scene::keyboard_inset(&self.content) > 0.0
    }

    fn sync_back(&mut self, owner: Option<TargetGuard>) {
        self.back.sync(owner);
    }

    fn did_present(&mut self) {
        self.back.did_present();
    }

    fn set_suspended(&mut self, suspended: bool) {
        self.active = !suspended;
        self.refresh_activity();
    }

    fn drain_scene_events(&mut self, out: &mut Vec<WindowHostEvent>) {
        self.refresh_activity();
        self.back.drain(out);
    }
}

fn status_bar_appearance(style: StatusBarStyle, scheme: ColorScheme) -> (UIKitStatusBarStyle, bool) {
    let foreground = match (style, scheme) {
        (StatusBarStyle::Auto, ColorScheme::Dark) | (StatusBarStyle::Light, _) => UIKitStatusBarStyle::LightContent,
        (StatusBarStyle::Auto, ColorScheme::Light) | (StatusBarStyle::Dark, _) => UIKitStatusBarStyle::DarkContent,
        (StatusBarStyle::Hidden, _) => UIKitStatusBarStyle::Default,
    };
    (foreground, style == StatusBarStyle::Hidden)
}

fn read_clipboard(accept: ClipboardKinds) -> Vec<ClipboardItem> {
    let Some(_mtm) = MainThreadMarker::new() else { return Vec::new() };
    let pasteboard = UIPasteboard::generalPasteboard();
    let mut items = Vec::new();
    // SAFETY: These pasteboard reads are on the main thread.
    unsafe {
        if accept.contains(ClipboardKinds::TEXT)
            && let Some(texts) = pasteboard.strings()
        {
            items.extend(texts.iter().map(|text| ClipboardItem::Text(text.to_string())));
        }
        if accept.contains(ClipboardKinds::IMAGE)
            && let Some(images) = pasteboard.images()
        {
            items.extend(images.iter().filter_map(|image| {
                Some(ClipboardItem::Image { media_type: "image/png".into(), bytes: image.png_representation()?.to_vec() })
            }));
        }
        if accept.contains(ClipboardKinds::FILES)
            && let Some(urls) = pasteboard.URLs()
        {
            let paths: Vec<_> = urls.iter().filter(|url| url.isFileURL()).filter_map(|url| url.to_file_path()).collect();
            if !paths.is_empty() {
                items.push(ClipboardItem::Files(paths));
            }
        }
    }
    items
}

fn has_paste_items() -> bool {
    // SAFETY: UIKit asks the responder to validate actions on the main thread.
    unsafe {
        let pasteboard = UIPasteboard::generalPasteboard();
        pasteboard.hasStrings() || pasteboard.hasImages() || pasteboard.hasURLs()
    }
}

fn write_clipboard(items: &[ClipboardItem]) -> Result<(), ServiceError> {
    let _mtm = MainThreadMarker::new().ok_or(ServiceError::Unavailable)?;
    let mut entries = Vec::new();
    for item in items {
        match item {
            ClipboardItem::Text(text) => entries
                .push(NSDictionary::from_slices(&[&*NSString::from_str("public.utf8-plain-text")], &[NSString::from_str(text).as_ref()])),
            ClipboardItem::Image { media_type, bytes } if media_type == "image/png" => {
                entries.push(NSDictionary::from_slices(&[&*NSString::from_str("public.png")], &[NSData::with_bytes(bytes).as_ref()]));
            }
            ClipboardItem::Files(paths) if !paths.is_empty() => {
                let kind = NSString::from_str("public.file-url");
                for path in paths {
                    let url = NSURL::from_file_path(path).ok_or(ServiceError::Failed)?;
                    entries.push(NSDictionary::from_slices(&[&*kind], &[url.as_ref()]));
                }
            }
            _ => return Err(ServiceError::Unavailable),
        }
    }
    if entries.is_empty() {
        return Err(ServiceError::Unavailable);
    }
    // SAFETY: UIKit pasteboard access is on the main thread. Values match their declared pasteboard types.
    unsafe { UIPasteboard::generalPasteboard().setItems(&NSArray::from_retained_slice(&entries)) };
    Ok(())
}
