//! Optional CPU and GPU timings. A present call returning is neither GPU completion nor display.
//!
//! `OCTARA_TRACE_INPUT=1` writes `octara.frame_trace/1` JSON lines on stderr. Frame totals
//! start before DisplayList preparation; event-loop work before that is not in the frame total.
//! Opportunity and present instants use the shell's monotonic clock. An observed window tick
//! skipped before drawing uses `octara.display_opportunity/1`. Surface configuration has its own
//! `octara.surface_trace/1` event. The worker's device wait uses `octara.surface_drain/1`.
//! Input times start at shell dispatch (or an owned native text callback), not at the hardware
//! event. Coalesced counts are dispatches, including nested key/native edits, not keystrokes.
//! `octara.inspect_delivery/1` records publication revisions before and after Inspector input.
//! `OCTARA_TRACE_NATIVE_INPUT=1` traces macOS window input and native text publication by kind,
//! recording shell entry, dispatch, and the settled revision. Held pointer motion includes a
//! window-relative position for correlating coalesced samples. Key and text content is omitted.
//! `OCTARA_TRACE_GPU=1` also reports renderer preparation, encoding, submission and work counts.
//! Resolved GPU timestamp samples use `octara.gpu_trace/1` and the same window/attempt/revision keys.
//! These events omit input contents. `OCTARA_TRACE_PRESENTED=1` separately records a Metal
//! drawable's actual display time after the system reports it.
//! `octara.update_trace/1` separately times shell `advance_to` and wake `poll`, including their
//! timers/tasks, reactive work and layout. Input handlers can contain these intervals; don't add
//! them to handler times. The logical app clock labels a sample; elapsed CPU/wait time is real.
//! `octara.touch_trace/1` reports native touch phases and the held contact after each delivery.
//! `OCTARA_TRACE_FRAME_CLOCK=1` adds link transitions and queued-tick state on macOS.

use super::presentation::Opportunity;
use octara::serde_json::{Value, json};
use octara_render::gpu::{FrameProfile, GpuTimings};
use octara_render::wgpu;
use std::time::{Duration, Instant};

fn us(start: Instant, end: Instant) -> f64 {
    duration_us(end.saturating_duration_since(start))
}

fn duration_us(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000_000.0
}

