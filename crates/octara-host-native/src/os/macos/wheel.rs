//! Preserve AppKit's phase at the point a scroll event reaches winit's view.

use objc2::runtime::{Imp, Sel};
use objc2::sel;
use objc2_app_kit::{NSEvent, NSEventPhase, NSView};
use octara_model::input::{WheelPhase, WheelStage};
use std::cell::Cell;
use std::mem::{DropGuard, transmute};
use std::ptr;
use std::sync::OnceLock;
use winit::dpi::LogicalPosition;
use winit::event::{MouseScrollDelta, TouchPhase};

type ScrollWheel = extern "C-unwind" fn(&NSView, Sel, &NSEvent);
static ORIGINAL_SCROLL_WHEEL: OnceLock<Option<ScrollWheel>> = OnceLock::new();

thread_local! {
    static CURRENT_WHEEL: Cell<Option<WheelSample>> = const { Cell::new(None) };
}

/// Observe only the `scrollWheel:` implementation of winit's content view. AppKit still
/// chooses the responder, so a scroll consumed by a native child never enters this hook.
pub(super) fn install(view: &NSView) {
    // AccessKit can subclass the content view before the platform window is created.
    let Some(class) = std::iter::successors(Some(view.class()), |class| class.superclass()).find(|class| class.name() == c"WinitView")
    else {
        return;
    };
    ORIGINAL_SCROLL_WHEEL.get_or_init(|| {
        let selector = sel!(scrollWheel:);
        class.verify_sel::<(&NSEvent,), ()>(selector).ok()?;
        let methods = class.instance_methods();
        let method = methods.iter().find(|method| method.name() == selector)?;
        // SAFETY: `view` comes from winit's live window handle on the main thread.
        // The verified method is declared on winit's view class, and both implementations use
        // the same selector, object argument and return ABI. AppKit dispatch stays on this thread.
        let original = unsafe { method.set_implementation(transmute::<ScrollWheel, Imp>(scroll_wheel)) };
        // SAFETY: This is the previous implementation of the verified scrollWheel: method.
        Some(unsafe { transmute::<Imp, ScrollWheel>(original) })
    });
}

extern "C-unwind" fn scroll_wheel(view: &NSView, selector: Sel, event: &NSEvent) {
    let original = ORIGINAL_SCROLL_WHEEL.get().and_then(|implementation| *implementation).expect("winit scrollWheel: implementation");
    let sample = view.window().map(|window| {
        let (x, y) = (event.scrollingDeltaX(), event.scrollingDeltaY());
        let delta = if event.hasPreciseScrollingDeltas() {
            MouseScrollDelta::PixelDelta(LogicalPosition::new(x, y).to_physical(window.backingScaleFactor()))
        } else {
            MouseScrollDelta::LineDelta(x as f32, y as f32)
        };
        WheelSample::new(ptr::from_ref(view), delta, event.phase(), event.momentumPhase())
    });
    with_sample(sample, || original(view, selector, event));
}

fn with_sample(sample: Option<WheelSample>, dispatch: impl FnOnce()) {
    CURRENT_WHEEL.with(|current| {
        let _restore = DropGuard::new(current.replace(sample), |outer| current.set(outer));
        dispatch();
    });
}

pub(super) fn take_phase(view: &NSView, delta: MouseScrollDelta, phase: TouchPhase) -> Option<WheelPhase> {
    take_sample(ptr::from_ref(view), delta, phase)
}

// Winit normally delivers the wheel callback synchronously after its cursor/modifier updates.
// When it defers a reentrant callback, the scope has expired and the caller uses winit's phase.
fn take_sample(view: *const NSView, delta: MouseScrollDelta, phase: TouchPhase) -> Option<WheelPhase> {
    CURRENT_WHEEL.with(|current| {
        let sample = current.get()?;
        if (sample.view, sample.delta, sample.winit_phase) != (view, delta, phase) {
            return None;
        }
        current.set(None);
        Some(sample.phase)
    })
}

#[derive(Clone, Copy)]
struct WheelSample {
    view: *const NSView,
    delta: MouseScrollDelta,
    winit_phase: TouchPhase,
    phase: WheelPhase,
}

