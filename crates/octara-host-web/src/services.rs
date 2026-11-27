//! Browser prompts and services. Start user-activation-sensitive requests in the initiating turn.

use crate::{Driver, Listener};
use js_sys::Uint8Array;
use octara::runtime::{HostCommand, WindowId};
use octara_model::ask::{Answer, Ask, AskId, Menu, MenuItem, PickedFile, ServiceError, ServiceId, ServiceOutcome, ServiceRequest};
use octara_model::input::{Chord, HostReply, KeyEvent, Modifiers};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{Clipboard, Document, DomException, Event, EventTarget, File, HtmlDialogElement, HtmlElement, HtmlInputElement, Window};

pub(crate) struct Services {
    browser: Window,
    document: Document,
    mount: HtmlElement,
    owner: Weak<Driver>,
    asks: BTreeMap<AskId, Dialog>,
    picker: Option<(ServiceId, Picker)>,
    menus: Vec<Menu>,
    primary: Modifiers,
}

pub(crate) fn shortcut_modifier(browser: &Window) -> Modifiers {
    if browser.navigator().platform().unwrap_or_default().starts_with("Mac") { Modifiers::META } else { Modifiers::CTRL }
}

impl Services {
    pub(crate) fn new(browser: Window, document: Document, mount: HtmlElement, owner: Weak<Driver>) -> Self {
        let primary = shortcut_modifier(&browser);
        Self { browser, document, mount, owner, asks: BTreeMap::new(), picker: None, menus: Vec::new(), primary }
    }

    pub(crate) fn apply(&mut self, command: HostCommand) {
        match command {
            HostCommand::Menus(menus) => self.menus = menus,
            HostCommand::SetWindowTitle { title, .. } => self.document.set_title(&title),
            HostCommand::FocusWindow { .. } => {
                let _ = self.browser.focus();
            }
            HostCommand::SetWindowGeometry { geometry, .. } => match geometry.fullscreen {
                Some(true) => {
                    let _ = self.mount.request_fullscreen();
                }
                Some(false) => self.document.exit_fullscreen(),
                None => {}
            },
            HostCommand::MinimizeWindow { .. } | HostCommand::ToggleMaximizeWindow { .. } | HostCommand::StartWindowDrag { .. } => {}
            HostCommand::Ask { window, id, ask } => {
                if self.show_ask(window, id, &ask).is_err() {
                    self.reply(window, HostReply::AskAnswer { id, answer: Answer::dismissed(&ask) });
                }
            }
            HostCommand::WithdrawAsk { id, .. } => {
                self.asks.remove(&id);
            }
            HostCommand::OpenUrl { url } => {
                let _ = self.browser.open_with_url_and_target(&url, "_self");
            }
            HostCommand::Service { window, id, request } => self.request(window, id, request),
            HostCommand::Opened(_) | HostCommand::Closed(_) | HostCommand::NativeCommand { .. } => {}
        }
    }

    pub(crate) fn stop(&mut self) {
        self.asks.clear();
        self.picker.take();
        self.menus.clear();
    }

    pub(crate) fn shortcut(&self, event: KeyEvent) -> Option<(String, Chord)> {
        fn locate(menu: &Menu, event: KeyEvent, primary: Modifiers) -> Option<(String, Chord)> {
            menu.items.iter().find_map(|item| match item {
                MenuItem::Action { id, enabled: true, shortcut: Some(shortcut), .. } => {
                    let chord: Chord = shortcut.parse().ok()?;
                    chord.matches(event, primary).then(|| (id.clone(), chord))
                }
                MenuItem::Submenu { menu, enabled: true } => locate(menu, event, primary),
                _ => None,
            })
        }
        self.menus.iter().find_map(|menu| locate(menu, event, self.primary))
    }

    fn reply(&self, window: WindowId, reply: HostReply) {
        let owner = self.owner.clone();
        spawn_local(async move { deliver(&owner, window, reply) });
    }

