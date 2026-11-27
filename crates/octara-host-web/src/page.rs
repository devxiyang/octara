//! Browser theme color and the page's synchronous close decision.

use crate::{Driver, css};
use octara_model::theme::Color;
use std::cell::{Cell, RefCell};
use std::rc::Weak;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{BeforeUnloadEvent, Document, Element, Window};

pub(crate) struct Page {
    window: Window,
    before_unload: Closure<dyn Fn(BeforeUnloadEvent)>,
    watching: Cell<bool>,
    theme: RefCell<Option<ThemeColor>>,
}

impl Page {
    pub(crate) fn new(window: Window, driver: Weak<Driver>) -> Self {
        let before_unload = Closure::new(move |event: BeforeUnloadEvent| {
            let Some(driver) = driver.upgrade() else { return };
            match driver.check_page_close() {
                Ok(false) => {
                    event.prevent_default();
                    event.set_return_value(" ");
                }
                Ok(true) => {}
                Err(error) => driver.report_error(error),
            }
        });
        Self { window, before_unload, watching: Cell::default(), theme: RefCell::default() }
    }

    pub(crate) fn watch_close(&self, watching: bool) -> Result<(), JsValue> {
        if self.watching.get() == watching {
            return Ok(());
        }
        let callback = self.before_unload.as_ref().unchecked_ref();
        if watching {
            self.window.add_event_listener_with_callback("beforeunload", callback)?;
        } else {
            self.window.remove_event_listener_with_callback("beforeunload", callback)?;
        }
        self.watching.set(watching);
        Ok(())
    }

    pub(crate) fn set_theme_color(&self, document: &Document, background: Color) -> Result<(), JsValue> {
        let value = css::color(background);
        let mut theme = self.theme.borrow_mut();
        if let Some(active) = theme.as_mut().filter(|active| active.element.is_connected()) {
            return active.update(value);
        }
        let Some(head) = document.head() else { return Ok(()) };
        let first = head.query_selector("meta[name='theme-color' i]")?;
        let (element, inserted) = if let Some(meta) = first.filter(|meta| !meta.has_attribute("media")) {
            (meta, false)
        } else {
            let meta = document.create_element("meta")?;
            meta.set_attribute("name", "theme-color")?;
            head.insert_before(&meta, head.first_child().as_ref())?;
            (meta, true)
        };
        let original = element.get_attribute("content");
        let mut applied = ThemeColor { element, original, inserted, written: String::new() };
        applied.update(value)?;
        *theme = Some(applied);
        Ok(())
    }

    pub(crate) fn stop(&self) {
        let _ = self.watch_close(false);
        self.theme.take();
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        self.stop();
    }
}

struct ThemeColor {
    element: Element,
    original: Option<String>,
    inserted: bool,
    written: String,
}

impl ThemeColor {
    fn update(&mut self, value: String) -> Result<(), JsValue> {
        if self.element.get_attribute("content").as_deref() != Some(value.as_str()) {
            self.element.set_attribute("content", &value)?;
        }
        self.written = value;
        Ok(())
    }
}

impl Drop for ThemeColor {
    fn drop(&mut self) {
        // A page script can take ownership of the tag while the app is mounted.
        if self.element.get_attribute("content").as_deref() != Some(self.written.as_str()) {
            return;
        }
        if self.inserted {
            self.element.remove();
        } else if let Some(original) = &self.original {
            let _ = self.element.set_attribute("content", original);
        } else {
            let _ = self.element.remove_attribute("content");
        }
    }
}
