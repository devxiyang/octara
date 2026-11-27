//! UIKit scene, native text editing, navigation, and platform services.

mod back;
mod draft;
mod editor;
mod gestures;
mod scene;

use crate::ShellEvent;
use crate::os::{
    ActiveEditorView, KeyDispatch, OsHost, OsWindow, OsWindowContext, ServiceCleanup, WindowHostEvent, WindowScene, WindowTextInput, reply,
};
use back::BackNavigation;
use editor::TextInput;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_foundation::{NSCurrentLocaleDidChangeNotification, NSDictionary, NSNotificationCenter, NSString, NSURL};
use objc2_ui_kit::{
    UIAccessibilityDarkerSystemColorsEnabled, UIAccessibilityDarkerSystemColorsStatusDidChangeNotification,
    UIAccessibilityIsReduceMotionEnabled, UIAccessibilityReduceMotionStatusDidChangeNotification, UIApplication,
    UIContentSizeCategoryDidChangeNotification, UIFont, UIFontTextStyleBody, UIPasteboard, UIUserInterfaceStyle, UIView,
};
use octara::module::{HostSurfaceKind, RawNativeSurface};
use octara::runtime::{Input, WindowId};
use octara_model::ask::{ClipboardItem, ClipboardKinds, ServiceId, ServiceOutcome, ServiceRequest};
use octara_model::geometry::{Size, WindowViewport};
use octara_model::identity::TargetGuard;
use octara_model::input::{HostReply, InteractionEvent};
use octara_model::theme::ColorScheme;
use octara_model::window::{WindowGeometry, WindowGeometryRequest, WindowMaterial};
use scene::SceneConnection;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use winit::event::KeyEvent;
use winit::event_loop::EventLoopProxy;
use winit::keyboard::PhysicalKey;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

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
        let OsWindowContext { window, id, proxy, .. } = context;
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
        Ok(Box::new(IosWindow { content, scene, editor, back, _gestures: gestures, proxy, id, scene_active, active: true, focused: true }))
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

    fn write_clipboard_text(&mut self, text: &str) {
        write_clipboard_text(text);
    }
}

struct IosWindow {
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
}

impl IosWindow {
    fn refresh_activity(&mut self) {
        let active = self.active && self.focused && self.scene_active.get();
        self.editor.set_active(active);
        self.back.set_active(active);
    }
}

impl OsWindow for IosWindow {
    fn window_geometry(&mut self, window: &Window, viewport: WindowViewport) -> WindowGeometry {
        let bounds = self.content.bounds().size;
        let size =
            if bounds.width > 0.0 && bounds.height > 0.0 { Size::new(bounds.width as f32, bounds.height as f32) } else { viewport.size };
        WindowGeometry { size, scale: viewport.scale, fullscreen: window.fullscreen().is_some(), ..Default::default() }
    }

    fn set_window_geometry(&mut self, _window: &Window, _geometry: WindowGeometryRequest) {}

    fn apply_presentation(&mut self, _window: &Window, title: &str, color_scheme: ColorScheme, _material: WindowMaterial) {
        self.content.setOverrideUserInterfaceStyle(match color_scheme {
            ColorScheme::Light => UIUserInterfaceStyle::Light,
            ColorScheme::Dark => UIUserInterfaceStyle::Dark,
        });
        if let Some(scene) = self.content.window().and_then(|window| window.windowScene()) {
            scene.setTitle(Some(&NSString::from_str(title)));
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

    fn show(&mut self, _: &Window) {
        self.scene.show();
        self.editor.focus();
    }

    fn request_service(&mut self, id: ServiceId, request: ServiceRequest) -> Option<ServiceCleanup> {
        let outcome = match request {
            ServiceRequest::ReadClipboard { accept } => ServiceOutcome::Clipboard(read_clipboard(accept)),
            ServiceRequest::WriteClipboardText { text } => {
                write_clipboard_text(&text);
                ServiceOutcome::ClipboardWritten
            }
        };
        reply(&self.proxy, self.id, HostReply::ServiceReply { id, outcome });
        Some(Box::new(|| {}))
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
    fn viewport(&self, viewport: WindowViewport) -> WindowViewport {
        scene::viewport(&self.content, self.editor.keyboard_inset(), viewport)
    }

    fn keyboard_open(&self) -> bool {
        self.editor.keyboard_inset() > 0.0
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

fn write_clipboard_text(text: &str) {
    if MainThreadMarker::new().is_some() {
        // SAFETY: UIKit pasteboard access is on the main thread with a valid NSString.
        unsafe { UIPasteboard::generalPasteboard().setString(Some(&NSString::from_str(text))) };
    }
}
