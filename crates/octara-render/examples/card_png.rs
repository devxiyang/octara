//! Renders a small card with both renderers: `cargo run -p octara-render --example card_png -- <dir>`.
use octara::prelude::*;
use octara_render::{CpuRenderer, GpuRenderer};

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let app = Window::headless(|| {
        col()
            .size_full()
            .p_4()
            .gap_4()
            .child(
                row()
                    .w(260)
                    .h(80)
                    .p_3()
                    .rounded_lg()
                    .border_2()
                    .bg(Token::Accent)
                    .border_color(Token::Danger)
                    .child(text("Hello, Octara").text_color(Token::AccentText).text_2xl()),
            )
            .child(el().size_16().rounded_full().bg(Token::Accent).opacity(0.5))
            .child(text("Agents see pixels without a GPU; windows use the same display list.").w(260).text_color(Token::TextMuted))
            .into()
    });
    let dl = app.display_list();
    CpuRenderer::new().render(&dl, 2.0).save_png(format!("{dir}/card_cpu.png")).unwrap();
    if let Some(mut gpu) = GpuRenderer::new() {
        gpu.render(&dl, 2.0).save_png(format!("{dir}/card_gpu.png")).unwrap();
    }
}
