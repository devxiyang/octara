//! UIKit scene, native text editing, navigation, and platform services.

mod back;
mod draft;
mod editor;
mod native;
mod scene;

use crate::ShellEvent;
use crate::os::{
    ActiveEditorView, KeyDispatch, OsHost, OsWindow, OsWindowContext, ServiceCleanup, WindowHostEvent, WindowNativeViews, WindowScene,
    WindowTextInput, reply,
};
use back::BackNavigation;
use editor::TextInput;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_foundation::{NSDictionary, NSString, NSURL};
use objc2_ui_kit::{UIApplication, UIPasteboard, UIView};
use octara::runtime::{Input, WindowId};
use octara_model::ask::{ServiceId, ServiceOutcome, ServiceRequest};
use octara_model::geometry::{Size, WindowViewport};
use octara_model::identity::TargetGuard;
use octara_model::input::{HostReply, InteractionEvent};
use octara_model::window::{WindowGeometry, WindowGeometryRequest};
use scene::SceneConnection;
use std::cell::Cell;
use std::rc::Rc;
use winit::event::KeyEvent;
use winit::event_loop::EventLoopProxy;
use winit::keyboard::PhysicalKey;
use winit::window::Window;

pub(super) fn create() -> Box<dyn OsHost> {
    Box::new(IosHost)
}

struct IosHost;

impl OsHost for IosHost {
    fn initialize(&mut self, proxy: EventLoopProxy<ShellEvent>) {
        super::scheduling::interactive();
        scene::register(proxy);
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
        let native = native::Views::new(content.clone(), id, proxy.clone());
        Ok(Box::new(IosWindow { content, scene, editor, back, native, proxy, id, scene_active, active: true, focused: true }))
    }

    fn open_url(&mut self, url: &str) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else { return };
        // SAFETY: The empty options dictionary has the types required by UIKit.
        unsafe { UIApplication::sharedApplication(mtm).openURL_options_completionHandler(&url, &NSDictionary::new(), None) };
    }

    fn read_clipboard_text(&mut self) -> Option<String> {
        clipboard_text()
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
    native: native::Views,
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

    fn set_window_title(&mut self, _window: &Window, _title: &str) {}

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

    fn native_views(&mut self) -> Option<&mut dyn WindowNativeViews> {
        Some(&mut self.native)
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
            ServiceRequest::ReadClipboardText => ServiceOutcome::ClipboardText(clipboard_text().unwrap_or_default()),
            ServiceRequest::WriteClipboardText { text } => {
                write_clipboard_text(&text);
                ServiceOutcome::ClipboardWritten
            }
            ServiceRequest::PickFile { .. } => return None,
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
        if let Some(event) = self.editor.take() {
            out.push(Input::Editor(event));
        }
    }

    fn inspect_text_input(&self) -> serde_json::Value {
        self.editor.inspect()
    }
}

impl WindowScene for IosWindow {
    fn viewport(&self, viewport: WindowViewport) -> WindowViewport {
        scene::viewport(&self.content, self.editor.keyboard_top(), viewport)
    }

    fn keyboard_open(&self) -> bool {
        self.editor.keyboard_top().is_some()
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

fn clipboard_text() -> Option<String> {
    let _mtm = MainThreadMarker::new()?;
    // SAFETY: UIKit pasteboard access is on the main thread.
    unsafe { UIPasteboard::generalPasteboard().string() }.map(|value| value.to_string())
}

fn write_clipboard_text(text: &str) {
    if MainThreadMarker::new().is_some() {
        // SAFETY: UIKit pasteboard access is on the main thread with a valid NSString.
        unsafe { UIPasteboard::generalPasteboard().setString(Some(&NSString::from_str(text))) };
    }
}