    fn show_ask(&mut self, window: WindowId, id: AskId, ask: &Ask) -> Result<(), JsValue> {
        let element: HtmlDialogElement = self.document.create_element("dialog")?.unchecked_into();
        let mut dialog = Dialog { element, listeners: Vec::new() };
        let dismiss = Answer::dismissed(ask);
        let owner = self.owner.clone();
        dialog.listen(dialog.element.clone().as_ref(), "close", move |_| {
            queue_answer(owner.clone(), window, id, dismiss.clone());
        })?;
        match ask {
            Ask::Prompt { message, detail, answers, .. } => {
                dialog.element.set_attribute("aria-label", message)?;
                let message_element = self.document.create_element("p")?;
                message_element.set_text_content(Some(message));
                dialog.element.append_child(&message_element)?;
                if let Some(detail) = detail {
                    let detail_element = self.document.create_element("p")?;
                    detail_element.set_text_content(Some(detail));
                    dialog.element.append_child(&detail_element)?;
                }
                for (index, label) in answers.iter().enumerate() {
                    self.answer_button(&mut dialog, window, id, label, Answer::Prompt(Some(index)), true)?;
                }
            }
            Ask::Menu { menu, at } => {
                dialog.element.set_attribute("aria-label", &menu.name)?;
                let origin = self.mount.get_bounding_client_rect();
                dialog.element.set_attribute(
                    "style",
                    &format!(
                        "position:fixed;margin:0;inset:{}px auto auto {}px;",
                        origin.y() + f64::from(at.y),
                        origin.x() + f64::from(at.x)
                    ),
                )?;
                self.menu_items(&mut dialog, window, id, menu, true)?;
                dialog.listen(dialog.element.clone().as_ref(), "click", |event| {
                    if let Some(target) = event.target().and_then(|target| target.dyn_into::<HtmlDialogElement>().ok()) {
                        target.close();
                    }
                })?;
            }
            Ask::Paths(_) | Ask::NewPath { .. } => return Err("browser paths are unavailable".into()),
        }
        if dialog.element.child_element_count() == 0 {
            return Err("the browser ask has no visible items".into());
        }
        self.document.body().ok_or("Octara browser prompts need a body")?.append_child(&dialog.element)?;
        dialog.element.show_modal()?;
        self.asks.insert(id, dialog);
        Ok(())
    }

    fn menu_items(&self, dialog: &mut Dialog, window: WindowId, id: AskId, menu: &Menu, enabled: bool) -> Result<(), JsValue> {
        for item in &menu.items {
            match item {
                MenuItem::Action { id: action, label, enabled: item_enabled, .. } => {
                    self.answer_button(dialog, window, id, label, Answer::Menu(Some(action.clone())), enabled && *item_enabled)?;
                }
                MenuItem::Submenu { menu, enabled: item_enabled } => {
                    self.menu_items(dialog, window, id, menu, enabled && *item_enabled)?;
                }
                MenuItem::Separator => {
                    let line = self.document.create_element("hr")?;
                    dialog.element.append_child(&line)?;
                }
            }
        }
        Ok(())
    }

    fn answer_button(
        &self,
        dialog: &mut Dialog,
        window: WindowId,
        id: AskId,
        label: &str,
        answer: Answer,
        enabled: bool,
    ) -> Result<(), JsValue> {
        let button = self.document.create_element("button")?;
        button.set_attribute("type", "button")?;
        button.set_text_content(Some(label));
        if !enabled {
            button.set_attribute("disabled", "")?;
        }
        dialog.element.append_child(&button)?;
        let owner = self.owner.clone();
        dialog.listen(button.as_ref(), "click", move |_| queue_answer(owner.clone(), window, id, answer.clone()))?;
        Ok(())
    }

    fn request(&mut self, window: WindowId, id: ServiceId, request: ServiceRequest) {
        match request {
            ServiceRequest::PickFile { max_bytes, accept } => {
                if let Err(error) = self.pick_file(window, id, max_bytes, &accept) {
                    self.reply(window, HostReply::ServiceReply { id, outcome: ServiceOutcome::Error(error) });
                }
            }
            request => {
                let Some(clipboard) = self.clipboard() else {
                    self.reply(window, HostReply::ServiceReply { id, outcome: ServiceOutcome::Error(ServiceError::Unavailable) });
                    return;
                };
                let (promise, reading) = match request {
                    ServiceRequest::ReadClipboardText => (clipboard.read_text(), true),
                    ServiceRequest::WriteClipboardText { text } => (clipboard.write_text(&text), false),
                    ServiceRequest::PickFile { .. } => unreachable!(),
                };
                let owner = self.owner.clone();
                spawn_local(async move {
                    let outcome = match JsFuture::from(promise).await {
                        Ok(value) if reading => {
                            value.as_string().map_or(ServiceOutcome::Error(ServiceError::Failed), ServiceOutcome::ClipboardText)
                        }
                        Ok(_) => ServiceOutcome::ClipboardWritten,
                        Err(error) => ServiceOutcome::Error(browser_error(error)),
                    };
                    deliver(&owner, window, HostReply::ServiceReply { id, outcome });
                });
            }
        }
    }

    fn clipboard(&self) -> Option<Clipboard> {
        if !self.browser.is_secure_context() {
            return None;
        }
        let clipboard = self.browser.navigator().clipboard();
        let value: &JsValue = clipboard.as_ref();
        (!value.is_undefined() && !value.is_null()).then_some(clipboard)
    }

