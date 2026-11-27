//! Materials, an author-managed GPU canvas, and a backdrop lens in a single Octara window.

mod flow;
mod glass;
pub mod input_pressure;
pub mod replay;
mod transition;

use crate::window_options;
use octara::prelude::*;
use octara_icons as icons;
use octara_kit::slider;
use octara_render::gpu::material;
use std::sync::Arc;

const BACKGROUND: Color = rgb(0x111217);
const PANEL: Color = rgb(0x1a1b23);
const BORDER: Color = rgb(0x31323e);
const TEXT: Color = rgb(0xefedf6);
const MUTED: Color = rgb(0xaaa8bb);
const ACCENT: Color = rgb(0xb9a4fa);
const SELECTION: Color = rgb(0x2e2743);
const DEFAULT_PHASE: f64 = 0.38;
const GLASS_RADIUS: f32 = 18.0;

#[derive(Clone, Copy)]
struct Preset {
    id: &'static str,
    name: &'static str,
    hue: f32,
    spread: f32,
}

const PRESETS: [Preset; 8] = [
    Preset { id: "prism", name: "Prism", hue: 0.675, spread: 0.46 },
    Preset { id: "ember", name: "Ember", hue: 0.045, spread: 0.21 },
    Preset { id: "lagoon", name: "Lagoon", hue: 0.39, spread: 0.28 },
    Preset { id: "ion", name: "Ion", hue: 0.60, spread: 0.21 },
    Preset { id: "orchid", name: "Orchid", hue: 0.82, spread: 0.19 },
    Preset { id: "solar", name: "Solar", hue: 0.16, spread: 0.22 },
    Preset { id: "frost", name: "Frost", hue: 0.48, spread: 0.08 },
    Preset { id: "copper", name: "Copper", hue: 0.995, spread: 0.078 },
];

const MATERIAL: &str = concat!(include_str!("gpu_lab/palette.wgsl"), include_str!("gpu_lab/swatch.wgsl"));

pub fn options() -> WindowOptions {
    let mut options = window_options("Octara Shader Lab", 1000.0, 700.0);
    options.presentation.theme = theme();
    options
}

fn theme() -> Theme {
    Theme::dark()
        .color(ColorToken::Background, BACKGROUND)
        .color(ColorToken::Surface, PANEL)
        .color(ColorToken::SurfaceHover, rgb(0x272834))
        .color(ColorToken::SurfacePressed, rgb(0x353446))
        .color(ColorToken::Border, BORDER)
        .color(ColorToken::Text, TEXT)
        .color(ColorToken::TextMuted, MUTED)
        .color(ColorToken::Accent, ACCENT)
        .color(ColorToken::AccentText, BACKGROUND)
        .color(ColorToken::Focus, ACCENT)
}

#[ui]
pub fn view(cx: &mut BuildCx<'_>) -> El {
    let phase = cx.signal(DEFAULT_PHASE);
    scene(cx, phase, flow::program(false))
}

/// The same scene with a local Flow output for the retention benchmark.
#[ui]
pub fn retained_view(cx: &mut BuildCx<'_>) -> El {
    let phase = cx.signal(DEFAULT_PHASE);
    scene(cx, phase, flow::program(true))
}

fn scene(cx: &mut BuildCx<'_>, phase: Signal<f64>, flow: PaintProgram) -> El {
    scene_with_materials_height(cx, phase, flow, None)
}

fn scene_with_materials_height(cx: &mut BuildCx<'_>, phase: Signal<f64>, flow: PaintProgram, materials_max_height: Option<f32>) -> El {
    let selected = cx.signal(0_usize);
    let grid = cx.signal(true);
    let passes = cx.signal(false);

    col()
        .size(rel(1.0))
        .bg(BACKGROUND)
        .text_color(TEXT)
        .child(header(move |w| {
            w.set(selected, 0);
            w.set(phase, DEFAULT_PHASE);
            w.set(grid, true);
        }))
        .child(
            row()
                .grow(1.0)
                .min_h(0.0)
                .p(16)
                .gap(14)
                .when(Breakpoint::Expanded, |p| p.p(22).gap(16))
                .child(preview(cx, selected, phase, grid, passes, flow))
                .child({
                    let materials = materials(cx, selected, phase);
                    match materials_max_height {
                        Some(height) => materials.max_h(height),
                        None => materials,
                    }
                }),
        )
}

