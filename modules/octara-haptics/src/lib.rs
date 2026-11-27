//! Typed haptic feedback requests and a deterministic test backend.

use std::any::Any;

use octara::module::{CallTicket, HostSurfaceKind, ModuleCall, ModuleCallCx, ModuleError, ModuleId, OctaraModule};

const ID: ModuleId = ModuleId("octara.haptics");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImpactStyle {
    Light,
    Medium,
    Heavy,
    Soft,
    Rigid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notification {
    Success,
    Warning,
    Error,
}

/// Play feedback associated with a user action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Haptic {
    Impact(ImpactStyle),
    Notification(Notification),
    Selection,
}

impl ModuleCall for Haptic {
    const MODULE: ModuleId = ID;
    const METHOD: &'static str = "play";
    type Output = ();
}

pub fn module() -> impl OctaraModule {
    Haptics(play)
}

/// Accepts feedback requests without accessing an operating system.
pub fn mock() -> impl OctaraModule {
    mock_with(|_| {})
}

/// Observe feedback requests in a deterministic test.
pub fn mock_with(mut observe: impl FnMut(Haptic) + 'static) -> impl OctaraModule {
    Haptics(move |_, feedback| {
        observe(feedback);
        Ok(())
    })
}

struct Haptics<F>(F);

impl<F: FnMut(HostSurfaceKind, Haptic) -> Result<(), ModuleError> + 'static> OctaraModule for Haptics<F> {
    fn id(&self) -> ModuleId {
        ID
    }

    fn call(&mut self, _: CallTicket, request: Box<dyn Any>, cx: &mut ModuleCallCx<'_>) {
        let result = cx.downcast::<Haptic>(request).and_then(|feedback| (self.0)(cx.host_kind(), feedback));
        let _ = cx.reply::<Haptic>(result);
    }
}

#[cfg(all(feature = "platform", target_os = "ios"))]
#[allow(deprecated)] // These initializers support iOS versions before the view-based factories.
fn play(kind: HostSurfaceKind, feedback: Haptic) -> Result<(), ModuleError> {
    use objc2::MainThreadMarker;
    use objc2_ui_kit::{
        UIImpactFeedbackGenerator, UIImpactFeedbackStyle, UINotificationFeedbackGenerator, UINotificationFeedbackType,
        UISelectionFeedbackGenerator,
    };

    if kind != HostSurfaceKind::Ios {
        return Err(ModuleError::Unavailable);
    }
    let main = MainThreadMarker::new().ok_or(ModuleError::Unavailable)?;
    match feedback {
        Haptic::Impact(style) => {
            let style = match style {
                ImpactStyle::Light => UIImpactFeedbackStyle::Light,
                ImpactStyle::Medium => UIImpactFeedbackStyle::Medium,
                ImpactStyle::Heavy => UIImpactFeedbackStyle::Heavy,
                ImpactStyle::Soft => UIImpactFeedbackStyle::Soft,
                ImpactStyle::Rigid => UIImpactFeedbackStyle::Rigid,
            };
            UIImpactFeedbackGenerator::initWithStyle(main.alloc(), style).impactOccurred();
        }
        Haptic::Notification(kind) => {
            let kind = match kind {
                Notification::Success => UINotificationFeedbackType::Success,
                Notification::Warning => UINotificationFeedbackType::Warning,
                Notification::Error => UINotificationFeedbackType::Error,
            };
            UINotificationFeedbackGenerator::new(main).notificationOccurred(kind);
        }
        Haptic::Selection => UISelectionFeedbackGenerator::new(main).selectionChanged(),
    }
    Ok(())
}

