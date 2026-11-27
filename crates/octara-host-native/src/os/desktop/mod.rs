//! Portable desktop window integration for Windows and Linux.

mod ime;
#[cfg(target_os = "linux")]
mod linux_preferences;
mod services;
#[cfg(target_os = "windows")]
mod windows_preferences;

use crate::ShellEvent;
use crate::os::{ActiveEditorView, OsHost, OsWindow, OsWindowContext, ServiceCleanup, WindowAsks, WindowTextInput};
use ime::WinitIme;
use octara::runtime::Input;
use octara_model::ask::{Ask, AskId, ServiceId, ServiceRequest};
use octara_model::input::{EditorEvent, HostReply};
use octara_model::paint::Image;
use serde_json::Value;
use services::{Clipboard, Services};
use std::rc::Rc;
use std::sync::Arc;
use winit::event::Ime;
use winit::window::Window;

pub(super) fn create() -> Box<dyn OsHost> {
    Box::new(DesktopHost {
        clipboard: Rc::new(Clipboard::new()),
        #[cfg(target_os = "linux")]
        preferences: None,
    })
}

struct DesktopHost {
    clipboard: Rc<Clipboard>,
    #[cfg(target_os = "linux")]
    preferences: Option<linux_preferences::Observer>,
}

impl OsHost for DesktopHost {
    #[cfg(target_os = "linux")]
    fn initialize(&mut self, proxy: winit::event_loop::EventLoopProxy<ShellEvent>) {
        self.preferences = linux_preferences::Observer::start(proxy).ok();
    }

    #[cfg(target_os = "windows")]
    fn reduced_motion(&self) -> Option<bool> {
        windows_preferences::reduced_motion()
    }

    fn create_window(&mut self, context: OsWindowContext<'_>) -> Result<Box<dyn OsWindow>, String> {
        #[cfg(target_os = "windows")]
        let preferences = windows_preferences::Observer::new(context.window, context.proxy.clone());
        let reply = move |reply| {
            let _ = context.proxy.send_event(ShellEvent::Input { window: context.id, input: Input::Host(reply) });
        };
        Ok(Box::new(DesktopWindow {
            #[cfg(target_os = "windows")]
            _preferences: preferences,
            window: context.window.clone(),
            ime: WinitIme::default(),
            services: Services::new(self.clipboard.clone(), reply),
        }))
    }

    fn open_url(&mut self, url: &str) {
        let _ = open::that_detached(url);
    }

    fn read_clipboard(&mut self, accept: octara_model::ask::ClipboardKinds) -> Option<Vec<octara_model::ask::ClipboardItem>> {
        self.clipboard.with_native(|native| services::read_clipboard(native, accept))?.ok()
    }

    fn write_clipboard_text(&mut self, text: &str) {
        let _ = self.clipboard.with_native(|clipboard| clipboard.set_text(text));
    }

    fn write_clipboard_image(&mut self, image: &Image) {
        let frame = image.frame();
        let data = arboard::ImageData { width: frame.width as usize, height: frame.height as usize, bytes: (&*frame.rgba).into() };
        let _ = self.clipboard.with_native(|clipboard| clipboard.set_image(data));
    }
}

struct DesktopWindow {
    #[cfg(target_os = "windows")]
    _preferences: Option<windows_preferences::Observer>,
    window: Arc<Window>,
    ime: WinitIme,
    services: Services,
}

impl OsWindow for DesktopWindow {
    fn text_input(&mut self) -> &mut dyn WindowTextInput {
        self
    }

    fn asks(&mut self) -> Option<&mut dyn WindowAsks> {
        Some(self)
    }

    fn request_service(&mut self, id: ServiceId, request: ServiceRequest) -> Option<ServiceCleanup> {
        self.services.request(&self.window, id, request)
    }
}

impl WindowTextInput for DesktopWindow {
    fn sync_text_input(&mut self, editor: Option<&ActiveEditorView<'_>>) {
        self.ime.sync(&self.window, editor);
    }

    fn on_winit_ime(&mut self, event: &Ime) -> Option<EditorEvent> {
        self.ime.event(event)
    }

    fn inspect_text_input(&self) -> Value {
        self.ime.inspect(&self.window)
    }
}

impl WindowAsks for DesktopWindow {
    fn show_ask(&mut self, id: AskId, ask: Ask) -> Option<Ask> {
        match &ask {
            Ask::Paths(options) if options.files != options.directories => {}
            Ask::NewPath { .. } => {}
            _ => return Some(ask),
        }
        self.services.show_ask(&self.window, id, ask);
        None
    }

    fn withdraw_ask(&mut self, id: AskId) {
        self.services.withdraw_ask(id);
    }
}

type Reply = Arc<dyn Fn(HostReply) + Send + Sync>;
