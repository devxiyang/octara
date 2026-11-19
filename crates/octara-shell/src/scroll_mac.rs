//! Retain each wheel event's AppKit data at the content view that receives it.
//! The inherited scrollWheel: still drives winit. Its MouseWheel delivery takes
//! the next sample from this view, in capture order, before Octara routes it.

use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_app_kit::{NSEvent, NSEventPhase, NSView};
use octara::app::{InputEvent, Phase, WheelPhase, Window as AppWindow};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{self, Write};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Instant;
use winit::event::TouchPhase;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

static RECEIVER_CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
thread_local! {
    static RECEIVERS: RefCell<HashMap<*const NSView, Samples>> = RefCell::default();
}

/// Tied to the shell window. A native subview handles its own events before they
/// can reach this receiver, exactly as when winit owns the content view alone.
pub(crate) struct Bridge {
    view: *const NSView,
    samples: Samples,
}

#[derive(Clone)]
struct Samples {
    pending: Rc<RefCell<VecDeque<Sample>>>,
    trace: Option<Trace>,
}

impl Bridge {
    pub(crate) fn attach(window: &Window) -> Option<Self> {
        let _main = MainThreadMarker::new()?;
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::AppKit(raw) = handle.as_raw() else { return None };
        // SAFETY: the handle lends the window's live NSView on the main thread.
        let view: &NSView = unsafe { raw.ns_view.cast().as_ref() };
        let native = view.window()?;
        let object: &AnyObject = view;
        let parent = object.class();
        let subclass = match RECEIVER_CLASS.get() {
            Some(&class) if class.superclass() == Some(parent) => class,
            Some(_) => return None,
            None => {
                let mut class = ClassBuilder::new(c"OctaraScrollReceiver", parent)?;
                // SAFETY: the single override uses the NSResponder scrollWheel: signature.
                unsafe {
                    class.add_method(sel!(scrollWheel:), scroll_wheel as unsafe extern "C-unwind" fn(*const NSView, Sel, *const NSEvent));
                }
                let class = class.register();
                RECEIVER_CLASS.set(class).expect("registered once on the main thread");
                class
            }
        };
        let trace = std::env::var_os("OCTARA_TRACE_SCROLL")
            .is_some_and(|value| value != "0")
            .then(|| Trace { window: native.windowNumber(), started: Instant::now() });
        let samples = Samples { pending: Rc::default(), trace };
        let key = std::ptr::from_ref(view);
        RECEIVERS.with_borrow_mut(|receivers| receivers.insert(key, samples.clone()));
        // SAFETY: this changes only this main-thread instance. The immediate subclass
        // adds no ivars and forwards the compatible override to the original class.
        let old = unsafe { AnyObject::set_class(object, subclass) };
        assert_eq!(old, parent, "the content view's class changed during attachment");
        if let Some(trace) = trace {
            trace.write("attached", json!({}));
        }
        Some(Self { view: key, samples })
    }

    pub(crate) fn next(&self) -> Option<Delivery> {
        self.samples.pending.borrow_mut().pop_front().map(|sample| Delivery { sample, trace: self.samples.trace })
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // AppKit can still own the view while closing. Its override then forwards
        // directly to winit, and any queued samples are freed with the shell host.
        let _ = RECEIVERS.try_with(|receivers| receivers.borrow_mut().remove(&self.view));
    }
}

unsafe extern "C-unwind" fn scroll_wheel(view: *const NSView, _: Sel, event: *const NSEvent) {
    // SAFETY: AppKit supplies the receiving view and a non-null event for scrollWheel:.
    let (view, event) = unsafe { (&*view, &*event) };
    let _ = RECEIVERS.try_with(|receivers| {
        if let Some(samples) = receivers.borrow().get(&std::ptr::from_ref(view)) {
            samples.capture(view, event);
        }
    });
    // SAFETY: this override is installed only on a subclass of the content view.
    // The superclass has the original winit method and receives the untouched event.
    let parent = RECEIVER_CLASS.get().and_then(|class| class.superclass()).expect("registered scroll receiver");
    unsafe { msg_send![super(view, parent), scrollWheel: event] }
}

struct Sample {
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
    phase: Option<WheelPhase>,
}

