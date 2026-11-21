//! Materials, an author-managed GPU canvas, and a backdrop lens in a single Octara window.

mod flow;
mod glass;
pub mod input_pressure;
pub mod replay;

use octara::prelude::*;
use octara_icons as icons;
use octara_kit::slider;
use octara_render::gpu::material;

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

pub fn options() -> octara_shell::Options {
    let mut options = octara_shell::Options { window: WindowOptions::new("Octara Shader Lab").size(1000.0, 700.0), ..Default::default() };
    options.app.theme = theme();
    options
}

fn theme() -> Theme {
    Theme::dark()
        .color(Token::Background, BACKGROUND)
        .color(Token::Surface, PANEL)
        .color(Token::SurfaceHover, rgb(0x272834))
        .color(Token::SurfacePressed, rgb(0x353446))
        .color(Token::Border, BORDER)
        .color(Token::Text, TEXT)
        .color(Token::TextMuted, MUTED)
        .color(Token::Accent, ACCENT)
        .color(Token::AccentText, BACKGROUND)
        .color(Token::Focus, ACCENT)
}

#[ui]
pub fn view() -> El {
    let phase = state("phase", || DEFAULT_PHASE);
    scene(phase, flow::program(false))
}

/// The same scene with a local Flow output for the retention benchmark.
#[ui]
pub fn retained_view() -> El {
    let phase = state("phase", || DEFAULT_PHASE);
    scene(phase, flow::program(true))
}

fn scene(phase: Signal<f64>, flow: PaintProgram) -> El {
    scene_with_materials_height(phase, flow, None)
}

fn scene_with_materials_height(phase: Signal<f64>, flow: PaintProgram, materials_max_height: Option<f32>) -> El {
    let selected = state("material", || 0_usize);
    let grid = state("grid", || true);
    let passes = state("passes", || false);

    col()
        .size_full()
        .bg(BACKGROUND)
        .text_color(TEXT)
        .child(header(move || {
            selected.set(0);
            phase.set(DEFAULT_PHASE);
            grid.set(true);
        }))
        .child(
            row()
                .flex_1()
                .min_h(0.0)
                .p(16)
                .gap(14)
                .when_expanded(|p| p.p(22).gap(16))
                .child(preview(selected, phase, grid, passes, flow))
                .child({
                    let materials = materials(selected, phase);
                    match materials_max_height {
                        Some(height) => materials.max_h(height),
                        None => materials,
                    }
                }),
        )
}

#[ui]
fn header(reset: impl Fn() + 'static) -> El {
    col()
        .flex_shrink_0()
        .bg(PANEL)
        .child(
            row()
                .h(60)
                .px(20)
                .when_expanded(|p| p.px(24))
                .items_center()
                .gap(11)
                .child(
                    el().size(34)
                        .rounded(9.0)
                        .items_center()
                        .justify_center()
                        .bg(SELECTION)
                        .text_color(ACCENT)
                        .a11y_hidden()
                        .child(icon(icons::APERTURE).size(19)),
                )
                .child(text("Shader Lab").role(Role::Heading).level(1).text_size(19).font_semibold())
                .child(el().flex_1())
                .child(action("gpu-reset", "Reset", icons::ROTATE_CCW).on_press(reset)),
        )
        .child(rule())
}

#[ui]
fn preview(selected: Signal<usize>, phase: Signal<f64>, grid: Signal<bool>, passes: Signal<bool>, flow: PaintProgram) -> El {
    let preview_size = size_of("gpu-preview");
    col()
        .flex_1()
        .min_w(0.0)
        .min_h(0.0)
        .rounded(12.0)
        .border()
        .border_color(BORDER)
        .overflow_clip()
        .bg(PANEL)
        .child(
            row()
                .flex_shrink_0()
                .h(58)
                .px(15)
                .items_center()
                .gap(8)
                .child(
                    col()
                        .flex_1()
                        .min_w(0.0)
                        .gap(2)
                        .child(text("Flow field").role(Role::Heading).level(2).text_size(16).font_semibold().truncate())
                        .child(
                            text(move || PRESETS[selected.get()].name)
                                .id("gpu-selected-material")
                                .text_size(11)
                                .text_color(MUTED)
                                .truncate(),
                        ),
                )
                .child(
                    action("gpu-grid", "Grid", icons::GRID_2X2)
                        .role(Role::CheckBox)
                        .checked(grid)
                        .text_color(move || if grid.get() { ACCENT } else { MUTED })
                        .on_toggle(move || grid.update(bool::toggle)),
                )
                .child(glass_control())
                .child(pass_control(passes, grid)),
        )
        .child(rule())
        .child(el().id("gpu-preview").role(Role::Image).label("Flow field preview").flex_1().min_h(0.0).w_full().bg(rgb(0x0e101a)).paint(
            flow,
            move || {
                let preset = PRESETS[selected.get()];
                [preset.hue, phase.get() as f32, if grid.get() { 1.0 } else { 0.0 }, preset.spread]
            },
        ))
        .child(rule())
        .child(
            row()
                .flex_shrink_0()
                .h(39)
                .px(15)
                .items_center()
                .gap(8)
                .text_color(MUTED)
                .child(icon(icons::GPU).size(15).a11y_hidden())
                .child(text("Compute → Resolve").text_size(11).font_mono())
                .child(el().flex_1())
                .child(
                    text(move || {
                        let (w, h) = preview_size.get();
                        format!("{w:.0} × {h:.0} pt")
                    })
                    .id("gpu-viewport-size")
                    .text_size(11)
                    .font_mono(),
                ),
        )
}

