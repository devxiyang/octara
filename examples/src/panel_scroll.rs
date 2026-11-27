//! A map panel whose viewport and scroll offset share one touch delta.

use octara::prelude::*;

pub const LIST: &str = "pan-list";
const CLOSED_HEIGHT: f32 = 100.0;
const TRAVEL: f32 = 12.0;

#[ui]
pub fn view(cx: &mut BuildCx<'_>) -> El {
    let expansion = cx.signal(0.0_f32);
    let activations = cx.signal(0_u32);
    let list_revision = cx.signal(0_u32);
    col()
        .size(rel(1.0))
        .bg(ColorToken::Surface)
        .text_color(ColorToken::Text)
        .on_command("pan.replace", move |w| w.update(list_revision, |revision| *revision += 1))
        .child(
            el().test_id("pan-map")
                .style_with(move |r, p| p.h(220.0 - r.get(expansion)))
                .bg(ColorToken::SurfaceHover)
                .align(AlignItems::Center)
                .justify(JustifyContent::Center)
                .child(text("Map").text_size(FontSizeToken::Xl)),
        )
        .child(keyed(move |r| r.get(list_revision), move |_, _| viewport(expansion, activations)))
        .child(text(move |r: &mut ReadCx<'_>| format!("Activations {}", r.get(activations))).test_id("pan-activations").p(sp(3.0)))
}

#[ui]
fn viewport(expansion: Signal<f32>, activations: Signal<u32>) -> El {
    col()
        .test_id(LIST)
        .style_with(move |r, p| p.h(CLOSED_HEIGHT + r.get(expansion)))
        .overflow_y(Overflow::Scroll)
        .on_touch_scroll_pre(move |w, available| expand(w, expansion, available.y, true))
        .on_touch_scroll_post(move |w, available| expand(w, expansion, available.y, false))
        .children((0..5).map(move |index| {
            el().test_id(format!("pan-place-{index}"))
                .role(Role::Button)
                .label(format!("Place {}", index + 1))
                .focusable()
                .h(40.0)
                .shrink(0.0)
                .px(sp(3.0))
                .justify(JustifyContent::Center)
                .when(State::PRESSED, |p| p.bg(ColorToken::Accent))
                .child(text(format!("Place {}", index + 1)))
                .on_press(move |w| w.update(activations, |count| *count += 1))
        }))
}

fn expand(w: &mut WriteCx<'_>, expansion: Signal<f32>, dy: f32, before: bool) -> HandlerOutcome {
    if (before && dy <= 0.0) || (!before && dy >= 0.0) {
        return HandlerOutcome::default();
    }
    let old = w.get(expansion);
    let next = (old + dy).clamp(0.0, TRAVEL);
    w.set(expansion, next);
    HandlerOutcome { consumed: next != old, scroll: Vec2::new(0.0, next - old) }
}
