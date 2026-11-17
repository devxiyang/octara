//! Finite numeric controls. Pointer/keyboard values and accessibility/Inspector requests use
//! the same clamping and stepping. Number drafts remain ordinary text until committed.
//!
//! The small semantic range follows Rune's `range_value`/`range_step`; keyboard and pointer
//! behavior follows its `on_step` and Exact's painted `presenter/range.rs`. Raw number editing
//! follows Exact's `docs/recipes/number-field.md`: normalize on Enter or blur, never per edit.

use crate::{Model, spinner, text_field};
use octara::kernel::NumericRange;
use octara::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// Parses a complete finite number (surrounding whitespace is ignored). Incomplete drafts,
/// empty text, units, NaN and infinity return `None`; this function never rewrites the draft.
pub fn parse_number(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|value| value.is_finite())
}

#[derive(Clone, Copy)]
struct Range {
    min: f64,
    max: f64,
    step: f64,
    decimals: usize,
}

impl Range {
    fn new(min: f64, max: f64, step: f64) -> Self {
        assert!(
            min.is_finite() && max.is_finite() && min < max && (max - min).is_finite(),
            "numeric controls require finite min < max and a finite span"
        );
        assert!(
            step.is_finite() && step > 0.0 && ((max - min) / step).is_finite(),
            "numeric controls require a positive finite step and a finite step count"
        );
        Self { min, max, step, decimals: [min, max, step].map(decimal_places).into_iter().max().unwrap_or(0) }
    }

    fn normalize(self, value: f64) -> f64 {
        let value = if value.is_nan() { self.min } else { value.clamp(self.min, self.max) };
        let value = if value == self.min || value == self.max {
            value
        } else {
            ((value - self.min) / self.step).round().mul_add(self.step, self.min).clamp(self.min, self.max)
        };
        // Decimal steps such as 0.1 should produce 0.3, including in the numeric model.
        self.text(value).parse().unwrap_or(value)
    }

    fn text(self, value: f64) -> String {
        let mut text = format!("{value:.precision$}", precision = self.decimals);
        if text.contains('.') {
            text.truncate(text.trim_end_matches('0').trim_end_matches('.').len());
        }
        if text == "-0" { "0".into() } else { text }
    }

    fn semantics(self, value: Option<f64>) -> NumericRange {
        NumericRange { value, min: self.min, max: self.max, step: Some(self.step) }
    }

    fn at(self, x: f32, width: f32) -> f64 {
        let travel = (width - THUMB).max(1.0);
        self.normalize(self.min + f64::from(((x - THUMB / 2.0) / travel).clamp(0.0, 1.0)) * (self.max - self.min))
    }
}

fn decimal_places(value: f64) -> usize {
    let text = value.to_string();
    let (mantissa, exponent) = text.split_once(['e', 'E']).map_or((text.as_str(), 0), |(m, e)| (m, e.parse::<i32>().unwrap_or(0)));
    let fraction = mantissa.split_once('.').map_or(0, |(_, fraction)| fraction.len()) as i32;
    (fraction - exponent).max(0) as usize
}

/// The text a valid number draft commits: clamped, snapped to a step from `min`, and written
/// without redundant fractional zeros. Invalid/incomplete drafts return `None` unchanged.
/// Like the controls, invalid static bounds/step panic with a clear message.
pub fn normalize_number(text: &str, min: f64, max: f64, step: f64) -> Option<String> {
    let range = Range::new(min, max, step);
    parse_number(text).map(|value| range.text(range.normalize(value)))
}

const THUMB: f32 = 16.0;

/// A horizontal slider; `id` names its single focusable semantic node. Arrows move by `step`,
/// PageUp/PageDown by ten steps, Home/End to the bounds. Pressing the track moves its thumb;
/// dragging follows the pointer and cancellation restores the starting value. Pointer and
/// semantic requests clamp and snap like the keyboard. External model values are normalized
/// for display only. Application press, drag and numeric callbacks run after the model setter;
/// `on_key` receives only unconsumed keys. Add a specific accessible `label` and chain `disabled`
/// as usual. Numeric callbacks receive the requested value; read the model for its accepted value.
#[ui]
#[track_caller]
pub fn slider(id: &str, value: impl Model<f64>, min: f64, max: f64, step: f64) -> El {
    let range = Range::new(min, max, step);
    let value: Rc<dyn Model<f64>> = Rc::new(value);
    let (draw, semantics, press, drag, keys, numeric) = (value.clone(), value.clone(), value.clone(), value.clone(), value.clone(), value);
    let size = size_of(id);
    let id = id.to_owned();
    let press_id = id.clone();
    let start = Cell::new(0.0);
    let grab = Cell::new(0.0f32);
    canvas(move |canvas| {
        let (width, height) = canvas.size();
        let fraction = ((range.normalize(draw.get()) - min) / (max - min)) as f32;
        let center = THUMB / 2.0 + fraction * (width - THUMB).max(0.0);
        let track = Rect { x: THUMB / 2.0, y: height / 2.0 - 2.0, w: (width - THUMB).max(0.0), h: 4.0 };
        canvas.fill(&Path::rounded_rect(track, 2.0), Token::Border);
        canvas.fill(&Path::rounded_rect(Rect { w: (center - track.x).max(0.0), ..track }, 2.0), Token::Accent);
        canvas.fill(&Path::circle(point(center, height / 2.0), THUMB / 2.0), Token::Accent);
    })
    .id(id)
    .role(Role::Slider)
    .label("Value")
    .numeric_range(move || range.semantics(Some(range.normalize(semantics.get()))))
    .focusable()
    .w(200.0)
    .min_w(44.0)
    .h(28.0)
    .when_touch(|style| style.h(crate::TOUCH_TARGET))
    .border()
    .border_color(Color::TRANSPARENT)
    .rounded_md()
    .focus_visible(|style| style.border_color(Token::Focus))
    .style_when(State::DISABLED, |style| style.opacity(0.5))
    .behavior(|b| {
        b.on_numeric_input(move |value| numeric.set(range.normalize(value)))
            .on_press(move || {
                if let Some((x, _)) = pointer_position_in(&press_id) {
                    instant(|| press.set(range.at(x - 1.0, size.get().0 - 2.0)));
                }
            })
            .on_drag(move |event| {
                let width = size.get().0 - 2.0;
                match event.phase {
                    DragPhase::Start => {
                        start.set(drag.get());
                        let fraction = ((range.normalize(start.get()) - min) / (max - min)) as f32;
                        let center = THUMB / 2.0 + fraction * (width - THUMB).max(0.0);
                        let down = event.x - event.dx - 1.0;
                        grab.set(if (down - center).abs() <= THUMB / 2.0 + 3.0 { down - center } else { 0.0 });
                        drag.set(range.at(event.x - 1.0 - grab.get(), width));
                    }
                    DragPhase::Move | DragPhase::End => drag.set(range.at(event.x - 1.0 - grab.get(), width)),
                    DragPhase::Cancel => drag.set(start.get()),
                }
            })
            .on_key(move |event| {
                if event.modifiers.intersects(Modifiers::CTRL | Modifiers::META | Modifiers::ALT) {
                    return false;
                }
                let value = range.normalize(keys.get());
                let next = match event.key {
                    Key::Left | Key::Down => value - step,
                    Key::Right | Key::Up => value + step,
                    Key::PageDown => value - step * 10.0,
                    Key::PageUp => value + step * 10.0,
                    Key::Home => min,
                    Key::End => max,
                    _ => return false,
                };
                keys.set(range.normalize(next));
                true
            })
    })
}