fn since_us(epoch: Instant, at: Instant) -> f64 {
    match at.checked_duration_since(epoch) {
        Some(duration) => duration_us(duration),
        None => -us(at, epoch),
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn frame_clock(epoch: Instant, window: winit::window::WindowId, running: bool, reason: &'static str, tick_queued: bool) {
    eprintln!(
        "{}",
        json!({
            "schema": "octara.frame_clock_trace/1",
            "window": format!("{window:?}"),
            "at_us": since_us(epoch, Instant::now()),
            "running": running,
            "reason": reason,
            "tick_queued": tick_queued,
        })
    );
}

#[cfg(target_os = "macos")]
pub(crate) fn metal_presented(
    drawable: &super::metal_frame::MetalFrame,
    paint: &octara::capture::PaintFrame,
    epoch: Instant,
    size_px: [u32; 2],
) {
    use octara::capture::PaintOrigin;
    use std::io::Write as _;

    let PaintOrigin::Submitted { frame, surface_generation, device_generation } = paint.origin else { return };
    let window = paint.window;
    let revision = paint.sample.revision;
    let scheduled_us = since_us(epoch, Instant::now());
    let media_time = objc2_quartz_core::CACurrentMediaTime();
    drawable.on_presented(move |presented_time_s| {
        let presented_us = scheduled_us + (presented_time_s - media_time) * 1_000_000.0;
        // A diagnostic write can fail without affecting the system's presentation callback.
        let _ = writeln!(
            std::io::stderr().lock(),
            "{}",
            json!({
                "schema": "octara.presented_trace/1",
                "window": window,
                "frame": frame,
                "revision": revision,
                "surface_generation": surface_generation,
                "device_generation": device_generation,
                "size_px": size_px,
                "scheduled_us": scheduled_us,
                "presented_us": presented_us,
                "presented_time_s": presented_time_s,
            })
        );
    });
}

#[cfg(target_os = "macos")]
pub(crate) fn acquire(
    epoch: Instant,
    request: super::presentation::Acquisition,
    route: &'static str,
    started: Instant,
    completion: Option<(Instant, &'static str)>,
) {
    let at = completion.map_or(started, |(finished, _)| finished);
    eprintln!(
        "{}",
        json!({
            "schema": "octara.acquire_trace/1",
            "phase": if completion.is_some() { "return" } else { "start" },
            "route": route,
            "window": request.window().number(),
            "acquisition": request.serial(),
            "at_us": since_us(epoch, at),
            "started_us": since_us(epoch, started),
            "handled_us": completion.map(|_| since_us(epoch, Instant::now())),
            "status": completion.map(|(_, status)| status),
        })
    );
}

pub(crate) struct InspectInput {
    started: Instant,
    before_revision: Option<u64>,
}

impl InspectInput {
    pub(crate) fn start(window: &octara::app::Window) -> Self {
        Self { started: Instant::now(), before_revision: window.published_revision() }
    }

    pub(crate) fn log(self, epoch: Instant, window: &octara::app::Window, route: &'static str, drag_step: Option<u8>) {
        let at = Instant::now();
        eprintln!(
            "{}",
            json!({
                "schema": "octara.inspect_delivery/1",
                "window": window.id().number(),
                "route": route,
                "started_us": since_us(epoch, self.started),
                "at_us": since_us(epoch, at),
                "before_revision": self.before_revision,
                "revision": window.published_revision(),
                "drag_step": drag_step,
            })
        );
    }
}

pub(crate) struct WindowInput {
    entered: Instant,
    publication: InspectInput,
}

impl WindowInput {
    pub(crate) fn start(entered: Option<Instant>, window: &octara::app::Window) -> Option<Self> {
        entered.map(|entered| Self { entered, publication: InspectInput::start(window) })
    }

    pub(crate) fn log(self, epoch: Instant, window: &octara::app::Window, kind: &'static str, position: Option<(f32, f32)>) {
        let at = Instant::now();
        eprintln!(
            "{}",
            json!({
                "schema": "octara.window_input/1",
                "window": window.id().number(),
                "kind": kind,
                "position": position,
                "entered_us": since_us(epoch, self.entered),
                "started_us": since_us(epoch, self.publication.started),
                "at_us": since_us(epoch, at),
                "before_revision": self.publication.before_revision,
                "revision": window.published_revision(),
            })
        );
    }
}

pub(crate) fn capture_output(epoch: Instant, window: octara::app::WindowId, size_px: [u32; 2], started: Instant, saved: bool) {
    eprintln!(
        "{}",
        json!({
            "schema": "octara.capture_trace/1",
            "phase": "painted_output",
            "thread": "worker",
            "window": window.number(),
            "size_px": size_px,
            "started_us": since_us(epoch, started),
            "elapsed_us": us(started, Instant::now()),
            "saved": saved,
        })
    );
}

pub(crate) fn render(profile: FrameProfile, submit: Duration) -> Value {
    json!({
        "prepare_us": duration_us(profile.prepare),
        "encode_us": duration_us(profile.encode),
        "submit_us": duration_us(submit),
        "draw_batches": profile.draw_batches,
        "material_batches": profile.material_batches,
        "render_passes": profile.render_passes,
        "layer_pixels": profile.layer_pixels,
        "canvas_pixels": profile.canvas_pixels,
        "backdrop_copy_pixels": profile.backdrop_copy_pixels,
        "retained_canvases": profile.retained_canvases,
        "retained_backdrops": profile.retained_backdrops,
        "backdrop_allocations": profile.backdrop_allocations,
        "backdrop_reuses": profile.backdrop_reuses,
        "backdrop_cache_peak_bytes": profile.backdrop_cache_peak_bytes,
        "gpu_sampled": profile.gpu_sampled,
    })
}

pub(crate) fn gpu(
    window: &str,
    attempt: u64,
    frame: u32,
    revision: u64,
    surface_generation: u64,
    device_generation: u64,
    timings: GpuTimings,
) -> Value {
    let passes: Vec<_> =
        timings.passes.iter().map(|pass| json!({ "kind": pass.kind.as_str(), "us": duration_us(pass.duration) })).collect();
    json!({
        "schema": "octara.gpu_trace/1",
        "window": window,
        "attempt": attempt,
        "frame": frame,
        "revision": revision,
        "surface_generation": surface_generation,
        "device_generation": device_generation,
        "passes": passes,
        "span_us": timings.span.map(duration_us),
        "truncated": timings.truncated,
    })
}

impl Opportunity {
    pub(crate) fn json(self, epoch: Instant) -> Value {
        match self {
            Self::Immediate => json!({ "source": "immediate" }),
            Self::Redraw => json!({ "source": "system_redraw" }),
            Self::Timer(deadline) => json!({ "source": "fallback_timer", "target_us": since_us(epoch, deadline) }),
            #[cfg(target_os = "macos")]
            Self::DisplayLink { tick, handled_at, expires_at } => json!({
                "source": "display_link",
                "queued_us": tick.queued_at.map(|at| since_us(epoch, at)),
                "callback_us": tick.callback_at.map(|at| since_us(epoch, at)),
                "handled_us": since_us(epoch, handled_at),
                "target_us": tick.target.map(|at| since_us(epoch, at)),
                "expires_us": since_us(epoch, expires_at),
                "coalesced": tick.coalesced,
            }),
        }
    }

    pub(crate) fn skipped(self, window: winit::window::WindowId, epoch: Instant, reason: &'static str) {
        eprintln!(
            "{}",
            json!({
                "schema": "octara.display_opportunity/1",
                "window": format!("{window:?}"),
                "opportunity": self.json(epoch),
                "skip_reason": reason,
            })
        );
    }
}

/// The result actually returned by `get_current_texture`, before recording or submission.
#[derive(Clone, Copy)]
pub(crate) enum AcquireStatus {
    Success,
    Suboptimal,
    Timeout,
    Occluded,
    Outdated,
    Lost,
    Validation,
}

impl AcquireStatus {
    pub(crate) fn from_texture(texture: &octara_render::wgpu::CurrentSurfaceTexture) -> Self {
        use octara_render::wgpu::CurrentSurfaceTexture;
        match texture {
            CurrentSurfaceTexture::Success(_) => Self::Success,
            CurrentSurfaceTexture::Suboptimal(_) => Self::Suboptimal,
            CurrentSurfaceTexture::Timeout => Self::Timeout,
            CurrentSurfaceTexture::Occluded => Self::Occluded,
            CurrentSurfaceTexture::Outdated => Self::Outdated,
            CurrentSurfaceTexture::Lost => Self::Lost,
            CurrentSurfaceTexture::Validation => Self::Validation,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Suboptimal => "suboptimal",
            Self::Timeout => "timeout",
            Self::Occluded => "occluded",
            Self::Outdated => "outdated",
            Self::Lost => "lost",
            Self::Validation => "validation",
        }
    }

    pub(crate) fn acquired(self) -> bool {
        matches!(self, Self::Success | Self::Suboptimal)
    }
}

pub(crate) fn surface_drain(epoch: Instant, device_generation: u64, ready: Option<bool>) {
    eprintln!(
        "{}",
        json!({
            "schema": "octara.surface_drain/1",
            "phase": if ready.is_some() { "finish" } else { "start" },
            "at_us": since_us(epoch, Instant::now()),
            "device_generation": device_generation,
            "ready": ready,
        })
    );
}

pub(crate) fn device_admission(
    epoch: Instant,
    at: Instant,
    device_generation: u64,
    selected: &'static str,
    capture: (octara::app::WindowId, Instant),
    frame: Option<(octara::app::WindowId, Opportunity)>,
) {
    eprintln!(
        "{}",
        json!({
            "schema": "octara.device_admission/1",
            "at_us": since_us(epoch, at),
            "device_generation": device_generation,
            "selected": selected,
            "window": frame.map(|(window, _)| window.number()),
            "capture": capture.0.number(),
            "capture_wait_us": us(capture.1, at),
            "frame_deadline_us": frame.and_then(|(_, opportunity)| opportunity.deadline()).map(|deadline| since_us(epoch, deadline)),
        })
    );
}

pub(crate) struct SurfaceConfiguration {
    pub(crate) window: winit::window::WindowId,
    pub(crate) attempt: Option<u64>,
    pub(crate) surface_generation: u64,
    pub(crate) device_generation: u64,
    pub(crate) size: [u32; 2],
    pub(crate) desired_maximum_frame_latency: u32,
    pub(crate) reason: &'static str,
    pub(crate) configured: bool,
}

impl SurfaceConfiguration {
    pub(crate) fn log(self, epoch: Instant, started: Instant, _surface: &wgpu::Surface<'_>) {
        let elapsed = started.elapsed();
        let metal_drawables = None::<usize>;
        #[cfg(target_os = "macos")]
        let metal_drawables = if self.configured && objc2::available!(macos = 10.13.2) {
            // SAFETY: Keep the HAL guard while reading the layer's configured pool size.
            unsafe { _surface.as_hal::<wgpu::hal::api::Metal>() }.map(|hal| hal.render_layer().lock().maximumDrawableCount())
        } else {
            metal_drawables
        };
        eprintln!(
            "{}",
            json!({
                "schema": "octara.surface_trace/1",
                "window": format!("{:?}", self.window),
                "attempt": self.attempt,
                "surface_generation": self.surface_generation,
                "device_generation": self.device_generation,
                "size_px": self.size,
                "desired_maximum_frame_latency": self.desired_maximum_frame_latency,
                "metal_drawables": metal_drawables,
                "reason": self.reason,
                "configured": self.configured,
                "started_us": since_us(epoch, started),
                "elapsed_us": duration_us(elapsed),
            })
        );
    }
}

/// Only the oldest unpresented dispatch is retained; counts include native edit callbacks.
pub(crate) struct Input {
    at: Instant,
    started: Instant,
    handled: Instant,
    kind: &'static str,
    queued: bool,
    dispatches: u32,
    native_compositions: u32,
    native_commits: u32,
}

impl Input {
    pub(crate) fn handled(at: Instant, kind: &'static str) -> Self {
        Self { at, started: at, handled: Instant::now(), kind, queued: false, dispatches: 1, native_compositions: 0, native_commits: 0 }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn native(input: NativeInput, started: Instant) -> Self {
        Self {
            at: input.at,
            started,
            handled: Instant::now(),
            kind: input.kind,
            queued: true,
            dispatches: input.dispatches,
            native_compositions: input.compositions,
            native_commits: input.commits,
        }
    }

    pub(crate) fn merge(&mut self, mut newer: Self) {
        // A key's synchronous native callbacks can finish before its outer key dispatch does.
        if newer.at < self.at {
            std::mem::swap(self, &mut newer);
        }
        self.dispatches = self.dispatches.saturating_add(newer.dispatches);
        self.native_compositions = self.native_compositions.saturating_add(newer.native_compositions);
        self.native_commits = self.native_commits.saturating_add(newer.native_commits);
    }

    pub(crate) fn json(&self, frame_start: Instant, present_returned: Option<Instant>) -> Value {
        json!({
            "kind": self.kind,
            "dispatches": self.dispatches,
            "coalesced": self.dispatches.saturating_sub(1),
            "handler_us": us(self.started, self.handled),
            // Only the owned native callback has a queue origin; winit starts at dispatch.
            "queue_us": self.queued.then(|| us(self.at, self.started)),
            // Includes later events and other main-thread work, not just display pacing.
            "wait_redraw_us": us(self.handled, frame_start),
            "to_present_return_us": present_returned.map(|end| us(self.at, end)),
            "native_compositions": self.native_compositions,
            "native_commits": self.native_commits,
        })
    }
}

/// An owned NSTextInputClient batch, recorded without retaining any typed text.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
pub(crate) struct NativeInput {
    at: Instant,
    kind: &'static str,
    dispatches: u32,
    compositions: u32,
    commits: u32,
}

#[cfg(target_os = "macos")]
impl NativeInput {
    pub(crate) fn new(kind: &'static str) -> Self {
        Self { at: Instant::now(), kind, dispatches: 0, compositions: 0, commits: 0 }
    }

    pub(crate) fn edit(&mut self, kind: &'static str) {
        self.dispatches = self.dispatches.saturating_add(1);
        self.compositions = self.compositions.saturating_add(u32::from(kind == "native composition"));
        self.commits = self.commits.saturating_add(u32::from(kind == "native commit"));
    }

    pub(crate) fn publication(self, window: &octara::app::Window) -> Option<(WindowInput, &'static str)> {
        let kind = if self.commits != 0 {
            "text_commit"
        } else if self.compositions != 0 {
            "composition"
        } else {
            return None;
        };
        Some((WindowInput { entered: self.at, publication: InspectInput::start(window) }, kind))
    }
}

pub(crate) struct Frame {
    pub(crate) started: Instant,
    pub(crate) app_clock_ms: u64,
    pub(crate) opportunity: Opportunity,
    pub(crate) display_list: Instant,
    pub(crate) surface_start: Option<Instant>,
    pub(crate) acquisition: Option<(Instant, AcquireStatus)>,
    pub(crate) acquisition_request: Option<u64>,
    pub(crate) encoded: Option<Instant>,
    pub(crate) native_placed: Option<Instant>,
    pub(crate) present_start: Option<Instant>,
    pub(crate) present_returned: Option<Instant>,
    pub(crate) surface_generation: u64,
    pub(crate) render: Option<(FrameProfile, Duration)>,
    pub(crate) render_worker: bool,
    pub(crate) presents_with_transaction: bool,
    pub(crate) skip_reason: Option<&'static str>,
}

impl Frame {
    pub(crate) fn new(started: Instant, app_clock_ms: u64, opportunity: Opportunity) -> Self {
        Self {
            started,
            app_clock_ms,
            opportunity,
            display_list: Instant::now(),
            surface_start: None,
            acquisition: None,
            acquisition_request: None,
            encoded: None,
            native_placed: None,
            present_start: None,
            present_returned: None,
            surface_generation: 0,
            render: None,
            render_worker: false,
            presents_with_transaction: false,
            skip_reason: None,
        }
    }

    pub(crate) fn timeline_json(&self, epoch: Instant) -> Value {
        json!({
            "started": since_us(epoch, self.started),
            "render_submitted": self.encoded.map(|at| since_us(epoch, at)),
            "present_called": self.present_start.map(|at| since_us(epoch, at)),
            "present_returned": self.present_returned.map(|at| since_us(epoch, at)),
        })
    }

    pub(crate) fn cpu_json(&self, finished: Instant) -> Value {
        let surface_ready = self.acquisition.map(|(at, _)| at);
        let phase = |from: Option<Instant>, to: Option<Instant>| from.zip(to).map(|(a, b)| us(a, b));
        let last =
            self.present_returned.or(self.present_start).or(self.native_placed).or(self.encoded).or(surface_ready).or(self.surface_start);
        json!({
            "display_list": us(self.started, self.display_list),
            "frame_setup": self.surface_start.map(|at| us(self.display_list, at)),
            "surface_acquire": phase(self.surface_start, surface_ready),
            // Includes renderer preparation/uploads, command encoding and queue.submit.
            "renderer_encode_submit": phase(surface_ready, self.encoded),
            "native_place": phase(self.encoded, self.native_placed),
            "pre_present_notify": phase(self.native_placed, self.present_start),
            "present_call": phase(self.present_start, self.present_returned),
            "frame_finish": us(last.unwrap_or(self.display_list), finished),
            "total": us(self.started, finished),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_nested_dispatch_keeps_the_oldest_handler_and_counts_both() {
        let at = Instant::now();
        let mut native = Input {
            at: at + Duration::from_micros(20),
            started: at + Duration::from_micros(30),
            handled: at + Duration::from_micros(40),
            kind: "native composition",
            queued: true,
            dispatches: 1,
            native_compositions: 1,
            native_commits: 0,
        };
        native.merge(Input {
            at,
            started: at,
            handled: at + Duration::from_micros(50),
            kind: "key",
            queued: false,
            dispatches: 1,
            native_compositions: 0,
            native_commits: 0,
        });
        let value = native.json(at + Duration::from_micros(100), Some(at + Duration::from_micros(150)));
        assert_eq!(value["kind"], "key");
        assert_eq!(value["dispatches"], 2);
        assert_eq!(value["coalesced"], 1);
        assert_eq!(value["native_compositions"], 1);
        assert_eq!(value["handler_us"], 50.0);
        assert_eq!(value["wait_redraw_us"], 50.0);
        assert_eq!(value["to_present_return_us"], 150.0);
        assert!(value["queue_us"].is_null());
    }

    #[test]
    fn frame_phases_partition_the_cpu_interval_and_absent_present_stays_null() {
        let at = Instant::now();
        let t = |n| at + Duration::from_micros(n);
        let mut frame = Frame {
            started: at,
            app_clock_ms: 0,
            opportunity: Opportunity::Timer(t(105)),
            display_list: t(10),
            surface_start: Some(t(20)),
            acquisition: Some((t(70), AcquireStatus::Success)),
            acquisition_request: None,
            encoded: Some(t(90)),
            native_placed: Some(t(95)),
            present_start: Some(t(96)),
            present_returned: Some(t(100)),
            surface_generation: 0,
            render: None,
            render_worker: false,
            presents_with_transaction: false,
            skip_reason: None,
        };
        let value = frame.cpu_json(t(110));
        assert_eq!(value["surface_acquire"], 50.0);
        assert_eq!(value["renderer_encode_submit"], 20.0);
        assert_eq!(value["present_call"], 4.0);
        let sum: f64 = value.as_object().unwrap().iter().filter(|(k, _)| *k != "total").map(|(_, v)| v.as_f64().unwrap()).sum();
        assert!((sum - value["total"].as_f64().unwrap()).abs() < 0.001);
        assert_eq!(frame.opportunity.json(at)["target_us"], 105.0);
        assert_eq!(frame.timeline_json(at)["present_returned"], 100.0);
        assert_eq!(frame.timeline_json(at)["render_submitted"], 90.0);
        frame.present_start = None;
        frame.present_returned = None;
        frame.acquisition = Some((t(70), AcquireStatus::Timeout));
        frame.encoded = None;
        frame.native_placed = None;
        assert!(frame.cpu_json(t(110))["present_call"].is_null());
        assert!(frame.cpu_json(t(110))["renderer_encode_submit"].is_null());
        assert!(frame.timeline_json(at)["present_returned"].is_null());
    }

    #[test]
    fn gpu_pass_samples_carry_the_original_submission_without_inventing_a_span() {
        use octara_render::gpu::{GpuPassKind, GpuPassTiming};
        let sample = gpu(
            "window 2",
            21,
            19,
            11,
            3,
            1,
            GpuTimings {
                passes: vec![GpuPassTiming { kind: GpuPassKind::CanvasClear, duration: Duration::from_micros(20) }],
                span: None,
                truncated: false,
            },
        );
        assert_eq!(sample["window"], "window 2");
        assert_eq!(sample["attempt"], 21);
        assert_eq!(sample["frame"], 19);
        assert_eq!(sample["revision"], 11);
        assert_eq!(sample["surface_generation"], 3);
        assert_eq!(sample["device_generation"], 1);
        assert_eq!(sample["passes"][0], json!({ "kind": "canvas_clear", "us": 20.0 }));
        assert!(sample["span_us"].is_null());
    }
}
