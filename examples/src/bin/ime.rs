//! Type, compose and reconnect the native editor in one window.

use octara::prelude::*;
use octara_examples::{showcase, window_options};
use octara_kit::{ButtonVariant, Size as KitSize, button, text_area, text_field};

fn view(cx: &mut BuildCx<'_>) -> El {
    let single = cx.signal(String::new());
    let multiline = cx.signal(String::new());
    let secret = cx.signal(String::new());
    let revision = cx.signal(0_u32);
    col()
        .size(rel(1.0))
        .overflow_y(Overflow::Scroll)
        .p(sp(4.0))
        .gap(sp(3.0))
        .child(text("Octara IME Validation").role(Role::Heading).text_size(FontSizeToken::Xl).text_weight(600))
        .child(text("Single line"))
        .child(keyed(
            move |r| r.get(revision),
            move |_, _| text_field(single.into()).test_id("ime-single").label("Single line").autofocus(),
        ))
        .child(text("Multiline"))
        .child(text_area(multiline.into()).test_id("ime-multiline").label("Multiline").rows(4, 7))
        .child(text("Secure entry"))
        .child(text_field(secret.into()).test_id("ime-secure").label("Secure entry").secure(true))
        .child(
            button("Remount single-line field", KitSize::Md, ButtonVariant::Secondary)
                .test_id("ime-remount")
                .align_self(AlignItems::Start)
                .on_press(move |w| w.update(revision, |value| *value += 1)),
        )
}

fn main() -> Result<(), octara_host_native::ShellError> {
    let options = window_options("Octara IME Validation", 800.0, 720.0);
    let mode = std::env::args().nth(1).or_else(|| std::env::var("OCTARA_IME_SCENE").ok());
    match mode.as_deref() {
        Some("rename") => {
            octara_examples::native::run(window_options("Octara Rename Validation", 800.0, 600.0), octara_examples::rename::view)
        }
        Some("choices") => octara_examples::native::run(options, |b| showcase::page(b, "choices").expect("choices card")),
        _ => octara_examples::native::run(options, view),
    }
}