impl Samples {
    fn capture(&self, view: &NSView, native: &NSEvent) {
        let direct = native.phase();
        let momentum = native.momentumPhase();
        let precise = native.hasPreciseScrollingDeltas();
        let location = native.locationInWindow();
        let local = view.convertPoint_fromView(location, None);
        let bounds = view.bounds();
        let y = local.y - bounds.origin.y;
        let raw = [native.scrollingDeltaX(), native.scrollingDeltaY()];
        let unit = if precise { 1.0 } else { super::LINE };
        self.pending.borrow_mut().push_back(Sample {
            x: (local.x - bounds.origin.x) as f32,
            y: (if view.isFlipped() { y } else { bounds.size.height - y }) as f32,
            dx: -(raw[0] as f32) * unit,
            dy: -(raw[1] as f32) * unit,
            phase: phase(direct, momentum),
        });
        if let Some(trace) = self.trace {
            trace.write(
                "appkit",
                json!({
                    "direct_phase": direct.0,
                    "momentum_phase": momentum.0,
                    "precise": precise,
                    "location_in_window": [location.x, location.y],
                    "scrolling_delta": raw,
                    "timestamp": native.timestamp(),
                }),
            );
        }
    }
}

fn phase(direct: NSEventPhase, momentum: NSEventPhase) -> Option<WheelPhase> {
    fn normalize(flags: NSEventPhase) -> Option<Phase> {
        if flags.contains(NSEventPhase::Cancelled) {
            Some(Phase::Cancel)
        } else if flags.contains(NSEventPhase::Ended) {
            Some(Phase::Up)
        } else if flags.contains(NSEventPhase::Began) {
            Some(Phase::Down)
        } else if flags.contains(NSEventPhase::Changed) {
            Some(Phase::Move)
        } else {
            // MayBegin and Stationary carry no ownership transition.
            None
        }
    }
    if !momentum.is_empty() {
        normalize(momentum).map(WheelPhase::Momentum)
    } else if !direct.is_empty() {
        normalize(direct).map(WheelPhase::Direct)
    } else {
        Some(WheelPhase::Discrete)
    }
}

pub(crate) struct Delivery {
    sample: Sample,
    trace: Option<Trace>,
}

impl Delivery {
    pub(crate) fn deliver(self, window: &mut AppWindow, winit_phase: TouchPhase) {
        let offsets = |window: &AppWindow| window.kernel().scroll_offsets().into_iter().collect::<BTreeMap<_, _>>();
        let before = self.trace.map(|_| offsets(window));
        let Sample { x, y, dx, dy, phase } = self.sample;
        if let Some(phase) = phase {
            window.event(InputEvent::Wheel { x, y, dx, dy, phase });
        }
        if let Some(trace) = self.trace {
            trace.write(
                "winit",
                json!({
                    "phase": format!("{winit_phase:?}"),
                    "routing_phase": phase,
                    "position": [x, y],
                    "delta": [dx, dy],
                    "offsets_before": before,
                    "offsets_after": (!window.is_closed()).then(|| offsets(window)),
                }),
            );
        }
    }
}

#[derive(Clone, Copy)]
struct Trace {
    window: isize,
    started: Instant,
}

impl Trace {
    fn write(self, stage: &str, sample: Value) {
        let _ = writeln!(
            io::stderr().lock(),
            "{}",
            json!({
                "schema": "octara.scroll_trace/1",
                "stage": stage,
                "window": self.window,
                "elapsed_us": self.started.elapsed().as_secs_f64() * 1_000_000.0,
                "sample": sample,
            })
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_phases_distinguish_preflight_release_and_momentum_from_a_wheel_tick() {
        use NSEventPhase as Native;
        assert_eq!(phase(Native::MayBegin, Native::None), None);
        assert_eq!(phase(Native::Stationary, Native::None), None);
        assert_eq!(phase(Native::Began, Native::None), Some(WheelPhase::Direct(Phase::Down)));
        assert_eq!(phase(Native::Ended, Native::None), Some(WheelPhase::Direct(Phase::Up)));
        assert_eq!(phase(Native::None, Native::Began), Some(WheelPhase::Momentum(Phase::Down)));
        assert_eq!(phase(Native::None, Native::Changed), Some(WheelPhase::Momentum(Phase::Move)));
        assert_eq!(phase(Native::None, Native::Ended), Some(WheelPhase::Momentum(Phase::Up)));
        assert_eq!(phase(Native::None, Native::Cancelled), Some(WheelPhase::Momentum(Phase::Cancel)));
        assert_eq!(phase(Native::None, Native::None), Some(WheelPhase::Discrete));
    }
}
