//! A platform web view (WKWebView on macOS; a placeholder box headless), placed with the
//! frame: it scrolls and clips with the page.

use super::section;
use octara::prelude::*;
use octara_kit::button;

#[component]
pub fn card() -> El {
    let page = state("page", || 0usize);
    let page_title = state("page_title", String::new);
    let loaded = state("loaded", || 0u32);
    section("Web view")
        .child(
            native_view("web")
                .id("web")
                .label("Web page")
                .props(move || {
                    let (title, color) = [("First page", "#6366f1"), ("Second page", "#ec4899")][page.get()];
                    json!({ "html": format!(
                        "<html><head><title>{title}</title></head>\
                         <body style='margin:0;font:15px -apple-system;display:grid;place-items:center;height:100vh;background:{color};color:white'>\
                         <div><h2>{title}</h2><p>HTML in a native web view.</p></div></body></html>"
                    ) })
                })
                .on_native(move |e: &NativeEvent| match e.name.as_str() {
                    "title" => page_title.set(e.data["title"].as_str().unwrap_or_default().to_owned()),
                    "load" => loaded.set(loaded.get() + 1),
                    _ => {}
                })
                .w(360)
                .h(160)
                .rounded_xl(),
        )
        .child(
            row()
                .gap_3()
                .items_center()
                .child(button("Other page").id("other_page").on_press(move || page.set(1 - page.get())))
                .child(text(move || format!("“{}”, loaded {} times", page_title.get(), loaded.get())).text_sm().id("web_status")),
        )
}
