//! A platform web view (WKWebView on macOS; a placeholder box headless), placed with the
//! frame: it scrolls and clips with the page.

use super::section;
use octara::prelude::*;
use octara_kit::{ButtonVariant, Size as KitSize, button};
use octara_webview::{WebViewEvent, WebViewProps, view as web_view};

#[ui]
pub fn view(cx: &mut BuildCx<'_>) -> El {
    let page = cx.signal(0usize);
    let page_title = cx.signal(String::new());
    let loaded = cx.signal(0u32);
    let section = section("Web view");
    let web = web_view(move |r: &mut ReadCx<'_>| {
        let (title, color) = [("First page", "#6366f1"), ("Second page", "#ec4899")][r.get(page)];
        WebViewProps::html(format!(
            "<html><head><title>{title}</title></head>\
             <body style='margin:0;font:15px -apple-system;display:grid;place-items:center;height:100vh;background:{color};color:white'>\
             <div><h2>{title}</h2><p>HTML in a native web view.</p></div></body></html>"
        ))
    })
    .test_id("web")
    .label("Web page")
    .on_typed_native::<WebViewEvent>(move |w, _, event| match event {
        WebViewEvent::Navigation(state) => {
            w.set_if_changed(page_title, state.title.clone().unwrap_or_default());
        }
        WebViewEvent::Loaded { .. } => w.update(loaded, |n| *n += 1),
        _ => {}
    })
    .w(360)
    .h(160)
    .rounded(RadiusToken::Xl);
    let section = if std::env::var_os("OCTARA_WEB_PLACEMENT_PROBE").is_some_and(|enabled| enabled == "1") {
        let moved = cx.signal(false);
        let marker = cx.signal(false);
        // Move only the native view to confirm that a capture can detect mismatched versions.

        section
            .on_command("web.move-native", move |w| w.update(moved, |v| *v = !*v))
            .child(
                el().position(Position::Relative)
                    .w(rel(1.0))
                    .h(160)
                    .child(
                        el().test_id("web-marker")
                            .size(28)
                            .bg(move |r: &mut ReadCx<'_>| if r.get(marker) { rgb(0x00ff00) } else { rgb(0xff0000) }),
                    )
                    .child(web.position(Position::Absolute).top(0).style_with(move |r, s| s.left(if r.get(moved) { 168.0 } else { 52.0 }))),
            )
            .child(button("Move web view", KitSize::Md, ButtonVariant::Secondary).test_id("move_web").on_press(move |w| {
                let right = !w.get(moved);
                w.set(moved, right);
                w.set(marker, right);
            }))
    } else {
        section.child(web)
    };
    section.child(
        row()
            .gap(sp(3.0))
            .align(AlignItems::Center)
            .child(
                button("Other page", KitSize::Md, ButtonVariant::Secondary)
                    .test_id("other_page")
                    .on_press(move |w| w.update(page, |page| *page = 1 - *page)),
            )
            .child(
                text(move |r: &mut ReadCx<'_>| format!("“{}”, loaded {} times", r.get(page_title), r.get(loaded)))
                    .text_size(FontSizeToken::Sm)
                    .test_id("web_status"),
            ),
    )
}
