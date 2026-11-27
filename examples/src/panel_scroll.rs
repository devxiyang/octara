//! A map panel whose viewport and scroll offset share one touch delta.

use octara::prelude::*;

pub const LIST: &str = "pan-list";
const CLOSED_HEIGHT: f32 = 100.0;
const TRAVEL: f32 = 12.0;
const PANEL: TwStyle = tw!("size-full bg-surface text-text");
const MAP: TwStyle = tw!("bg-surface-hover items-center justify-center");
const PLACE: TwStyle = tw!("h-[40px] shrink-0 px-3 justify-center");

#[ui]
pub fn view(cx: &mut BuildCx<'_>) -> El {
    let expansion = cx.signal(0.0_f32);
    let activations = cx.signal(0_u32);
    let list_revision = cx.signal(0_u32);
    col().tw(PANEL).on_command("pan.replace", move |w| w.update(list_revision, |revision| *revision += 1)).child((
        map(expansion),
        keyed(move |r| r.get(list_revision), move |_, _| viewport(expansion, activations)),
        text(move |r: &mut ReadCx<'_>| format!("Activations {}", r.get(activations))).test_id("pan-activations").p(sp(3.0)),
    ))
}

#[ui]
fn map(expansion: Signal<f32>) -> El {
    el().test_id("pan-map").style_with(move |r, p| p.h(220.0 - r.get(expansion))).tw(MAP).child(text("Map").text_size(FontSizeToken::Xl))
}

#[ui]
fn viewport(expansion: Signal<f32>, activations: Signal<u32>) -> El {
    col()
        .test_id(LIST)
        .style_with(move |r, p| p.h(CLOSED_HEIGHT + r.get(expansion)))
        .overflow_y(Overflow::Scroll)
        .on_touch_scroll_pre(move |w, available| expand(w, expansion, available.y, true))
        .on_touch_scroll_post(move |w, available| expand(w, expansion, available.y, false))
        .children((0..5).map(move |index| place(index, activations)))
}

#[ui]
fn place(index: usize, activations: Signal<u32>) -> El {
    el().test_id(format!("pan-place-{index}"))
        .role(Role::Button)
        .label(format!("Place {}", index + 1))
        .focusable()
        .tw(PLACE)
        .when(State::PRESSED, |p| p.bg(ColorToken::Accent))
        .child(text(format!("Place {}", index + 1)))
        .on_press(move |w| w.update(activations, |count| *count += 1))
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
