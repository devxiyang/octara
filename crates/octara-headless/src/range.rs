//! Numeric controls that use the caller's geometry and presentation.

use crate::Control;
use octara::prelude::*;

/// Parses a complete finite draft while preserving incomplete text in the caller's editor.
pub fn parse_number(draft: &str) -> Option<f64> {
    draft.trim().parse::<f64>().ok().filter(|value| value.is_finite())
}

/// Finite bounds and a positive increment for an editable numeric control.
#[derive(Clone, Copy, Debug)]
pub struct Range {
    min: f64,
    max: f64,
    step: f64,
    decimals: usize,
}

impl Range {
    pub fn new(min: f64, max: f64, step: f64) -> Self {
        assert!(min.is_finite() && max.is_finite() && min < max && (max - min).is_finite(), "range requires finite bounds");
        assert!(step.is_finite() && step > 0.0 && ((max - min) / step).is_finite(), "range requires a positive finite step");
        let decimals = [min, max, step].map(decimal_places).into_iter().max().unwrap_or(0);
        Self { min, max, step, decimals }
    }

    pub fn normalize(self, value: f64) -> f64 {
        let value = if value.is_nan() { self.min } else { value.clamp(self.min, self.max) };
        if value == self.min || value == self.max {
            return value;
        }
        let value = ((value - self.min) / self.step).round().mul_add(self.step, self.min).clamp(self.min, self.max);
        self.decimalize(value)
    }

    /// Formats a clamped and snapped value for a number field.
    pub fn normalize_text(self, value: f64) -> String {
        let normalized = self.normalize(value);
        if normalized == 0.0 { "0".into() } else { normalized.to_string() }
    }

    fn decimalize(self, value: f64) -> f64 {
        let digits = self.decimals;
        format!("{value:.digits$}").parse().expect("formatted finite number")
    }

    fn step_by(self, value: f64, steps: i32) -> f64 {
        let value = self.normalize(value);
        let ratio = (value - self.min) / self.step;
        let nearest = ratio.round();
        // A maximum that is between two steps is itself the final step.
        let index =
            if value == self.max && self.decimalize(nearest.mul_add(self.step, self.min)) != self.max { ratio.ceil() } else { nearest };
        self.normalize((index + f64::from(steps)).mul_add(self.step, self.min))
    }

    fn expose(self, value: Option<f64>) -> NumericRange {
        NumericRange { value, min: self.min, max: self.max, step: Some(self.step) }
    }

    fn request(self, current: Option<f64>, action: &A11yAction) -> Option<Option<f64>> {
        Some(match action {
            A11yAction::SetNumericValue(value) if value.is_finite() => Some(*value),
            A11yAction::Increment => current.map(|value| self.step_by(value, 1)),
            A11yAction::Decrement => current.map(|value| self.step_by(value, -1)),
            _ => return None,
        })
    }
}

fn decimal_places(value: f64) -> usize {
    let value = value.to_string();
    value.split_once('.').map_or(0, |(_, fraction)| fraction.len())
}

#[derive(Clone, Copy)]
pub struct SliderState {
    value: Binding<f64>,
    range: Range,
    bounds: Signal<Option<LayoutBox>>,
    drag_start: Signal<Option<(f64, f32)>>,
}

impl SliderState {
    pub fn new(build: &mut BuildCx<'_>, value: impl Into<Control<f64>>, range: Range) -> Self {
        Self { value: value.into().bind(build), range, bounds: build.signal(None), drag_start: build.signal(None) }
    }

    pub fn value(self, read: &mut ReadCx<'_>) -> f64 {
        self.range.normalize(read.get(self.value))
    }

    pub fn set(self, write: &mut WriteCx<'_>, value: f64) {
        write.set(self.value, self.range.normalize(value));
    }

    /// Attaches horizontal slider behavior to the actual track element.
    pub fn slider<C>(self, element: El<C>) -> El<C> {
        self.slider_with_thumb(element, 0.0)
    }

