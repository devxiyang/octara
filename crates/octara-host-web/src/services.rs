//! Browser prompts and services. Start user-activation-sensitive requests in the initiating cycle.

use crate::{Driver, Listener};
use js_sys::Uint8Array;
use octara::runtime::{HostCommand, WindowId};
use octara_model::ask::{
    Answer, Ask, AskId, ClipboardItem, ClipboardKinds, Menu, MenuItem, ServiceError, ServiceId, ServiceOutcome, ServiceRequest,
};
use octara_model::input::{Chord, HostReply, KeyEvent, Modifiers};
use octara_model::theme::ColorScheme;
use std::collections::BTreeMap;
use std::rc::Weak;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{Blob, Clipboard, Document, DomException, Event, EventTarget, HtmlDialogElement, HtmlElement, Window};

pub(crate) struct Services {
    browser: Window,
    document: Document,
    mount: HtmlElement,
    owner: Weak<Driver>,
    asks: BTreeMap<AskId, Dialog>,
    menus: Vec<Menu>,
    primary: Modifiers,
}

pub(crate) fn shortcut_modifier(browser: &Window) -> Modifiers {
    if browser.navigator().platform().unwrap_or_default().starts_with("Mac") { Modifiers::META } else { Modifiers::CTRL }
}

impl Services {
    pub(crate) fn new(browser: Window, document: Document, mount: HtmlElement, owner: Weak<Driver>) -> Self {
        let primary = shortcut_modifier(&browser);
        Self { browser, document, mount, owner, asks: BTreeMap::new(), menus: Vec::new(), primary }
    }

    pub(crate) fn apply(&mut self, command: HostCommand) {
        match command {
            HostCommand::Menus(menus) => self.menus = menus,
            HostCommand::UpdateWindowPresentation { title, color_scheme, .. } => {
                self.document.set_title(&title);
                let scheme = match color_scheme {
                    ColorScheme::Light => "light",
                    ColorScheme::Dark => "dark",
                };
                let _ = self.mount.style().set_property("color-scheme", scheme);
            }
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
            HostCommand::Opened(_)
            | HostCommand::Closed(_)
            | HostCommand::NativeCommand { .. }
            | HostCommand::TypedNativeCommand { .. }
            | HostCommand::SetAppBadge { .. }
            | HostCommand::RequestUserAttention { .. } => {}
        }
    }

    pub(crate) fn stop(&mut self) {
        self.asks.clear();
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
            ServiceRequest::ReadClipboard { accept } if !accept.intersects(ClipboardKinds::TEXT | ClipboardKinds::IMAGE) => {
                self.reply(window, HostReply::ServiceReply { id, outcome: ServiceOutcome::Clipboard(Vec::new()) });
            }
            ServiceRequest::ReadClipboard { accept } => {
                let Some(clipboard) = self.clipboard() else {
                    self.reply(window, HostReply::ServiceReply { id, outcome: ServiceOutcome::Error(ServiceError::Unavailable) });
                    return;
                };
                let multimodal = accept.contains(ClipboardKinds::IMAGE)
                    && js_sys::Reflect::get(clipboard.as_ref(), &JsValue::from_str("read")).is_ok_and(|read| read.is_function());
                if !multimodal && !accept.contains(ClipboardKinds::TEXT) {
                    self.reply(window, HostReply::ServiceReply { id, outcome: ServiceOutcome::Error(ServiceError::Unavailable) });
                    return;
                }
                let promise = if multimodal { clipboard.read() } else { clipboard.read_text() };
                let owner = self.owner.clone();
                spawn_local(async move {
                    let outcome = match JsFuture::from(promise).await {
                        Ok(value) if multimodal => ServiceOutcome::Clipboard(clipboard_items(value, accept).await),
                        Ok(value) => value.as_string().map_or(ServiceOutcome::Error(ServiceError::Failed), |text| {
                            ServiceOutcome::Clipboard(if text.is_empty() { Vec::new() } else { vec![ClipboardItem::Text(text)] })
                        }),
                        Err(error) => ServiceOutcome::Error(browser_error(error)),
                    };
                    deliver(&owner, window, HostReply::ServiceReply { id, outcome });
                });
            }
            ServiceRequest::WriteClipboardText { text } => {
                let Some(clipboard) = self.clipboard() else {
                    self.reply(window, HostReply::ServiceReply { id, outcome: ServiceOutcome::Error(ServiceError::Unavailable) });
                    return;
                };
                let promise = clipboard.write_text(&text);
                let owner = self.owner.clone();
                spawn_local(async move {
                    let outcome = match JsFuture::from(promise).await {
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

fn deliver(owner: &Weak<Driver>, window: WindowId, reply: HostReply) {
    if let Some(driver) = owner.upgrade().filter(|driver| driver.window.get() == Some(window)) {
        driver.send(octara::runtime::Input::Host(reply));
    }
}

async fn clipboard_items(value: JsValue, accept: ClipboardKinds) -> Vec<ClipboardItem> {
    let mut items = Vec::new();
    for item in js_sys::Array::from(&value).iter().filter_map(|item| item.dyn_into::<web_sys::ClipboardItem>().ok()) {
        let types = item.types();
        if accept.contains(ClipboardKinds::TEXT)
            && types.includes(&JsValue::from_str("text/plain"), 0)
            && let Ok(blob) = JsFuture::from(item.get_type("text/plain")).await
            && let Ok(blob) = blob.dyn_into::<Blob>()
            && let Ok(text) = JsFuture::from(blob.text()).await
            && let Some(text) = text.as_string()
        {
            items.push(ClipboardItem::Text(text));
        }
        if accept.contains(ClipboardKinds::IMAGE) {
            for media_type in types.iter().filter_map(|kind| kind.as_string()).filter(|kind| kind.starts_with("image/")) {
                if let Ok(blob) = JsFuture::from(item.get_type(&media_type)).await
                    && let Ok(blob) = blob.dyn_into::<Blob>()
                    && let Some(image) = clipboard_image(&blob, media_type).await
                {
                    items.push(image);
                    break;
                }
            }
        }
    }
    items
}

pub(crate) async fn clipboard_image(blob: &Blob, media_type: String) -> Option<ClipboardItem> {
    let buffer = JsFuture::from(blob.array_buffer()).await.ok()?;
    Some(ClipboardItem::Image { media_type, bytes: Uint8Array::new(&buffer).to_vec() })
}

fn browser_error(value: JsValue) -> ServiceError {
    match value.dyn_ref::<DomException>().map(DomException::name).as_deref() {
        Some("NotAllowedError" | "SecurityError") => ServiceError::Denied,
        Some("AbortError") => ServiceError::Cancelled,
        Some("NotSupportedError") => ServiceError::Unavailable,
        _ => ServiceError::Failed,
    }
}