impl WheelSample {
    fn new(view: *const NSView, delta: MouseScrollDelta, direct: NSEventPhase, momentum: NSEventPhase) -> Self {
        // Match the lossy phase emitted by winit 0.30. Its momentum transition phases
        // take priority, and both native terminal phases become TouchPhase::Ended.
        let winit_phase = winit_transition(momentum).or_else(|| winit_transition(direct)).unwrap_or(TouchPhase::Moved);
        let phase = match (stage(direct), stage(momentum)) {
            (_, Some(stage)) => WheelPhase::Momentum(stage),
            (Some(stage), None) => WheelPhase::Direct(stage),
            (None, None) => WheelPhase::Discrete,
        };
        Self { view, delta, winit_phase, phase }
    }
}

fn winit_transition(phase: NSEventPhase) -> Option<TouchPhase> {
    match phase {
        NSEventPhase::MayBegin | NSEventPhase::Began => Some(TouchPhase::Started),
        NSEventPhase::Ended | NSEventPhase::Cancelled => Some(TouchPhase::Ended),
        _ => None,
    }
}

fn stage(phase: NSEventPhase) -> Option<WheelStage> {
    Some(match phase {
        NSEventPhase::None => return None,
        NSEventPhase::MayBegin | NSEventPhase::Began => WheelStage::Start,
        NSEventPhase::Ended => WheelStage::End,
        NSEventPhase::Cancelled => WheelStage::Cancel,
        _ => WheelStage::Move,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::PhysicalPosition;

    #[test]
    fn preserves_both_appkit_wheel_lifecycles() {
        let delta = MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 0.0));
        let view = ptr::null();
        let none = NSEventPhase::None;
        for (native, winit, stage) in [
            (NSEventPhase::MayBegin, TouchPhase::Started, WheelStage::Start),
            (NSEventPhase::Began, TouchPhase::Started, WheelStage::Start),
            (NSEventPhase::Changed, TouchPhase::Moved, WheelStage::Move),
            (NSEventPhase::Stationary, TouchPhase::Moved, WheelStage::Move),
            (NSEventPhase::Ended, TouchPhase::Ended, WheelStage::End),
            (NSEventPhase::Cancelled, TouchPhase::Ended, WheelStage::Cancel),
        ] {
            for (direct, momentum, expected) in [(native, none, WheelPhase::Direct(stage)), (none, native, WheelPhase::Momentum(stage))] {
                with_sample(Some(WheelSample::new(view, delta, direct, momentum)), || {
                    assert_eq!(take_sample(view, delta, winit), Some(expected));
                });
            }
        }
        let discrete = WheelSample::new(view, MouseScrollDelta::LineDelta(0.0, 1.0), none, none);
        assert_eq!(discrete.winit_phase, TouchPhase::Moved);
        assert_eq!(discrete.phase, WheelPhase::Discrete);
    }

    #[test]
    fn consumes_only_the_matching_callback_during_dispatch() {
        // Use distinct identities without constructing AppKit objects on a test thread.
        let (first, second) = (0u8, 1u8);
        let first = ptr::from_ref(&first).cast();
        let second = ptr::from_ref(&second).cast();
        let delta = MouseScrollDelta::LineDelta(0.0, 1.0);
        let outer = WheelSample::new(first, delta, NSEventPhase::Ended, NSEventPhase::None);
        let inner = WheelSample::new(second, delta, NSEventPhase::None, NSEventPhase::Cancelled);
        with_sample(Some(outer), || {
            assert_eq!(take_sample(first, delta, TouchPhase::Started), None);
            assert_eq!(take_sample(first, MouseScrollDelta::LineDelta(0.0, 2.0), TouchPhase::Ended), None);
            with_sample(Some(inner), || {
                assert_eq!(take_sample(first, delta, TouchPhase::Ended), None);
                assert_eq!(take_sample(second, delta, TouchPhase::Ended), Some(WheelPhase::Momentum(WheelStage::Cancel)));
                assert_eq!(take_sample(second, delta, TouchPhase::Ended), None);
            });
            assert_eq!(take_sample(first, delta, TouchPhase::Ended), Some(WheelPhase::Direct(WheelStage::End)));
            assert_eq!(take_sample(first, delta, TouchPhase::Ended), None);
        });
        with_sample(Some(inner), || {});
        assert_eq!(take_sample(second, delta, TouchPhase::Ended), None);
    }
}