    /// Keeps a circular thumb inside the track and preserves the pointer's offset when grabbed.
    /// `diameter` is the painted thumb width in logical pixels.
    pub fn slider_with_thumb<C>(self, element: El<C>, diameter: f32) -> El<C> {
        assert!(diameter.is_finite() && diameter >= 0.0, "slider requires a finite nonnegative thumb diameter");
        let inset = diameter / 2.0;
        element
            .role(Role::Slider)
            .observe_geometry(self.bounds)
            .focusable()
            .numeric_range(bind(move |read| self.range.expose(Some(self.value(read)))))
            .behavior_first(|behavior| {
                behavior
                    .on_key(move |write, event| self.key(write, event))
                    .handle(HandlerKind::Accessibility, move |write, event| {
                        let HandlerEvent::Accessibility(action) = event else { return HandlerOutcome::default() };
                        let current = write.read(|read| self.value(read));
                        let Some(Some(value)) = self.range.request(Some(current), action) else { return HandlerOutcome::default() };
                        self.set(write, value);
                        consumed()
                    })
                    .on(HandlerKind::Press, move |write, event| {
                        if let HandlerEvent::Interaction { local_pos: Some(at), .. } = event {
                            self.move_to(write, at.x, inset);
                        }
                    })
                    .on(HandlerKind::Drag, move |write, event| {
                        let HandlerEvent::Interaction { payload: InteractionPayload::Drag(drag), local_pos, .. } = event else { return };
                        match drag.phase {
                            DragPhase::Start => {
                                let before = write.get(self.value);
                                let grab = local_pos.map_or(0.0, |at| self.grab_offset(write, at.x - drag.displacement.x, before, inset));
                                write.set(self.drag_start, Some((before, grab)));
                                if let Some(at) = local_pos {
                                    self.move_to(write, at.x - grab, inset);
                                }
                            }
                            DragPhase::Move | DragPhase::End => {
                                let grab = write.get(self.drag_start).map_or(0.0, |(_, grab)| grab);
                                if let Some(at) = local_pos {
                                    self.move_to(write, at.x - grab, inset);
                                }
                                if drag.phase == DragPhase::End {
                                    write.set(self.drag_start, None);
                                }
                            }
                            DragPhase::Cancel => {
                                if let Some((before, _)) = write.get(self.drag_start) {
                                    write.set(self.value, before);
                                }
                                write.set(self.drag_start, None);
                            }
                        }
                    })
            })
    }

    fn key(self, write: &mut WriteCx<'_>, event: &KeyEvent) -> bool {
        if !event.modifiers.is_empty() {
            return false;
        }
        let value = write.read(|read| self.value(read));
        let target = match event.key {
            Key::Left | Key::Down => self.range.step_by(value, -1),
            Key::Right | Key::Up => self.range.step_by(value, 1),
            Key::PageDown => self.range.step_by(value, -10),
            Key::PageUp => self.range.step_by(value, 10),
            Key::Home => self.range.min,
            Key::End => self.range.max,
            _ => return false,
        };
        self.set(write, target);
        true
    }

    fn move_to(self, write: &mut WriteCx<'_>, x: f32, inset: f32) {
        let Some((left, travel)) = write.get(self.bounds).and_then(|bounds| Self::track(bounds, inset)) else { return };
        let fraction = f64::from(((x - left) / travel).clamp(0.0, 1.0));
        self.set(write, (self.range.max - self.range.min).mul_add(fraction, self.range.min));
    }

    fn grab_offset(self, write: &mut WriteCx<'_>, down: f32, before: f64, inset: f32) -> f32 {
        if inset == 0.0 {
            return 0.0;
        }
        let Some((left, travel)) = write.get(self.bounds).and_then(|bounds| Self::track(bounds, inset)) else { return 0.0 };
        let fraction = (self.range.normalize(before) - self.range.min) / (self.range.max - self.range.min);
        let offset = down - (left + fraction as f32 * travel);
        if offset.abs() <= inset { offset } else { 0.0 }
    }