/// A number's raw text editor, with numeric bounds and step exposed to accessibility and
/// Inspector. Typing keeps the exact draft (including empty text, signs and exponents); Enter
/// and blur normalize a complete finite number, while an invalid draft stays unchanged.
/// Up/Down step a complete number. The system IME owns these keys while composing.
///
/// Application input, numeric, submit and blur callbacks run after the component updates or
/// commits its model. They preserve the binding and normalization; `on_key` receives unconsumed
/// keys. Invalid drafts still notify submit/blur, so validate before saving.
/// Add a label or wrap this `El` with [`crate::field`] for required state and explanatory errors.
#[ui]
#[track_caller]
pub fn number_field(value: impl Model<String>, min: f64, max: f64, step: f64) -> El {
    let range = Range::new(min, max, step);
    let value: Rc<dyn Model<String>> = Rc::new(value);
    let (read, edit, parsed, submit, blur, keys, numeric) =
        (value.clone(), value.clone(), value.clone(), value.clone(), value.clone(), value.clone(), value);
    let number = memo(move || parse_number(&parsed.get()));
    text_field((move || read.get(), move |text| edit.set(text)))
        .role(Role::SpinButton)
        .numeric_range(move || range.semantics(number.get()))
        .invalid(move || number.get().is_none_or(|value| value < min || value > max))
        .behavior(|b| {
            b.on_numeric_input(move |value| numeric.set(range.text(range.normalize(value))))
                .on_submit(move || {
                    if let Some(number) = parse_number(&submit.get()) {
                        submit.set(range.text(range.normalize(number)));
                    }
                })
                .on_blur(move || {
                    if let Some(number) = parse_number(&blur.get()) {
                        blur.set(range.text(range.normalize(number)));
                    }
                })
                .on_key(move |event| {
                    if !event.modifiers.is_empty() {
                        return false;
                    }
                    let by = match event.key {
                        Key::Up => step,
                        Key::Down => -step,
                        _ => return false,
                    };
                    if let Some(number) = parse_number(&keys.get()) {
                        keys.set(range.text(range.normalize(number + by)));
                    }
                    true
                })
        })
}

/// Progress from zero to positive finite `max`. `Some(value)` paints a bounded bar and
/// publishes its number; `None` reuses [`spinner`] and publishes no current number. Non-finite
/// dynamic values also count as unknown. This is read-only and never takes keyboard focus.
#[ui]
#[track_caller]
pub fn progress(value: impl Fn() -> Option<f64> + 'static, max: f64) -> El {
    assert!(max.is_finite() && max > 0.0, "progress requires a positive finite maximum");
    let value = memo(move || value().filter(|value| value.is_finite()).map(|value| value.clamp(0.0, max)));
    el().role(Role::ProgressIndicator)
        .label("Progress")
        .numeric_range(move || NumericRange { value: value.get(), min: 0.0, max, step: None })
        .w(160.0)
        .min_h(16.0)
        .items_center()
        .child(show_else(
            move || value.get().is_some(),
            move || {
                canvas(move |canvas| {
                    let (width, height) = canvas.size();
                    let bounds = Rect { x: 0.0, y: 0.0, w: width, h: height };
                    canvas.fill(&Path::rounded_rect(bounds, height / 2.0), Token::Border);
                    let fraction = (value.get().unwrap_or(0.0) / max) as f32;
                    canvas.fill(&Path::rounded_rect(Rect { w: width * fraction, ..bounds }, height / 2.0), Token::Accent);
                })
                .w_full()
                .h(6.0)
                .a11y_hidden()
                .into()
            },
            || spinner().a11y_hidden().into(),
        ))
}
