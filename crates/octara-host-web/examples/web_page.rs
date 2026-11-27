//! Browser fixture for page chrome, close guards, and visual viewport changes.

fn main() {}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod browser {
    use octara::prelude::*;
    use octara_host_web::WebApp;
    use std::{cell::Cell, rc::Rc};
    use wasm_bindgen::prelude::*;
    use web_sys::HtmlElement;

    #[wasm_bindgen]
    pub struct PageFixture {
        app: WebApp,
        allow: Signal<bool>,
        attempts: Rc<Cell<u32>>,
    }

    #[wasm_bindgen]
    impl PageFixture {
        #[wasm_bindgen(constructor)]
        pub fn new(mount: HtmlElement) -> Result<Self, JsValue> {
            let options = WindowOptions {
                presentation: octara::WindowPresentation {
                    title: "Initial page title".into(),
                    theme: Theme::light().color(ColorToken::Background, rgb(0x285480)),
                    ..Default::default()
                },
                ..Default::default()
            };
            let allow = Rc::new(Cell::new(None));
            let attempts = Rc::new(Cell::new(0));
            let app = WebApp::mount_with_options(mount, options, {
                let allow = allow.clone();
                move |build| {
                    let guard = build.signal(false);
                    allow.set(Some(guard));
                    text("The browser host owns the page.")
                }
            })?;
            let guard = allow.get().expect("fixture built");
            app.with_app(|app_impl| {
                let attempts = attempts.clone();
                app_impl.window(app.window_id()).expect("fixture window").on_close_request(move |write| {
                    if attempts.get() == 0 {
                        // A failed projection from a guard effect must not erase the veto.
                        write.request_window(WindowOptions::default(), |_| {});
                    }
                    attempts.set(attempts.get() + 1);
                    write.get(guard)
                });
            });
            Ok(Self { app, allow: guard, attempts })
        }

        pub fn update_chrome(&self) -> Result<(), JsValue> {
            self.app
                .update(|write| {
                    write.set_window_title("Updated page title");
                    write.set_window_theme(Theme::light().color(ColorToken::Background, rgb(0x713251)));
                })
                .map_err(|error| JsValue::from_str(&error.to_string()))
        }

        pub fn allow_close(&self) -> Result<(), JsValue> {
            self.app.update(|write| write.set(self.allow, true)).map_err(|error| JsValue::from_str(&error.to_string()))
        }

        pub fn attempts(&self) -> u32 {
            self.attempts.get()
        }

        pub fn keyboard_inset(&self) -> Option<f32> {
            self.app.with_app(|app| app.runtime().window(self.app.window_id()).map(|window| window.options().viewport.keyboard_inset))?
        }
    }
}