    fn track(bounds: LayoutBox, inset: f32) -> Option<(f32, f32)> {
        let left = bounds.border.left + bounds.padding.left + inset;
        let right = bounds.border.right + bounds.padding.right + inset;
        let travel = bounds.rect.w - left - right;
        (travel > 0.0).then_some((left, travel))
    }
}

#[derive(Clone, Copy)]
pub struct NumberFieldState {
    draft: Binding<String>,
    range: Range,
}

impl NumberFieldState {
    pub fn new(build: &mut BuildCx<'_>, draft: impl Into<Control<String>>, range: Range) -> Self {
        Self { draft: draft.into().bind(build), range }
    }

    pub fn value(self, read: &mut ReadCx<'_>) -> Option<f64> {
        read.with(self.draft, |draft| parse_number(draft))
    }

    /// Commits a finite draft to the range and preserves an incomplete draft.
    pub fn commit(self, write: &mut WriteCx<'_>) {
        if let Some(value) = write.read(|read| self.value(read)) {
            self.set(write, value);
        }
    }

    fn set(self, write: &mut WriteCx<'_>, value: f64) {
        write.set(self.draft, self.range.normalize_text(value));
    }

    pub fn input(self, element: TextInput) -> TextInput {
        element
            .role(Role::SpinButton)
            .value(self.draft)
            .submit_on_enter()
            .numeric_range(bind(move |read| self.range.expose(self.value(read))))
            .invalid_when(move |read| self.value(read).is_none_or(|value| value < self.range.min || value > self.range.max))
            .behavior_first(|behavior| {
                behavior
                    .on_input(move |write, draft| write.set(self.draft, draft.into()))
                    .on_submit(move |write| self.commit(write))
                    .on(HandlerKind::Blur, move |write, _| self.commit(write))
                    .on_key(move |write, event| {
                        if !event.modifiers.is_empty() {
                            return false;
                        }
                        let step = match event.key {
                            Key::Up => 1,
                            Key::Down => -1,
                            _ => return false,
                        };
                        if let Some(value) = write.read(|read| self.value(read)) {
                            self.set(write, self.range.step_by(value, step));
                        }
                        true
                    })
                    .handle(HandlerKind::Accessibility, move |write, event| {
                        let HandlerEvent::Accessibility(action) = event else { return HandlerOutcome::default() };
                        let value = write.read(|read| self.value(read));
                        let Some(value) = self.range.request(value, action) else { return HandlerOutcome::default() };
                        if let Some(value) = value {
                            self.set(write, value);
                        }
                        consumed()
                    })
            })
    }
}

#[derive(Clone, Copy)]
pub struct ProgressState<S> {
    value: S,
    max: f64,
}

impl<S: Readable<Option<f64>>> ProgressState<S> {
    pub fn new(value: S, max: f64) -> Self {
        assert!(max.is_finite() && max > 0.0, "progress requires a positive finite maximum");
        Self { value, max }
    }

    pub fn value(self, read: &mut ReadCx<'_>) -> Option<f64> {
        read.get(self.value).filter(|value| value.is_finite()).map(|value| value.clamp(0.0, self.max))
    }

    pub fn progress<C>(self, element: El<C>) -> El<C> {
        element.role(Role::ProgressIndicator).numeric_range(bind(move |read| NumericRange {
            value: self.value(read),
            min: 0.0,
            max: self.max,
            step: None,
        }))
    }
}

fn consumed() -> HandlerOutcome {
    HandlerOutcome { consumed: true, ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::Range;

    #[test]
    fn the_maximum_is_a_reversible_step_even_between_grid_points() {
        let range = Range::new(0.0, 10.0, 3.0);
        assert_eq!(range.step_by(9.0, 1), 10.0);
        assert_eq!(range.step_by(10.0, -1), 9.0);
        let decimal = Range::new(0.0, 0.3, 0.1);
        assert_eq!(decimal.step_by(0.3, -1), 0.2);
    }
}