#[cfg(all(feature = "platform", target_os = "macos"))]
fn play(kind: HostSurfaceKind, feedback: Haptic) -> Result<(), ModuleError> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSHapticFeedbackManager, NSHapticFeedbackPattern, NSHapticFeedbackPerformanceTime, NSHapticFeedbackPerformer};

    if kind != HostSurfaceKind::MacOs || MainThreadMarker::new().is_none() {
        return Err(ModuleError::Unavailable);
    }
    let pattern = match feedback {
        Haptic::Impact(_) => NSHapticFeedbackPattern::Generic,
        Haptic::Notification(Notification::Success) => NSHapticFeedbackPattern::Alignment,
        Haptic::Notification(_) | Haptic::Selection => NSHapticFeedbackPattern::LevelChange,
    };
    NSHapticFeedbackManager::defaultPerformer().performFeedbackPattern_performanceTime(pattern, NSHapticFeedbackPerformanceTime::Now);
    Ok(())
}

#[cfg(all(feature = "platform", target_arch = "wasm32"))]
fn play(kind: HostSurfaceKind, feedback: Haptic) -> Result<(), ModuleError> {
    use js_sys::{Function, Reflect};
    use wasm_bindgen::{JsCast, JsValue};

    if kind != HostSurfaceKind::Web {
        return Err(ModuleError::Unavailable);
    }
    let navigator = web_sys::window().ok_or(ModuleError::Unavailable)?.navigator();
    let vibrate = Reflect::get(navigator.as_ref(), &JsValue::from_str("vibrate"))
        .ok()
        .and_then(|value| value.dyn_into::<Function>().ok())
        .ok_or(ModuleError::Unavailable)?;
    let milliseconds = match feedback {
        Haptic::Impact(ImpactStyle::Soft | ImpactStyle::Light) | Haptic::Selection => 12,
        Haptic::Impact(ImpactStyle::Medium) | Haptic::Notification(Notification::Success) => 24,
        Haptic::Impact(ImpactStyle::Heavy | ImpactStyle::Rigid) | Haptic::Notification(Notification::Warning | Notification::Error) => 40,
    };
    let accepted = vibrate.call1(navigator.as_ref(), &JsValue::from(milliseconds)).map_err(|_| ModuleError::Unavailable)?;
    if accepted.as_bool() == Some(true) { Ok(()) } else { Err(ModuleError::ActivationRequired) }
}

#[cfg(not(all(feature = "platform", any(target_os = "ios", target_os = "macos", target_arch = "wasm32"))))]
fn play(_: HostSurfaceKind, _: Haptic) -> Result<(), ModuleError> {
    Err(ModuleError::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara::prelude::{LayoutBox, Point};
    use octara::{App, AppDefinition, CommitOutput, HostEngine, ProjectError, ProjectFeedback, WindowViewport};
    use octara::{NodeId, Tree, TreeDelta};
    use std::{cell::RefCell, rc::Rc};

    struct Headless;
    impl HostEngine for Headless {
        fn project(&mut self, _: &Tree, _: &TreeDelta, _: WindowViewport, _: f64) -> Result<ProjectFeedback, ProjectError> {
            Ok(ProjectFeedback::default())
        }
        fn commit(&mut self) -> CommitOutput {
            CommitOutput::default()
        }
        fn rollback(&mut self) {}
        fn hit_test(&self, _: &Tree, _: Point) -> Option<octara::ContactTarget> {
            None
        }
        fn geometry(&self, _: NodeId) -> Option<LayoutBox> {
            None
        }
    }

    #[test]
    fn mock_observes_typed_feedback_in_order() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let received = log.clone();
        let mut app = App::new(|_, _| Box::new(Headless));
        AppDefinition::new().with_module(mock_with(move |feedback| received.borrow_mut().push(feedback))).launch(&mut app, None).unwrap();
        app.update(|w| {
            w.notify_module(Haptic::Selection);
            w.call_module(Haptic::Impact(ImpactStyle::Rigid), |_, result| assert_eq!(result, Ok(())));
        })
        .unwrap();
        assert_eq!(*log.borrow(), [Haptic::Selection, Haptic::Impact(ImpactStyle::Rigid)]);
    }
}
