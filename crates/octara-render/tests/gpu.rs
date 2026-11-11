//! The GPU renderer against the reference renderer: same display list, same pixels within two
//! levels per channel. Skipped when there is no GPU.

use octara::prelude::*;
use octara_render::{Bitmap, CpuRenderer, GpuRenderer};

fn compare(name: &str, cpu: &Bitmap, gpu: &Bitmap) {
    assert_eq!((cpu.width, cpu.height), (gpu.width, gpu.height), "{name}: size");
    let (mut worst, mut off, mut at) = (0u8, 0usize, (0, 0));
    for y in 0..cpu.height {
        for x in 0..cpu.width {
            let (a, b) = (cpu.pixel(x, y), gpu.pixel(x, y));
            let d = a.iter().zip(b).map(|(a, b)| a.abs_diff(b)).max().unwrap();
            if d > 0 {
                off += 1;
            }
            if d > worst {
                worst = d;
                at = (x, y);
            }
        }
    }
    eprintln!("{name}: max channel difference {worst} at {at:?}, {off} pixels differ");
    assert!(
        worst <= 2,
        "{name}: max channel difference {worst} at {at:?} (cpu {:?}, gpu {:?})",
        cpu.pixel(at.0, at.1),
        gpu.pixel(at.0, at.1)
    );
}

fn card() -> View {
    col()
        .size_full()
        .p_4()
        .gap_4()
        .child(
            row()
                .w(240)
                .h(80)
                .p_3()
                .rounded_lg()
                .border_2()
                .bg(Token::Accent)
                .border_color(Token::Danger)
                .child(text("Hello, 世界 ✓").text_color(Token::AccentText).text_2xl()),
        )
        .child(el().size_16().rounded_full().bg(Token::Accent).opacity(0.5))
        .child(
            col()
                .w(200)
                .h(60)
                .rounded_xl()
                .overflow_y_scroll()
                .bg(Token::Surface)
                .children((0..10).map(|i| text(format!("row {i} with some words")))),
        )
        .into()
}

fn shadows() -> View {
    row()
        .size_full()
        .p_8()
        .gap_8()
        .flex_wrap()
        .bg(Token::Background)
        .child(el().size_16().rounded_md().bg(Token::Surface).shadow_sm())
        .child(el().size_16().rounded_lg().bg(Token::Surface).shadow_lg())
        .child(el().size_16().rounded_full().bg(Token::Surface).shadow_2xl())
        .child(el().size_16().bg(Token::Surface).shadow([BoxShadow::new(6.0, 6.0, rgb(0x2563eb)).spread_radius(2.0)]))
        .child(
            el().w(80)
                .h(40)
                .rounded_md()
                .overflow_hidden()
                .bg(Token::Surface)
                .child(el().size_8().m_2().rounded_sm().bg(Token::Accent).shadow_md()),
        )
        .into()
}

fn gradients() -> View {
    row()
        .size_full()
        .p_6()
        .gap_4()
        .flex_wrap()
        .child(el().w(120).h(60).rounded_lg().bg(linear_gradient(
            90.0,
            linear_color_stop(rgb(0x3b82f6), 0.0),
            linear_color_stop(rgb(0xec4899), 1.0),
        )))
        .child(el().w(120).h(60).rounded_full().bg(linear_gradient(
            180.0,
            linear_color_stop(Token::Accent, 0.2),
            linear_color_stop(Token::Surface, 0.8),
        )))
        .child(el().size_16().border_2().border_color(Token::Border).bg(linear_gradient(
            45.0,
            linear_color_stop(rgb(0x10b981), 0.0),
            linear_color_stop(Color::TRANSPARENT, 1.0),
        )))
        .child(el().w(80).h(40).opacity(0.5).bg(linear_gradient(
            315.0,
            linear_color_stop(rgb(0xf59e0b), 0.0),
            linear_color_stop(rgb(0x7c3aed), 1.0),
        )))
        .into()
}