#[ui]
fn materials(selected: Signal<usize>, phase: Signal<f64>) -> El {
    let program = material(MATERIAL);
    col()
        .w(203)
        .when_expanded(|p| p.w(228))
        .flex_shrink_0()
        .min_h(0.0)
        .rounded(12.0)
        .border()
        .border_color(BORDER)
        .overflow_clip()
        .bg(PANEL)
        .child(
            row()
                .h(51)
                .px(13)
                .items_center()
                .justify_between()
                .flex_shrink_0()
                .child(text("Materials").role(Role::Heading).level(2).text_size(15).font_semibold())
                .child(text(format!("{} presets", PRESETS.len())).text_size(11).text_color(MUTED)),
        )
        .child(rule())
        .child(
            col()
                .id("gpu-materials")
                .role(Role::List)
                .label("Material presets")
                .flex_1()
                .min_h(0.0)
                .overflow_y_scroll()
                .p(9)
                .gap(6)
                .children(PRESETS.into_iter().enumerate().map(|(index, preset)| swatch(index, preset, selected, program.clone()))),
        )
        .child(rule())
        .child(
            col()
                .flex_shrink_0()
                .p(13)
                .gap(4)
                .child(
                    row().items_center().justify_between().child(text("Phase").text_size(13).font_medium()).child(
                        text(move || format!("{:.2}", phase.get())).id("gpu-phase-value").text_size(12).font_mono().text_color(ACCENT),
                    ),
                )
                .child(slider("gpu-phase", phase, 0.0, 1.0, 0.01).label("Flow phase").w_full()),
        )
}

#[ui]
fn swatch(index: usize, preset: Preset, selected: Signal<usize>, program: PaintProgram) -> El {
    let current = move || selected.get() == index;
    row()
        .id(format!("gpu-material-{}", preset.id))
        .role(Role::Button)
        .label(preset.name)
        .selected(current)
        .focusable()
        .cursor_pointer()
        .h(61)
        .flex_shrink_0()
        .min_w(0.0)
        .px(7)
        .gap(11)
        .items_center()
        .border()
        .rounded(8.0)
        .border_color(move || if current() { rgb(0x65558c) } else { BORDER })
        .bg(move || if current() { SELECTION } else { rgb(0x20212b) })
        .hover(|p| p.bg(Token::SurfaceHover))
        .focus_visible(|p| p.border_color(ACCENT))
        .on_press(move || selected.set(index))
        .child(
            el().id(format!("gpu-swatch-{}", preset.id))
                .w(61)
                .h(43)
                .flex_shrink_0()
                .rounded(5.0)
                .bg(rgb(0x121222))
                .paint(program, [preset.hue, index as f32 * 0.27, 0.0, preset.spread]),
        )
        .child(text(preset.name).min_w(0.0).flex_1().truncate().text_size(13).font_medium())
        .child(
            icon(icons::CHECK).size(14).flex_shrink_0().a11y_hidden().text_color(ACCENT).opacity(move || if current() { 1.0 } else { 0.0 }),
        )
}

#[ui]
fn glass_control() -> El {
    let open = state("open", || false);
    let program = glass::program();
    action("gpu-glass-open", "Glass", icons::BLEND)
        .label("Open glass lens")
        .controls("gpu-glass-dialog")
        .expanded(open)
        .bg(SELECTION)
        .border_color(rgb(0x594a7a))
        .text_color(ACCENT)
        .on_press(move || open.set(true))
        .overlay(move || open.get(), Placement::BELOW.align(Align::End).gap(60.0), move || glass_dialog(open, program.clone()))
}

