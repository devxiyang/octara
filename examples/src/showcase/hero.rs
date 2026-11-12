//! The heading: a gradient, the app clock, and the theme select.

use octara::prelude::*;
use octara::task::now;
use octara_kit::select;
use std::time::Duration;

#[component]
pub fn card() -> El {
    let theme = state("theme", || "light");
    let clock = state("clock", || 0u64);
    interval(Duration::from_secs(1), move || clock.set(now().as_secs()));
    watch(move || theme.get(), |t, _| set_theme(if *t == "dark" { Theme::dark() } else { Theme::light() }));
    col()
        .gap_1()
        .p_6()
        .rounded_2xl()
        .shadow_xl()
        .bg(linear_gradient(135.0, linear_color_stop(rgb(0x6366f1), 0.0), linear_color_stop(rgb(0xec4899), 1.0)))
        .child(text("Octara").text_3xl().font_semibold().text_color(rgb(0xffffff)))
        .child(text("Shadows, gradients, images, layers, editing and async — drawn by the GPU renderer.").text_color(rgb(0xfce7f3)))
        .child(text(move || format!("Running for {} s on the app's clock", clock.get())).text_sm().text_color(rgb(0xe0e7ff)))
        .child(row().pt_2().child(select("theme", theme, [("light", "Light theme"), ("dark", "Dark theme")])))
}
