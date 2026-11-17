//! A visible text-input fixture for actual OS keyboard and Chinese IME qualification.
//! Build this binary, then prepare its app bundle with `python3 examples/ime_window.py`.

use octara::prelude::*;
use octara_kit::{button, text_field};

#[ui]
fn ime() -> View {
    let single = state("single", || "A😀Z".to_owned());
    let multi = state("multi", || "alpha\nbeta".to_owned());
    let password = state("password", String::new);
    let single_edits = state("single_edits", || 0u32);
    let multi_edits = state("multi_edits", || 0u32);
    let password_edits = state("password_edits", || 0u32);
    let submits = state("submits", || 0u32);
    watch(
        move || single.get(),
        move |_, before| {
            if before.is_some() {
                single_edits.update(|n| *n += 1);
            }
        },
    );
    watch(
        move || multi.get(),
        move |_, before| {
            if before.is_some() {
                multi_edits.update(|n| *n += 1);
            }
        },
    );
    watch(
        move || password.get(),
        move |_, before| {
            if before.is_some() {
                password_edits.update(|n| *n += 1);
            }
        },
    );

    col()
        .id("ime-root")
        .size_full()
        .p_6()
        .gap_3()
        .bg(Token::Surface)
        .child(text("OS input and Chinese IME").text_xl().font_semibold())
        .child(text("Use real keyboard input. Composing must not change committed values or edit counters.").text_sm())
        .child(
            col()
                .gap_2()
                .child(text("Single line — Unicode seed").font_semibold())
                .child(text_field(single).id("ime-single").label("Single line").on_submit(move || submits.update(|n| *n += 1)))
                .child(text(move || format!("Single value: {:?}", single.get())).id("ime-single-value").text_sm())
                .child(text(move || format!("Single edits: {}", single_edits.get())).id("ime-single-edits").text_sm()),
        )
        .child(
            col()
                .gap_2()
                .child(text("Multiple lines — Enter breaks a line, Command+Enter submits").font_semibold())
                .child(
                    text_field(multi)
                        .id("ime-multi")
                        .label("Multiple lines")
                        .multiline()
                        .rows(3, 5)
                        .on_submit(move || submits.update(|n| *n += 1)),
                )
                .child(text(move || format!("Multi value: {:?}", multi.get())).id("ime-multi-value").text_sm())
                .child(text(move || format!("Multi edits: {}", multi_edits.get())).id("ime-multi-edits").text_sm()),
        )
        .child(
            col()
                .gap_2()
                .child(text("Secure field — use test text only").font_semibold())
                .child(text_field(password).id("ime-password").label("Password").secure())
                .child(text(move || format!("Password edits: {}", password_edits.get())).id("ime-password-edits").text_sm()),
        )
        .child(text(move || format!("Submits: {}", submits.get())).id("ime-submits"))
        .child(button("Move focus away").id("ime-blur").self_start().on_press(|| focus("ime-blur")))
        .into()
}

fn main() {
    octara_shell::run_with(
        octara_shell::Options { title: "Octara IME Validation".into(), size: (800.0, 720.0), ..Default::default() },
        ime,
    );
}
