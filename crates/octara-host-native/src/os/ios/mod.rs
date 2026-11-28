//! UIKit application, window, and platform services.

mod a11y;
mod app;
mod back;
mod editor;
mod gestures;
mod scene;
mod view;

use super::apple_frame_clock::{DisplayClock, retry_on_clock};
use super::apple_media::encode_oriented_png;
use super::apple_metal::set_transactional;
use crate::os::{
    ActiveEditorView, Drawable, DrawableAcquisition, FrameClockActivity, HostWake, OsHost, OsWindow, OsWindowContext, PlatformEvent,
    PlatformKey, PlatformSender, ServiceCleanup, ShellDriver, WindowAccessibility, WindowHostEvent, WindowPresentation, WindowScene,
    WindowTextInput, reply, surface_drawable,
};
use back::BackNavigation;
use editor::{ContentView, TextInput};
use objc2::rc::{Retained, Weak};
use objc2::{ClassType, MainThreadMarker};
use objc2_foundation::{
    NSArray, NSCurrentLocaleDidChangeNotification, NSData, NSDictionary, NSNotificationCenter, NSRect, NSString, NSURL,
};
use objc2_quartz_core::{CAMetalLayer, CATransaction};
use objc2_ui_kit::{
    UIAccessibilityDarkerSystemColorsEnabled, UIAccessibilityDarkerSystemColorsStatusDidChangeNotification,
    UIAccessibilityIsReduceMotionEnabled, UIAccessibilityReduceMotionStatusDidChangeNotification,
    UIAccessibilitySpeakScreenStatusDidChangeNotification, UIAccessibilitySwitchControlStatusDidChangeNotification,
    UIAccessibilityVoiceOverStatusDidChangeNotification, UIApplication, UIColor, UIContentSizeCategoryDidChangeNotification,
    UICoordinateSpace, UIFont, UIFontTextStyleBody, UIImage, UIImageOrientation, UIPasteboard, UIScreen, UIStatusBarStyle,
    UIUserInterfaceStyle, UIView,
};
use octara::WindowOptions;
use octara::module::{HostSurface, HostSurfaceKind, RawNativeSurface};
use octara::runtime::WindowId;
use octara_model::ask::{ClipboardItem, ClipboardKinds, ServiceError, ServiceId, ServiceOutcome, ServiceRequest};
use octara_model::geometry::WindowViewport;
use octara_model::identity::TargetGuard;
use octara_model::input::{HostReply, Key, Modifiers};
use octara_model::theme::ColorScheme;
use octara_model::window::{StatusBarStyle, WindowGeometry, WindowLevel, WindowMaterial};
use octara_render::{gpu::OutputColor, wgpu};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, UiKitDisplayHandle, UiKitWindowHandle};
use scene::AreaConnection;
use std::cell::Cell;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use view::{Controller, NativeWindow};

pub(super) const SURFACE_KIND: HostSurfaceKind = HostSurfaceKind::Ios;

pub(super) fn create() -> Box<dyn OsHost> {
    Box::new(IosHost { events: Arc::new(app::Queue::default()), preferences: None })
}

struct IosHost {
    events: Arc<app::Queue>,
    preferences: Option<super::apple_preferences::Observer>,
}

impl IosHost {
    fn observe_preferences(&mut self) {
        super::scheduling::interactive();
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
        self.preferences =
            Some(super::apple_preferences::Observer::new(NSNotificationCenter::defaultCenter(), &names, self.events.sender()));
    }
}

impl OsHost for IosHost {
    fn run(self: Box<Self>, driver: Box<dyn ShellDriver>) -> Result<(), String> {
        app::run(self, driver)
    }

    fn wake(&self) -> HostWake {
        self.events.wake()
    }

    fn reduced_motion(&self) -> Option<bool> {
        Some(UIAccessibilityIsReduceMotionEnabled())
    }

    fn locale(&self) -> Option<Arc<str>> {
        super::apple_preferences::locale()
    }

