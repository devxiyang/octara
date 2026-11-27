//! Runs the headless parts through the DOM host. Open `headless.html?trusted` for browser-generated right clicks.

fn main() {}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod browser {
    use octara::prelude::*;
    use octara_headless::{
        Choice, ContextMenuState, Control, MenuState,
        field::{self, FieldState},
        tabs::{Activation, Orientation, TabsState},
    };
    use octara_host_web::WebApp;
    use wasm_bindgen::prelude::*;
    use web_sys::HtmlElement;

    #[wasm_bindgen]
    pub struct HeadlessFixture {
        _app: WebApp,
    }

    #[wasm_bindgen]
    impl HeadlessFixture {
        #[wasm_bindgen(constructor)]
        pub fn new(mount: HtmlElement) -> Result<Self, JsValue> {
            let app = WebApp::mount(mount, |build| {
                let rows = build.list_from([("customize", Choice::new("Customize"))]);
                let menu = MenuState::new(build, Control::Uncontrolled(false), rows);
                let context = ContextMenuState::new(build, menu);
                let trigger = el().w(200).h(80).test_id("context-trigger").child(text("Canvas").w(180).h(72).test_id("context-target"));
                let context = context.trigger(trigger, Placement::BELOW.gap(0.0).margin(0.0), None, move |_| {
                    let action = menu.item(text("Customize").h(28).test_id("context-item"), &"customize");
                    menu.items(col().w(100).test_id("context-items").child(action))
                });

                let label = build.signal("Name");
                let value = build.signal("draft".to_owned());
                let field = FieldState::new(build, false);
                let field = field::field(col().test_id("field"), field, move |build| {
                    let editor = input().test_id("editor").value(value).on_input(move |write, value_text| {
                        write.set(value, value_text.to_owned());
                    });
                    col()
                        .child(field::label(build, text(label).test_id("field-label")))
                        .child(field::description(build, text("Use your full name").test_id("field-description")))
                        .child(field::control(build, editor))
                });
                let rename = el()
                    .role(Role::Button)
                    .focusable()
                    .test_id("rename-field")
                    .on_press(move |write| write.set(label, "Display name"))
                    .child(text("Rename field"));

                let tabs = TabsState::new(build, Control::Uncontrolled(Some(1)), Activation::Manual, Orientation::Horizontal);
                let tab_list = tabs.list(
                    row()
                        .test_id("tab-list")
                        .child(tabs.tab(text("First").test_id("first-tab"), 1))
                        .child(tabs.tab(text("Second").test_id("second-tab"), 2)),
                );
                let panels = tabs.panels(
                    col()
                        .child(tabs.panel(text("First panel").test_id("first-panel"), 1))
                        .child(tabs.panel(text("Second panel").test_id("second-panel"), 2)),
                );

                col()
                    .child(context)
                    .child(text("Plain region").w(160).h(30).test_id("plain-region"))
                    .child(field)
                    .child(rename)
                    .child(tab_list)
                    .child(panels)
            })?;
            Ok(Self { _app: app })
        }
    }
}
