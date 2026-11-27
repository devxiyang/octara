//! Shared pointer and keyboard resizing for split panes and table headers.

use octara::prelude::*;

#[derive(Clone, Copy)]
pub struct SplitterState {
    width: Binding<f32>,
    before_drag: Signal<Option<f32>>,
    step: f32,
    shift_step: Option<f32>,
}

impl SplitterState {
    pub fn new(build: &mut BuildCx<'_>, width: Binding<f32>) -> Self {
        Self::with_steps(build, width, 8.0, Some(40.0))
    }

    pub(crate) fn with_steps(build: &mut BuildCx<'_>, width: Binding<f32>, step: f32, shift_step: Option<f32>) -> Self {
        Self { width, before_drag: build.signal(None), step, shift_step }
    }

    /// Attaches horizontal resize behavior. The binding owns bounds and acceptance.
    pub fn handle<C>(self, element: El<C>) -> El<C> {
        self.part(element, None).a11y_value(bind(move |read| read.get(self.width).round().to_string()))
    }

    pub(crate) fn bounded<C>(self, element: El<C>, min: f32, max: f32) -> El<C> {
        self.part(element, Some((min, max)))
            .numeric_range(bind(move |read| NumericRange {
                min: min.into(),
                max: max.into(),
                value: Some(read.get(self.width).into()),
                step: Some(self.step.into()),
            }))
            .behavior_first(|behavior| {
                behavior.handle(HandlerKind::Accessibility, move |write, event| {
                    let current = write.get(self.width);
                    let width = match event {
                        HandlerEvent::Accessibility(A11yAction::SetNumericValue(value)) if value.is_finite() => {
                            value.clamp(min.into(), max.into()) as f32
                        }
                        HandlerEvent::Accessibility(A11yAction::Increment) => current + self.step,
                        HandlerEvent::Accessibility(A11yAction::Decrement) => current - self.step,
                        _ => return HandlerOutcome::default(),
                    };
                    write.set(self.width, width.clamp(min, max));
                    HandlerOutcome { consumed: true, ..Default::default() }
                })
            })
    }

    fn part<C>(self, element: El<C>, bounds: Option<(f32, f32)>) -> El<C> {
        element.role(Role::Splitter).focusable().behavior_first(|behavior| {
            behavior
                .on(HandlerKind::Drag, move |write, event| {
                    let HandlerEvent::Interaction { payload: InteractionPayload::Drag(drag), .. } = event else { return };
                    if drag.phase == DragPhase::Start {
                        let width = write.get(self.width);
                        write.set(self.before_drag, Some(width));
                    }
                    if let Some(start) = write.get(self.before_drag) {
                        write.set(self.width, if drag.phase == DragPhase::Cancel { start } else { start + drag.displacement.x });
                    }
                    if matches!(drag.phase, DragPhase::Cancel | DragPhase::End) {
                        write.set(self.before_drag, None);
                    }
                })
                .on_key(move |write, event| {
                    let step = match event.modifiers {
                        modifiers if modifiers.is_empty() => self.step,
                        Modifiers::SHIFT if self.shift_step.is_some() => self.shift_step.unwrap_or(self.step),
                        _ => return false,
                    };
                    let current = write.get(self.width);
                    let width = match (event.key, bounds) {
                        (Key::Left, _) => current - step,
                        (Key::Right, _) => current + step,
                        (Key::Home, Some((min, _))) => min,
                        (Key::End, Some((_, max))) => max,
                        _ => return false,
                    };
                    write.set(self.width, width);
                    true
                })
        })
    }
}