#[ui]
fn header(reset: impl Fn(&mut WriteCx<'_>) + 'static) -> El {
    col()
        .shrink(0.0)
        .bg(PANEL)
        .child(
            row()
                .h(60)
                .px(20)
                .when(Breakpoint::Expanded, |p| p.px(24))
                .align(AlignItems::Center)
                .gap(11)
                .child(
                    el().size(34)
                        .rounded(9.0)
                        .align(AlignItems::Center)
                        .justify(JustifyContent::Center)
                        .bg(SELECTION)
                        .text_color(ACCENT)
                        .a11y_hidden()
                        .child(icon(icons::APERTURE.into()).size(19)),
                )
                .child(text("Shader Lab").role(Role::Heading).level(1).text_size(19).text_weight(600))
                .child(el().grow(1.0))
                .child(action("gpu-reset", "Reset", icons::ROTATE_CCW).on_press(reset)),
        )
        .child(rule())
}

#[ui]
fn preview(
    cx: &mut BuildCx<'_>,
    selected: Signal<usize>,
    phase: Signal<f64>,
    grid: Signal<bool>,
    passes: Signal<bool>,
    flow: PaintProgram,
) -> El {
    let preview_size = cx.signal(None::<LayoutBox>);
    let palettes = PRESETS.map(Arc::new);
    col()
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .rounded(12.0)
        .border(1)
        .border_color(BORDER)
        .overflow_x(Overflow::Clip)
        .overflow_y(Overflow::Clip)
        .bg(PANEL)
        .child(
            row()
                .shrink(0.0)
                .h(58)
                .px(15)
                .align(AlignItems::Center)
                .gap(8)
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(2)
                        .child(
                            text("Flow field")
                                .role(Role::Heading)
                                .level(2)
                                .text_size(16)
                                .text_weight(600)
                                .text_overflow(TextOverflow::Ellipsis),
                        )
                        .child(
                            text(move |r: &mut ReadCx<'_>| PRESETS[r.get(selected)].name)
                                .test_id("gpu-selected-material")
                                .text_size(11)
                                .text_color(MUTED)
                                .text_overflow(TextOverflow::Ellipsis),
                        ),
                )
                .child(
                    action("gpu-grid", "Grid", icons::GRID_2X2)
                        .text_color(move |r: &mut ReadCx<'_>| if r.get(grid) { ACCENT } else { MUTED })
                        .on_press(move |w| w.update(grid, |v| *v = !*v)),
                )
                .child(glass_control(cx))
                .child(pass_control(passes, grid)),
        )
        .child(rule())
        .child(
            el().test_id("gpu-preview")
                .observe_geometry(preview_size)
                .role(Role::Image)
                .label("Flow field preview")
                .grow(1.0)
                .min_h(0.0)
                .w(rel(1.0))
                .bg(rgb(0x0e101a))
                .paint(flow, move |r: &mut ReadCx<'_>| {
                    let params = [r.get(phase) as f32, if r.get(grid) { 1.0 } else { 0.0 }, 0.0, 0.0];
                    PaintInput::with_data(params, palettes[r.get(selected)].clone())
                }),
        )
        .child(rule())
        .child(
            row()
                .shrink(0.0)
                .h(39)
                .px(15)
                .align(AlignItems::Center)
                .gap(8)
                .text_color(MUTED)
                .child(icon(icons::GPU.into()).size(15).a11y_hidden())
                .child(text("Compute → Resolve").text_size(11).text_family(FontFamilyToken::Mono))
                .child(el().grow(1.0))
                .child(
                    text(move |r: &mut ReadCx<'_>| {
                        let rect = r.get(preview_size).unwrap_or_default().rect;
                        format!("{:.0} × {:.0} pt", rect.w, rect.h)
                    })
                    .test_id("gpu-viewport-size")
                    .text_size(11)
                    .text_family(FontFamilyToken::Mono),
                ),
        )
}

