//! The opt-in browser bridge exposes one JSON-lines exchange as `window.__octaraInspect`.

use super::InspectTransport;
use js_sys::{Object, Reflect};
use wasm_bindgen::{JsValue, closure::Closure};

pub struct WebTransport {
    window: Object,
    key: JsValue,
    exchange: Closure<dyn FnMut(String) -> String>,
}

impl WebTransport {
    /// Use strings in both directions to preserve all 64 bits of guard identities.
    pub fn install(mut handler: impl InspectTransport + 'static) -> Result<Self, JsValue> {
        let window = js_sys::global();
        let key = JsValue::from_str("__octaraInspect");
        if Reflect::has(&window, &key)? {
            return Err(JsValue::from_str("the Octara inspection bridge is already installed"));
        }
        let exchange = Closure::<dyn FnMut(String) -> String>::new(move |line: String| handler.exchange(line.as_bytes()));
        if !Reflect::set(&window, &key, exchange.as_ref())? {
            return Err(JsValue::from_str("the Octara inspection bridge could not be installed"));
        }
        Ok(Self { window, key, exchange })
    }
}

impl Drop for WebTransport {
    fn drop(&mut self) {
        if let Ok(installed) = Reflect::get(&self.window, &self.key)
            && Object::is(&installed, self.exchange.as_ref())
        {
            let _ = Reflect::delete_property(&self.window, &self.key);
        }
    }
}