    fn pick_file(&mut self, window: WindowId, id: ServiceId, max_bytes: usize, accept: &str) -> Result<(), ServiceError> {
        if self.picker.is_some() {
            return Err(ServiceError::Busy);
        }
        let body = self.document.body().ok_or(ServiceError::Unavailable)?;
        let input = self.document.create_element("input").map_err(browser_error)?.unchecked_into::<HtmlInputElement>();
        input.set_type("file");
        input.set_accept(accept);
        input.set_hidden(true);
        input.set_attribute("data-octara-file-picker", "").map_err(browser_error)?;
        let finished = Rc::new(Cell::new(false));
        let changed = finished.clone();
        let owner = self.owner.clone();
        let change = register(input.as_ref(), "change", move |event| {
            if changed.replace(true) {
                return;
            }
            let file = event
                .target()
                .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
                .and_then(|input| input.files())
                .and_then(|files| files.item(0));
            let owner = owner.clone();
            spawn_local(async move {
                let outcome = match file {
                    Some(file) => {
                        read_file(file, max_bytes).await.map_or_else(ServiceOutcome::Error, |file| ServiceOutcome::FilePicked(Some(file)))
                    }
                    None => ServiceOutcome::FilePicked(None),
                };
                finish_picker(&owner, window, id, outcome);
            });
        })
        .map_err(browser_error)?;
        let owner = self.owner.clone();
        let cancel = register(input.as_ref(), "cancel", move |_| {
            if !finished.replace(true) {
                let owner = owner.clone();
                spawn_local(async move { finish_picker(&owner, window, id, ServiceOutcome::FilePicked(None)) });
            }
        })
        .map_err(browser_error)?;
        let picker = Picker { input, _listeners: [change, cancel] };
        body.append_child(&picker.input).map_err(browser_error)?;
        let method = js_sys::Reflect::get(picker.input.as_ref(), &JsValue::from_str("showPicker")).map_err(browser_error)?;
        if !method.is_function() {
            return Err(ServiceError::Unavailable);
        }
        picker.input.show_picker().map_err(|error| {
            if error.dyn_ref::<DomException>().is_some_and(|error| error.name() == "NotAllowedError") {
                ServiceError::ActivationRequired
            } else {
                browser_error(error)
            }
        })?;
        self.picker = Some((id, picker));
        Ok(())
    }
}

struct Dialog {
    element: HtmlDialogElement,
    listeners: Vec<Listener>,
}

impl Dialog {
    fn listen(&mut self, target: &EventTarget, name: &'static str, handle: impl Fn(Event) + 'static) -> Result<(), JsValue> {
        self.listeners.push(register(target, name, handle)?);
        Ok(())
    }
}

impl Drop for Dialog {
    fn drop(&mut self) {
        self.listeners.clear();
        if self.element.open() {
            self.element.close();
        }
        self.element.remove();
    }
}

struct Picker {
    input: HtmlInputElement,
    _listeners: [Listener; 2],
}

impl Drop for Picker {
    fn drop(&mut self) {
        self.input.remove();
    }
}

fn register(target: &EventTarget, name: &'static str, handle: impl Fn(Event) + 'static) -> Result<Listener, JsValue> {
    let callback = Closure::new(handle);
    target.add_event_listener_with_callback(name, callback.as_ref().unchecked_ref())?;
    Ok(Listener { target: target.clone(), name, capture: false, callback })
}

fn queue_answer(owner: Weak<Driver>, window: WindowId, id: AskId, answer: Answer) {
    spawn_local(async move {
        let Some(driver) = owner.upgrade() else { return };
        let removed = driver.services.borrow_mut().asks.remove(&id);
        if removed.is_some() {
            drop(removed);
            deliver(&owner, window, HostReply::AskAnswer { id, answer });
        }
    });
}

fn finish_picker(owner: &Weak<Driver>, window: WindowId, id: ServiceId, outcome: ServiceOutcome) {
    let Some(driver) = owner.upgrade() else { return };
    let finished = driver.services.borrow_mut().picker.take_if(|(pending, _)| *pending == id);
    if finished.is_some() {
        drop(finished);
        deliver(owner, window, HostReply::ServiceReply { id, outcome });
    }
}

fn deliver(owner: &Weak<Driver>, window: WindowId, reply: HostReply) {
    if let Some(driver) = owner.upgrade().filter(|driver| driver.window.get() == Some(window)) {
        driver.send(octara::runtime::Input::Host(reply));
    }
}

async fn read_file(file: File, max_bytes: usize) -> Result<PickedFile, ServiceError> {
    let size = file.size();
    if size > max_bytes as f64 {
        return Err(ServiceError::TooLarge { bytes: size as u64, max_bytes });
    }
    let buffer = JsFuture::from(file.array_buffer()).await.map_err(browser_error)?;
    Ok(PickedFile { name: file.name(), media_type: file.type_(), bytes: Uint8Array::new(&buffer).to_vec() })
}

fn browser_error(value: JsValue) -> ServiceError {
    match value.dyn_ref::<DomException>().map(DomException::name).as_deref() {
        Some("NotAllowedError" | "SecurityError") => ServiceError::Denied,
        Some("AbortError") => ServiceError::Cancelled,
        Some("NotSupportedError") => ServiceError::Unavailable,
        _ => ServiceError::Failed,
    }
}