    fn color_scheme(&self) -> Option<ColorScheme> {
        let traits = scene::current()?.traitCollection();
        // SAFETY: The collection belongs to the current UIKit scene.
        let style = unsafe { traits.userInterfaceStyle() };
        match style {
            UIUserInterfaceStyle::Dark => Some(ColorScheme::Dark),
            UIUserInterfaceStyle::Light => Some(ColorScheme::Light),
            _ => None,
        }
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
        Some(scene::foreground())
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

    fn report_error(&self, error: &str) {
        unsafe extern "C" {
            fn NSLog(format: *const NSString, ...);
        }
        let message = NSString::from_str(error);
        let format = NSString::from_str("Octara: %@");
        // SAFETY: The Objective-C string operands are retained through the call.
        unsafe { NSLog(&*format, &*message as *const NSString) };
    }

    fn create_window(&mut self, id: WindowId, options: &WindowOptions, context: OsWindowContext<'_>) -> Result<Box<dyn OsWindow>, String> {
        let main = MainThreadMarker::new().ok_or("UIKit requires the application main thread")?;
        let OsWindowContext { config, backend, sender, transparent } = context;
        let scene = scene::current();
        if !scene::ready() {
            return Err("The UIKit scene is disconnected".into());
        }
        // UIScreen.mainScreen and the scene coordinate space remain available at the iOS 18.4 floor.
        #[allow(deprecated)]
        let screen = scene.as_deref().map_or_else(|| UIScreen::mainScreen(main), objc2_ui_kit::UIWindowScene::screen);
        #[allow(deprecated)]
        let bounds = scene.as_deref().map_or_else(|| screen.bounds(), |scene| scene.coordinateSpace().bounds());
        let window = NativeWindow::new(main, id, sender.clone(), scene.as_deref(), bounds);
        let content = ContentView::new(main, NSRect::new(Default::default(), bounds.size), id, sender.clone());
        content.setContentScaleFactor(screen.scale());
        let (style, hidden) = status_bar_appearance(options.presentation.status_bar, options.presentation.theme.scheme);
        let controller = Controller::new(content.clone(), style, hidden);
        window.setRootViewController(Some(&controller));
        window.setHidden(true);
        window.layoutIfNeeded();
        scene::register(&window);

        let plain_content: Retained<UIView> = Retained::into_super(content.clone());
        let weak_view = Weak::new(&*content);
        let area = AreaConnection::new(
            &content,
            Rc::new(move || {
                if let Some(view) = weak_view.load() {
                    view.refresh_viewport();
                }
            }),
        );
        let editor = TextInput::new(content.clone());
        let back = BackNavigation::new(
            Retained::into_super(window.clone()),
            Retained::into_super(controller.clone()),
            plain_content.clone(),
            sender.clone(),
        );
        let gestures = gestures::Recognizers::new(&plain_content, sender.clone(), id);
        let clock = (backend.contains(wgpu::Backends::METAL) && !config.manual_clock)
            .then(|| DisplayClock::new(&plain_content, id, sender.clone()));
        let accessibility = a11y::Bridge::new(plain_content, id, sender.clone());
        let layer = content.layer().downcast::<CAMetalLayer>().map_err(|_| "UIKit content view has no CAMetalLayer")?;
        let mut host = IosWindow {
            accessibility,
            editor,
            back,
            _gestures: gestures,
            _area: area,
            clock,
            sender,
            id,
            content,
            controller,
            window,
            suspended: !scene::active(),
            layer,
            transactional: false,
            last_viewport: Cell::new(None),
        };
        host.set_transparent(transparent);
        Ok(Box::new(host))
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
    editor: TextInput,
    back: BackNavigation,
    _gestures: gestures::Recognizers,
    _area: AreaConnection,
    clock: Option<DisplayClock>,
    sender: PlatformSender,
    id: WindowId,
    content: Retained<ContentView>,
    controller: Retained<Controller>,
    window: Retained<NativeWindow>,
    suspended: bool,
    layer: Retained<CAMetalLayer>,
    transactional: bool,
    last_viewport: Cell<Option<WindowViewport>>,
}

impl IosWindow {
    fn refresh_activity(&mut self) {
        let active = !self.suspended && self.window.focused();
        self.editor.set_active(active);
        self.back.set_active(active);
    }

    fn view_handle(&self) -> NonNull<std::ffi::c_void> {
        NonNull::from(self.content.as_super()).cast()
    }

    fn controller_handle(&self) -> NonNull<std::ffi::c_void> {
        NonNull::from(self.controller.as_super()).cast()
    }
}

// SAFETY: The content view, controller, and window are retained for the entire
// host window lifetime. Hiding the window leaves the native objects alive.
unsafe impl OsWindow for IosWindow {
    fn raw_handles(&self) -> Result<(RawDisplayHandle, RawWindowHandle), String> {
        let mut handle = UiKitWindowHandle::new(self.view_handle());
        handle.ui_view_controller = Some(self.controller_handle());
        Ok((UiKitDisplayHandle::new().into(), handle.into()))
    }

    fn host_surface(&self) -> HostSurface<'_> {
        let raw = RawNativeSurface::UiKit { ui_view: self.view_handle(), ui_view_controller: self.controller_handle() };
        // SAFETY: These retained UIKit objects outlive the borrowed surface.
        unsafe { HostSurface::new(SURFACE_KIND, Some(self.id)).with_raw_native(raw) }
    }

    fn physical_size(&self) -> [u32; 2] {
        self.content.physical_size()
    }

    fn scale_factor(&self) -> f32 {
        self.content.contentScaleFactor() as f32
    }

    fn accessibility(&mut self) -> Option<&mut dyn WindowAccessibility> {
        Some(&mut self.accessibility)
    }

    fn window_geometry(&mut self, viewport: WindowViewport) -> WindowGeometry {
        WindowGeometry { size: viewport.size, scale: viewport.scale, fullscreen: true, ..Default::default() }
    }

    fn apply_presentation(
        &mut self,
        title: &str,
        color_scheme: ColorScheme,
        _material: WindowMaterial,
        status_bar: StatusBarStyle,
        _level: WindowLevel,
    ) {
        self.window.setOverrideUserInterfaceStyle(match color_scheme {
            ColorScheme::Light => UIUserInterfaceStyle::Light,
            ColorScheme::Dark => UIUserInterfaceStyle::Dark,
        });
        if let Some(scene) = self.window.windowScene() {
            scene.setTitle(Some(&NSString::from_str(title)));
        }
        let (style, hidden) = status_bar_appearance(status_bar, color_scheme);
        self.controller.set_status(style, hidden);
    }

    fn focus(&mut self) {
        self.window.makeKeyWindow();
    }

    fn is_focused(&self) -> bool {
        self.window.focused()
    }

    fn request_redraw(&mut self) {
        self.sender.send_event(PlatformEvent::WindowRedrawRequested(self.id));
    }

    fn set_transparent(&mut self, transparent: bool) {
        self.content.setOpaque(!transparent);
        self.window.setOpaque(!transparent);
        if transparent {
            self.content.setBackgroundColor(Some(&UIColor::clearColor()));
            self.window.setBackgroundColor(Some(&UIColor::clearColor()));
        }
    }

    fn close(&mut self) {
        self.window.hide();
        self.set_suspended(true);
        self.set_frame_clock_activity(FrameClockActivity::Paused);
    }

    fn text_input(&mut self) -> &mut dyn WindowTextInput {
        self
    }

    fn scene(&mut self) -> Option<&mut dyn WindowScene> {
        Some(self)
    }

    fn presentation(&mut self) -> Option<&mut dyn WindowPresentation> {
        Some(self)
    }

    fn show(&mut self) {
        self.window.show();
        self.refresh_activity();
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
        reply(&self.sender, self.id, HostReply::ServiceReply { id, outcome });
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
        set_transactional(&self.layer, false);
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
        set_transactional(&self.layer, self.transactional);
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
        self.editor.set_active(focused && !self.suspended);
        self.back.set_active(focused && !self.suspended);
    }

    fn is_composing(&self) -> bool {
        self.editor.is_composing()
    }

    fn on_unhandled_key(&mut self, key: &PlatformKey) -> bool {
        if !self.editor.is_focused() {
            return false;
        }
        let native = self.editor.is_composing()
            || key.event.is_some_and(|event| {
                !event.modifiers.intersects(Modifiers::META | Modifiers::CTRL)
                    && (matches!(event.key, Key::Char(_) | Key::Space)
                        || event.key == Key::Backspace && !event.modifiers.contains(Modifiers::ALT))
            });
        if native {
            self.window.forward_key_to_uikit();
        }
        native
    }

    fn drain_text_input(&mut self, out: &mut Vec<WindowHostEvent>) {
        self.editor.drain(out);
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
        self.suspended = suspended;
        self.refresh_activity();
    }

    fn drain_scene_events(&mut self, out: &mut Vec<WindowHostEvent>) {
        self.refresh_activity();
        self.back.drain(out);
    }
}

fn status_bar_appearance(style: StatusBarStyle, scheme: ColorScheme) -> (UIStatusBarStyle, bool) {
    let foreground = match (style, scheme) {
        (StatusBarStyle::Auto, ColorScheme::Dark) | (StatusBarStyle::Light, _) => UIStatusBarStyle::LightContent,
        (StatusBarStyle::Auto, ColorScheme::Light) | (StatusBarStyle::Dark, _) => UIStatusBarStyle::DarkContent,
        (StatusBarStyle::Hidden, _) => UIStatusBarStyle::Default,
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
            items.extend(images.iter().filter_map(|image| clipboard_png(&image)));
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

fn clipboard_png(image: &UIImage) -> Option<ClipboardItem> {
    // SAFETY: This helper runs on the main thread as part of a pasteboard read.
    let bytes = if let Some(cg_image) = unsafe { image.CGImage() } {
        let orientation = match unsafe { image.imageOrientation() } {
            UIImageOrientation::UpMirrored => 2,
            UIImageOrientation::Down => 3,
            UIImageOrientation::DownMirrored => 4,
            UIImageOrientation::LeftMirrored => 5,
            UIImageOrientation::Right => 6,
            UIImageOrientation::RightMirrored => 7,
            UIImageOrientation::Left => 8,
            _ => 1,
        };
        encode_oriented_png(&cg_image, orientation).ok()?
    } else {
        image.png_representation()?.to_vec()
    };
    Some(ClipboardItem::Image { media_type: "image/png".into(), bytes })
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