#[ui]
fn materials(cx: &mut BuildCx<'_>, selected: Signal<usize>, phase: Signal<f64>) -> El {
    let program = material(MATERIAL);
    col()
        .w(203)
        .when(Breakpoint::Expanded, |p| p.w(228))
        .shrink(0.0)
        .min_h(0.0)
        .rounded(12.0)
        .border(1)
        .border_color(BORDER)
        .overflow_x(Overflow::Clip)
        .overflow_y(Overflow::Clip)
        .bg(PANEL)
        .child(
            row()
                .h(51)
                .px(13)
                .align(AlignItems::Center)
                .justify(JustifyContent::SpaceBetween)
                .shrink(0.0)
                .child(text("Materials").role(Role::Heading).level(2).text_size(15).text_weight(600))
                .child(text(format!("{} presets", PRESETS.len())).text_size(11).text_color(MUTED)),
        )
        .child(rule())
        .child(
            col()
                .test_id("gpu-materials")
                .role(Role::List)
                .label("Material presets")
                .grow(1.0)
                .min_h(0.0)
                .overflow_y(Overflow::Scroll)
                .p(9)
                .gap(6)
                .children(PRESETS.into_iter().enumerate().map(|(index, preset)| swatch(index, preset, selected, program.clone()))),
        )
        .child(rule())
        .child(
            col()
                .shrink(0.0)
                .p(13)
                .gap(4)
                .child(
                    row()
                        .align(AlignItems::Center)
                        .justify(JustifyContent::SpaceBetween)
                        .child(text("Phase").text_size(13).text_weight(500))
                        .child(
                            text(move |r: &mut ReadCx<'_>| format!("{:.2}", r.get(phase)))
                                .test_id("gpu-phase-value")
                                .text_size(12)
                                .text_family(FontFamilyToken::Mono)
                                .text_color(ACCENT),
                        ),
                )
                .child(slider(cx, "gpu-phase", phase.into(), 0.0, 1.0, 0.01).label("Flow phase").w(rel(1.0))),
        )
}

#[ui]
fn swatch(index: usize, preset: Preset, selected: Signal<usize>, program: PaintProgram) -> El {
    let current = move |r: &mut ReadCx<'_>| r.get(selected) == index;
    row()
        .test_id(format!("gpu-material-{}", preset.id))
        .role(Role::Button)
        .label(preset.name)
        .focusable()
        .cursor(CursorStyle::PointingHand)
        .h(61)
        .shrink(0.0)
        .min_w(0.0)
        .px(7)
        .gap(11)
        .align(AlignItems::Center)
        .border(1)
        .rounded(8.0)
        .border_color(move |r: &mut ReadCx<'_>| if current(r) { rgb(0x65558c) } else { BORDER })
        .bg(move |r: &mut ReadCx<'_>| if current(r) { SELECTION } else { rgb(0x20212b) })
        .when(State::HOVER, |p| p.bg(ColorToken::SurfaceHover))
        .when(State::FOCUS_VISIBLE, |p| p.border_color(ACCENT))
        .on_press(move |w| w.set(selected, index))
        .child(
            el().test_id(format!("gpu-swatch-{}", preset.id))
                .w(61)
                .h(43)
                .shrink(0.0)
                .rounded(5.0)
                .bg(rgb(0x121222))
                .paint(program, [preset.hue, index as f32 * 0.27, 0.0, preset.spread]),
        )
        .child(text(preset.name).min_w(0.0).grow(1.0).text_overflow(TextOverflow::Ellipsis).text_size(13).text_weight(500))
        .child(
            icon(icons::CHECK.into())
                .size(14)
                .shrink(0.0)
                .a11y_hidden()
                .text_color(ACCENT)
                .opacity(move |r: &mut ReadCx<'_>| if current(r) { 1.0_f32 } else { 0.0 }),
        )
}

#[ui]
fn glass_control(cx: &mut BuildCx<'_>) -> El {
    let open = cx.signal(false);
    let program = glass::program();
    action("gpu-glass-open", "Glass", icons::BLEND)
        .label("Open glass lens")
        .controls("gpu-glass-dialog")
        .bg(SELECTION)
        .border_color(rgb(0x594a7a))
        .text_color(ACCENT)
        .on_press(move |w| w.set(open, true))
        .overlay(Placement::BELOW.align(Align::End).gap(60.0), move |r| r.get(open), move |b| glass_dialog(b, open, program.clone()))
}

#[ui]
fn glass_dialog(cx: &mut BuildCx<'_>, open: Signal<bool>, program: PaintProgram) -> El {
    let refraction = cx.signal(0.75);
    let dialog = col()
        .test_id("gpu-glass-dialog")
        .role(Role::Dialog)
        .label("Glass lens")
        .layer(LayerPolicy::modal(), move |w| w.set(open, false))
        .w(360)
        .max_w(rel(1.0))
        .min_h(192)
        .justify(JustifyContent::SpaceBetween)
        .p(18)
        .gap(13)
        .rounded(GLASS_RADIUS)
        .border(1)
        .border_color(rgb(0x67587f))
        .bg(rgb(0x28223b))
        .paint(program, move |r: &mut ReadCx<'_>| [r.get(refraction) as f32, 0.0, 0.0, 0.0])
        .shadow(Shadows::custom([BoxShadow::new(0.0, 13.0, Color::BLACK.with_alpha(0.45)).blur_radius(30.0)]))
        .text_color(TEXT)
        .child(
            row()
                .align(AlignItems::Center)
                .gap(10)
                .child(
                    el().size(36)
                        .shrink(0.0)
                        .rounded(9.0)
                        .bg(rgb(0x302641))
                        .text_color(ACCENT)
                        .align(AlignItems::Center)
                        .justify(JustifyContent::Center)
                        .a11y_hidden()
                        .child(icon(icons::BLEND.into()).size(19)),
                )
                .child(text("Glass lens").role(Role::Heading).level(2).grow(1.0).min_w(0.0).text_size(19).text_weight(600))
                .child(action("gpu-glass-cancel", "Close", icons::X).shrink(0.0).autofocus().on_press(move |w| w.set(open, false))),
        )
        .child(text("Bend the grid and ribbons behind this panel.").text_size(13))
        .child(
            col()
                .w(rel(1.0))
                .px(10)
                .py(5)
                .rounded(8.0)
                .bg(rgb(0x171222).with_alpha(0.72))
                .child(
                    row()
                        .align(AlignItems::Center)
                        .justify(JustifyContent::SpaceBetween)
                        .child(text("Refraction").text_size(12).text_weight(500))
                        .child(
                            text(move |r: &mut ReadCx<'_>| format!("{:.0}%", r.get(refraction) * 100.0))
                                .test_id("gpu-glass-refraction-value")
                                .text_size(12)
                                .text_family(FontFamilyToken::Mono)
                                .text_color(ACCENT),
                        ),
                )
                .child(slider(cx, "gpu-glass-refraction", refraction.into(), 0.0, 1.0, 0.05).label("Refraction").w(rel(1.0))),
        );
    if std::env::var_os("OCTARA_GPU_LAB_TRANSITION").as_deref() == Some(std::ffi::OsStr::new("1")) {
        dialog.child(transition::panel(cx))
    } else {
        dialog
    }
}

#[ui]
fn pass_control(open: Signal<bool>, grid: Signal<bool>) -> El {
    action("gpu-pass-open", "Passes", icons::LAYERS)
        .label("Render passes")
        .child(icon(icons::CHEVRON_DOWN.into()).size(12).a11y_hidden())
        .on_press(move |w| w.update(open, |v| *v = !*v))
        .overlay(Placement::BELOW.align(Align::End).gap(9.0), move |r| r.get(open), move |_| pass_popup(open, grid))
}

#[ui]
fn pass_popup(open: Signal<bool>, grid: Signal<bool>) -> El {
    col()
        .test_id("gpu-pass-popover")
        .role(Role::Dialog)
        .label("Render passes")
        .layer(LayerPolicy::modal(), move |w| w.set(open, false))
        .w(246)
        .p(12)
        .gap(9)
        .rounded(10.0)
        .border(1)
        .border_color(rgb(0x454458))
        .bg(rgb(0x22232e))
        .text_color(TEXT)
        .shadow(Shadows::custom([BoxShadow::new(0.0, 10.0, Color::BLACK.with_alpha(0.42)).blur_radius(26.0)]))
        .child(
            row()
                .align(AlignItems::Center)
                .justify(JustifyContent::SpaceBetween)
                .child(text("Render passes").role(Role::Heading).level(2).text_size(14).text_weight(600))
                .child(
                    row()
                        .test_id("gpu-pass-close")
                        .role(Role::Button)
                        .label("Close render passes")
                        .focusable()
                        .cursor(CursorStyle::PointingHand)
                        .on_press(move |w| w.set(open, false))
                        .size(26)
                        .align(AlignItems::Center)
                        .justify(JustifyContent::Center)
                        .rounded(5.0)
                        .border(1)
                        .border_color(Color::TRANSPARENT)
                        .when(State::HOVER, |p| p.bg(ColorToken::SurfaceHover))
                        .when(State::FOCUS_VISIBLE, |p| p.border_color(ACCENT))
                        .child(icon(icons::X.into()).size(14).a11y_hidden().text_color(MUTED)),
                ),
        )
        .child(rule())
        .child(pass_row(icons::CPU, "Vector field", "Compute · 8 × 8 workgroups"))
        .child(pass_row(icons::BLEND, "Color resolve", "Render · RGBA8 Unorm"))
        .child(pass_row(
            icons::GRID_2X2,
            "Reference grid",
            move |r: &mut ReadCx<'_>| if r.get(grid) { "Composite · On" } else { "Composite · Off" },
        ))
}

#[ui]
fn pass_row(symbol: icons::LucideIcon, title: &'static str, detail: impl IntoText) -> El {
    row()
        .align(AlignItems::Center)
        .gap(10)
        .px(2)
        .child(
            el().size(29)
                .rounded(6.0)
                .align(AlignItems::Center)
                .justify(JustifyContent::Center)
                .bg(SELECTION)
                .text_color(ACCENT)
                .a11y_hidden()
                .child(icon(symbol.into()).size(15)),
        )
        .child(col().gap(2).child(text(title).text_size(12).text_weight(500)).child(text(detail).text_size(11).text_color(MUTED)))
}

#[ui]
fn action(id: &'static str, title: &'static str, symbol: icons::LucideIcon) -> El {
    row()
        .test_id(id)
        .role(Role::Button)
        .label(title)
        .focusable()
        .cursor(CursorStyle::PointingHand)
        .h(32)
        .px(9)
        .gap(6)
        .align(AlignItems::Center)
        .rounded(6.0)
        .border(1)
        .border_color(BORDER)
        .bg(rgb(0x22232d))
        .text_color(TEXT)
        .when(State::HOVER, |p| p.bg(ColorToken::SurfaceHover))
        .when(State::PRESSED, |p| p.bg(ColorToken::SurfacePressed))
        .when(State::FOCUS_VISIBLE, |p| p.border_color(ACCENT))
        .child(icon(symbol.into()).size(14).a11y_hidden())
        .child(text(title).text_size(12).text_weight(500))
}

fn rule() -> El {
    el().w(rel(1.0)).h(1).shrink(0.0).bg(BORDER).a11y_hidden()
}