/// A 24×16 test image: color ramps, a transparent corner, a hard checker.
fn picture() -> Image {
    let (w, h) = (24u32, 16u32);
    let mut px = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let checker = ((x / 4) + (y / 4)) % 2 == 0;
            let a = if x < 6 && y < 6 { 0 } else { 255 };
            px.extend([(x * 10) as u8, (y * 15) as u8, if checker { 230 } else { 40 }, a]);
        }
    }
    Image::rgba(w, h, px)
}

fn images() -> View {
    let img = picture();
    row()
        .size_full()
        .p_4()
        .gap_3()
        .flex_wrap()
        .items_start()
        .child(el().img(&img))
        .child(el().w(96).img(&img).rounded_lg())
        .child(el().size_16().img(&img).object_fit(ObjectFit::Contain).bg(Token::Surface))
        .child(el().size_16().img(&img).object_fit(ObjectFit::Cover).rounded_full())
        .child(el().w(120).h(40).img(&img).object_fit(ObjectFit::None).border_2().border_color(Token::Border))
        .child(el().w(12).img(&img).opacity(0.6))
        .into()
}

fn layers() -> View {
    let overlap = || {
        el().w(120)
            .h(70)
            .child(el().absolute().top_0().left_0().size_12().rounded_md().bg(rgb(0xef4444)))
            .child(el().absolute().top_4().left_8().size_12().rounded_full().bg(rgb(0x3b82f6)))
            .child(text("Group").absolute().bottom_0().right_0().text_lg())
    };
    row()
        .size_full()
        .p_4()
        .gap_4()
        .flex_wrap()
        .bg(Token::Background)
        .child(overlap().opacity(0.5))
        .child(overlap().opacity(0.8).child(el().absolute().top_2().left_2().size_6().bg(rgb(0x10b981)).opacity(0.5)))
        .child(el().w(80).h(50).rounded_lg().overflow_hidden().child(overlap().opacity(0.6)))
        .child(el().size_12().rounded_md().bg(Token::Accent).opacity(0.4))
        .into()
}

#[test]
fn gpu_matches_the_reference_renderer() {
    let Some(mut gpu) = GpuRenderer::new() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let mut cpu = CpuRenderer::new();
    type Case = (&'static str, fn() -> View);
    let cases: [Case; 8] = [
        ("card", card),
        ("todo", || octara_examples_like::todo()),
        ("settings", || octara_examples_like::settings()),
        ("dark", card),
        ("shadows", shadows),
        ("gradients", gradients),
        ("images", images),
        ("layers", layers),
    ];
    for (name, view) in cases {
        let mut app = App::new(view);
        if name == "dark" {
            app.set_theme(Theme::dark());
        }
        for scale in [1.0, 2.0] {
            let dl = app.display_list();
            compare(&format!("{name}@{scale}x"), &cpu.render(&dl, scale), &gpu.render(&dl, scale));
        }
    }
}

/// Small stand-ins for the example apps (this crate does not depend on the examples).
mod octara_examples_like {
    use octara::prelude::*;

    pub fn todo() -> View {
        col()
            .size_full()
            .p_4()
            .gap_2()
            .children((1..=8).map(|i| {
                row()
                    .gap_2()
                    .items_center()
                    .child(el().size_4().rounded_sm().border().bg(Token::Background))
                    .child(text(format!("Item {i}")))
                    .child(text("Delete").text_sm().px_3().py_1p5().rounded_md())
            }))
            .into()
    }

    pub fn settings() -> View {
        col()
            .size_full()
            .p_4()
            .gap_3()
            .child(row().w_8().h_5().p(1.0).rounded_full().bg(Token::Border).child(el().size_4().rounded_full().bg(Token::Background)))
            .child(text("notifications=off sound=on volume=50 theme=system").text_color(Token::TextMuted))
            .into()
    }
}
