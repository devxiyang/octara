//! Styled numeric controls driven by Headless range state.

use crate::{KitLocale, TOUCH_TARGET, parts::editor_style, spinner};
use octara::prelude::*;
use octara_headless::{NumberFieldState, ProgressState, SliderState, range::Range};

const THUMB: f32 = 16.0;

/// A horizontal slider. Arrows step, PageUp/PageDown move by ten steps, and Home/End
/// move to the bounds. Cancelling a drag restores its previous value.
/// Use `label` to name the edited value for assistive technology.
#[ui]
#[track_caller]
pub fn slider(build: &mut BuildCx<'_>, id: &str, value: Binding<f64>, min: f64, max: f64, step: f64) -> El {
    let state = SliderState::new(build, value, Range::new(min, max, step));
    let track = canvas(move |r, canvas| {
        let octara::Size { width, height } = canvas.size();
        let fraction = ((state.value(r) - min) / (max - min)) as f32;
        let travel = (width - THUMB).max(0.0);
        let track = Rect::new(THUMB / 2.0, height / 2.0 - 2.0, travel, 4.0);
        let filled = Rect { w: travel * fraction, ..track };
        let center = track.x + filled.w;
        canvas.fill(&Path::rounded_rect(track, 2.0), ColorToken::Border);
        canvas.fill(&Path::rounded_rect(filled, 2.0), ColorToken::Accent);
        canvas.fill(&Path::circle(point(center, height / 2.0), THUMB / 2.0), ColorToken::Accent);
    })
    .test_id(id.to_owned())
    .label(KitLocale::get(build).value)
    .w(200.0)
    .min_w(TOUCH_TARGET)
    .h(28.0)
    .when(When::Touch, |style| style.min_h(TOUCH_TARGET))
    .border(1.0)
    .border_color(Color::TRANSPARENT)
    .rounded(RadiusToken::Md)
    .when(State::FOCUS_VISIBLE, |style| style.border_color(ColorToken::Focus))
    .when(State::DISABLED, |style| style.opacity(0.5));
    state.slider_with_thumb(track, THUMB)
}

/// A number's raw text editor. Enter or blur commits a finite draft, and Up/Down step
/// a complete value. Application callbacks observe the updated binding.
#[ui]
#[track_caller]
pub fn number_field(build: &mut BuildCx<'_>, value: Binding<String>, min: f64, max: f64, step: f64) -> TextInput {
    NumberFieldState::new(build, value, Range::new(min, max, step)).input(editor_style(input()))
}

/// Progress measured from zero to a positive finite `max`. Missing and nonfinite values
/// display an indeterminate indicator. Finite values paint a bounded bar.
#[ui]
#[track_caller]
pub fn progress(build: &mut BuildCx<'_>, value: impl Readable<Option<f64>>, max: f64) -> El {
    let state = ProgressState::new(value, max);
    state.progress(el().label(KitLocale::get(build).progress).w(160.0).min_h(16.0).align(AlignItems::Center).child(keyed(
        move |r| state.value(r).is_some(),
        move |b, known| {
            let indicator = if *known {
                canvas(move |r, canvas| {
                    let octara::Size { width, height } = canvas.size();
                    let bounds = Rect::new(0.0, 0.0, width, height);
                    let fraction = (state.value(r).unwrap_or(0.0) / max) as f32;
                    canvas.fill(&Path::rounded_rect(bounds, height / 2.0), ColorToken::Border);
                    canvas.fill(&Path::rounded_rect(Rect { w: width * fraction, ..bounds }, height / 2.0), ColorToken::Accent);
                })
                .w(rel(1.0))
                .h(6.0)
            } else {
                spinner(b)
            };
            indicator.a11y_hidden()
        },
    )))
}
