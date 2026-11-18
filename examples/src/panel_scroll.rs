//! A map panel whose viewport and scroll offset share one touch delta.

use octara::app::TouchScrollStage;
use octara::prelude::*;

pub const LIST: &str = "pan-list";
const CLOSED_HEIGHT: f32 = 100.0;
const TRAVEL: f32 = 12.0;

#[ui]
pub fn view() -> El {
    let expansion = state("expansion", || 0.0_f32);
    let activations = state("activations", || 0_u32);
    let list_revision = state("list-revision", || 0_u32);
    on_command("pan.replace", move || list_revision.update(|revision| *revision += 1));

    col()
        .size_full()
        .bg(Token::Surface)
        .text_color(Token::Text)
        .child(
            el().id("pan-map")
                .style_with(move |p| p.h(220.0 - expansion.get()))
                .bg(Token::SurfaceHover)
                .items_center()
                .justify_center()
                .child(text("Map").text_xl()),
        )
        .child(keyed(move || list_revision.get(), move |_| viewport(expansion, activations)))
        .child(text(move || format!("Activations {}", activations.get())).id("pan-activations").p_3())
}

#[ui]
fn viewport(expansion: Signal<f32>, activations: Signal<u32>) -> El {
    col()
        .id(LIST)
        .style_with(move |p| p.h(CLOSED_HEIGHT + expansion.get()))
        .overflow_y_scroll()
        .on_touch_scroll(move |stage, [_, dy]| {
            let panel_turn = match stage {
                TouchScrollStage::Before => dy > 0.0,
                TouchScrollStage::After => dy < 0.0,
            };
            if !panel_turn {
                return [0.0; 2];
            }
            let old = expansion.get();
            let next = (old + dy).clamp(0.0, TRAVEL);
            expansion.set(next);
            [0.0, next - old]
        })
        .children((0..5).map(move |index| {
            el().id(format!("pan-place-{index}"))
                .role(Role::Button)
                .label(format!("Place {}", index + 1))
                .focusable()
                .h(40.0)
                .px_3()
                .justify_center()
                .style_when(State::PRESSED, |p| p.bg(Token::Accent))
                .child(text(format!("Place {}", index + 1)))
                .on_press(move || activations.update(|count| *count += 1))
        }))
}