#[ui]
fn glass_dialog(open: Signal<bool>, program: PaintProgram) -> El {
    let refraction = state("refraction", || 0.75);
    col()
        .id("gpu-glass-dialog")
        .role(Role::Dialog)
        .label("Glass lens")
        .modal(true, move || open.set(false))
        .w(360)
        .max_w_full()
        .min_h(192)
        .justify_between()
        .p(18)
        .gap(13)
        .rounded(GLASS_RADIUS)
        .border()
        .border_color(rgb(0x67587f))
        .bg(rgb(0x28223b))
        .paint(program, move || [refraction.get() as f32, 0.0, 0.0, 0.0])
        .shadow([BoxShadow::new(0.0, 13.0, Color::BLACK.with_alpha(0.45)).blur_radius(30.0)])
        .text_color(TEXT)
        .opacity_motion(Curve::Fade(0.8))
        .child(
            row()
                .items_center()
                .gap(10)
                .child(
                    el().size(36)
                        .flex_shrink_0()
                        .rounded(9.0)
                        .bg(rgb(0x302641))
                        .text_color(ACCENT)
                        .items_center()
                        .justify_center()
                        .a11y_hidden()
                        .child(icon(icons::BLEND).size(19)),
                )
                .child(text("Glass lens").role(Role::Heading).level(2).flex_1().min_w(0.0).text_size(19).font_semibold())
                .child(action("gpu-glass-cancel", "Close", icons::X).flex_shrink_0().autofocus().on_press(move || open.set(false))),
        )
        .child(text("Bend the grid and ribbons behind this panel.").text_size(13))
        .child(
            col()
                .w_full()
                .px(10)
                .py(5)
                .rounded(8.0)
                .bg(rgb(0x171222).with_alpha(0.72))
                .child(
                    row().items_center().justify_between().child(text("Refraction").text_size(12).font_medium()).child(
                        text(move || format!("{:.0}%", refraction.get() * 100.0))
                            .id("gpu-glass-refraction-value")
                            .text_size(12)
                            .font_mono()
                            .text_color(ACCENT),
                    ),
                )
                .child(slider("gpu-glass-refraction", refraction, 0.0, 1.0, 0.05).label("Refraction").w_full()),
        )
}

#[ui]
fn pass_control(open: Signal<bool>, grid: Signal<bool>) -> El {
    action("gpu-pass-open", "Passes", icons::LAYERS)
        .label("Render passes")
        .child(icon(icons::CHEVRON_DOWN).size(12).a11y_hidden())
        .expanded(open)
        .on_press(move || open.update(bool::toggle))
        .overlay(move || open.get(), Placement::BELOW.align(Align::End).gap(9.0), move || pass_popup(open, grid))
}

#[ui]
fn pass_popup(open: Signal<bool>, grid: Signal<bool>) -> El {
    col()
        .id("gpu-pass-popover")
        .role(Role::Dialog)
        .label("Render passes")
        .modal(true, move || open.set(false))
        .w(246)
        .p(12)
        .gap(9)
        .rounded(10.0)
        .border()
        .border_color(rgb(0x454458))
        .bg(rgb(0x22232e))
        .text_color(TEXT)
        .shadow([BoxShadow::new(0.0, 10.0, Color::BLACK.with_alpha(0.42)).blur_radius(26.0)])
        .child(
            row()
                .items_center()
                .justify_between()
                .child(text("Render passes").role(Role::Heading).level(2).text_size(14).font_semibold())
                .child(
                    row()
                        .id("gpu-pass-close")
                        .role(Role::Button)
                        .label("Close render passes")
                        .focusable()
                        .cursor_pointer()
                        .on_press(move || open.set(false))
                        .size(26)
                        .items_center()
                        .justify_center()
                        .rounded(5.0)
                        .border()
                        .border_color(Color::TRANSPARENT)
                        .hover(|p| p.bg(Token::SurfaceHover))
                        .focus_visible(|p| p.border_color(ACCENT))
                        .child(icon(icons::X).size(14).a11y_hidden().text_color(MUTED)),
                ),
        )
        .child(rule())
        .child(pass_row(icons::CPU, "Vector field", "Compute · 8 × 8 workgroups"))
        .child(pass_row(icons::BLEND, "Color resolve", "Render · RGBA8 Unorm"))
        .child(pass_row(icons::GRID_2X2, "Reference grid", move || if grid.get() { "Composite · On" } else { "Composite · Off" }))
}

#[ui]
fn pass_row(symbol: icons::Icon, title: &'static str, detail: impl IntoText) -> El {
    row()
        .items_center()
        .gap(10)
        .px(2)
        .child(
            el().size(29)
                .rounded(6.0)
                .items_center()
                .justify_center()
                .bg(SELECTION)
                .text_color(ACCENT)
                .a11y_hidden()
                .child(icon(symbol).size(15)),
        )
        .child(col().gap(2).child(text(title).text_size(12).font_medium()).child(text(detail).text_size(11).text_color(MUTED)))
}

#[ui]
fn action(id: &'static str, title: &'static str, symbol: icons::Icon) -> El {
    row()
        .id(id)
        .role(Role::Button)
        .label(title)
        .focusable()
        .cursor_pointer()
        .h(32)
        .px(9)
        .gap(6)
        .items_center()
        .rounded(6.0)
        .border()
        .border_color(BORDER)
        .bg(rgb(0x22232d))
        .text_color(TEXT)
        .hover(|p| p.bg(Token::SurfaceHover))
        .style_when(State::PRESSED, |p| p.bg(Token::SurfacePressed))
        .focus_visible(|p| p.border_color(ACCENT))
        .child(icon(symbol).size(14).a11y_hidden())
        .child(text(title).text_size(12).font_medium())
}

fn rule() -> El {
    el().w_full().h(1).flex_shrink_0().bg(BORDER).a11y_hidden()
}
