//! Browser regression fixture for route mounting and the platform history bridge.

fn main() {}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod browser {
    use std::{cell::Cell, rc::Rc};

    use octara::prelude::*;
    use octara_host_web::{RouteCodec, WebApp};
    use wasm_bindgen::prelude::*;
    use web_sys::HtmlElement;

    #[wasm_bindgen]
    pub struct NavigationFixture {
        app: WebApp,
        navigator: Navigator<String>,
        block: Signal<bool>,
    }

    #[wasm_bindgen]
    impl NavigationFixture {
        #[wasm_bindgen(constructor)]
        pub fn new(mount: HtmlElement, pathname: String) -> Result<Self, JsValue> {
            let handles = Rc::new(Cell::new(None));
            let app = WebApp::mount(mount, {
                let handles = handles.clone();
                move |b| {
                    let nav = b.navigator("fallback".to_owned());
                    let block = b.signal(false);
                    handles.set(Some((nav, block)));
                    route_stack(nav, RouteMount::KeepAlive, move |b, route| {
                        if route == "form" {
                            b.on_before_leave(nav, move |r, _, _| !r.get(block));
                        }
                        let draft = b.signal(String::new());
                        col()
                            .h(100)
                            .overflow_y(Overflow::Scroll)
                            .test_id(format!("scroll-{route}"))
                            .child(
                                input()
                                    .test_id(format!("draft-{route}"))
                                    .value(draft)
                                    .on_input(move |w, value| w.set(draft, value.to_owned())),
                            )
                            .child(el().h(600).shrink(0.0))
                    })
                }
            })?;
            let (navigator, block) = handles.get().expect("mounted navigator");
            app.bind_history(
                navigator,
                RouteCodec::new(
                    move |route| format!("{pathname}?route={route}"),
                    |path| path.split_once("?route=").map(|(_, route)| route.to_owned()),
                ),
            )?;
            Ok(Self { app, navigator, block })
        }

        pub fn current(&self) -> String {
            self.app.update(|w| w.read(|r| self.navigator.current(r))).expect("read current route")
        }

        pub fn push(&self, route: String) {
            assert!(self.app.update(|w| self.navigator.push(w, route)).unwrap());
        }

        pub fn replace(&self, route: String) {
            assert!(self.app.update(|w| self.navigator.replace(w, route)).unwrap());
        }

        pub fn go(&self, delta: i32) {
            assert!(self.app.update(|w| self.navigator.go(w, delta as isize)).unwrap());
        }

        pub fn block(&self, dirty: bool) {
            self.app.update(|w| w.set(self.block, dirty)).unwrap();
        }
    }
}
