//! Captures from a window ([design](../../../docs/design/capture.md)): settle on the wall clock,
//! take the native views' snapshots, paint the frame with the window's GPU renderer offscreen.
//!
//! `OCTARA_CAPTURE=shot.png` captures once and exits, printing the report (JSON) on stdout:
//! after `OCTARA_CAPTURE_AFTER_MS` (default 300), scrolled down `OCTARA_CAPTURE_SCROLL` pixels
//! at the window's center, settling for at most `OCTARA_CAPTURE_SETTLE_MS` (default 5000).
//! `OCTARA_CAPTURE_PIXELS=display` asks for the window server's picture (macOS), painted with a
//! stated `fallback` where there is none.

use super::{Shell, ShellEvent};
use crate::capture_output::{CaptureOutput, CaptureWriter};
use octara::app::{App, Window, WindowId};
use octara::capture::{CaptureId, DisplayCapture, PaintFrame, PaintOrigin, Pixels, PreparedCapture};
use octara_primitives::capture::{PaintedPicture, Picture};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// How long a native view's snapshot may take.
const SNAPSHOT_LIMIT: Duration = Duration::from_secs(5);
/// Give the submitted frame time to reach the window server (two 60 Hz refreshes and a margin).
const SHOWN: Duration = Duration::from_millis(40);
const MAX_PIXELS: f64 = 32.0 * 1024.0 * 1024.0;
const DISPLAY_CHANGED: &str = "the window changed while sampling the submitted frame";
const DISPLAY_SUPERSEDED: &str = "the window changed before the requested drawing was submitted";
const DISPLAY_NOT_SUBMITTED: &str = "the window's current frame was not submitted within the capture budget";

/// Bound a live capture's actual device-pixel allocation, including rounding each side up.
pub(crate) fn validate_size(size: (f32, f32), scale: f32) -> Result<(), String> {
    let width = (size.0 * scale).ceil().max(1.0);
    let height = (size.1 * scale).ceil().max(1.0);
    if !scale.is_finite()
        || scale <= 0.0
        || scale > 8.0
        || !size.0.is_finite()
        || !size.1.is_finite()
        || size.0 <= 0.0
        || size.1 <= 0.0
        || !width.is_finite()
        || !height.is_finite()
        || f64::from(width) * f64::from(height) > MAX_PIXELS
    {
        return Err("capture size and scale must be finite and positive, scale at most 8, and produce at most 32 megapixels".into());
    }
    Ok(())
}

/// A capture in progress.
pub(crate) struct Job {
    window: WindowId,
    path: String,
    settle: Duration,
    scroll: Option<f32>,
    pixels: Pixels,
    stage: Stage,
    scale: Option<f32>,
    reply: Option<super::inspect::Responder>,
    readback_active: Arc<AtomicBool>,
    retried_gpu_paint: bool,
}

enum Stage {
    Start(Instant),
    Settling {
        until: Instant,
        display_failure: Option<String>,
    },
    AwaitingSubmission {
        until: Instant,
        display: DisplayFrame,
    },
    /// Waiting to sample the window server after submitting an Octara frame.
    Display {
        at: Instant,
        until: Instant,
        display: DisplayFrame,
        paint: Box<PaintFrame>,
    },
    PreparingSnapshots {
        until: Instant,
        fallback: Option<String>,
        placement_requested: bool,
    },
    Snapshots {
        window: Window,
        id: CaptureId,
        until: Instant,
        scale: f32,
        pending: Vec<String>,
        fallback: Option<String>,
        ready_at: Option<Instant>,
    },
    Readback {
        prepared: PreparedCapture,
        pixels: Receiver<Result<PaintedPicture, String>>,
        generation: u64,
        renderer: &'static str,
        scale: f32,
        pending: Vec<String>,
        fallback: Option<String>,
    },
    Output {
        output: CaptureOutput,
        generation: u64,
        fallback: Option<String>,
    },
}

#[derive(Clone, Copy, PartialEq)]
struct DisplayGeometry {
    size: (u32, u32),
    scale: f32,
    surface_generation: u64,
}

impl DisplayGeometry {
    fn current(window: &Window) -> Option<Self> {
        super::ww(window, |host| {
            let size = host.configured_size()?;
            let scale = window.scale();
            host.matches_drawable(scale).then_some(Self {
                size: (size.width, size.height),
                scale,
                surface_generation: host.surface_generation,
            })
        })
        .flatten()
    }
}

struct DisplayFrame {
    frame: DisplayCapture,
    geometry: DisplayGeometry,
    pending: Vec<String>,
}

impl DisplayFrame {
    fn begin(window: &Window, pending: Vec<String>) -> Result<Self, String> {
        let frame = window.capture_display_begin()?;
        let geometry = DisplayGeometry::current(window).ok_or("the window and its drawing surface disagree on size or scale")?;
        Ok(Self { frame, geometry, pending })
    }

    fn is_current(&self, window: &Window) -> bool {
        self.frame.is_current(window) && DisplayGeometry::current(window) == Some(self.geometry)
    }

    fn submitted_paint(&self, window: &Window) -> Option<PaintFrame> {
        super::ww(window, |host| host.submitted_paint().filter(|paint| paint.sample.revision == self.frame.revision()).cloned()).flatten()
    }

    fn has_submission(&self, window: &Window, paint: &PaintFrame) -> bool {
        self.is_current(window)
            && super::ww(window, |host| {
                host.submitted_paint()
                    .is_some_and(|submitted| submitted.sample.revision == self.frame.revision() && submitted.origin == paint.origin)
            })
            .unwrap_or(false)
    }
}

fn env<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

impl Job {
    /// The capture `OCTARA_CAPTURE` asks for, if any.
    pub(crate) fn from_env(window: WindowId) -> Option<Job> {
        let path = std::env::var("OCTARA_CAPTURE").ok()?;
        let after = Duration::from_millis(env("OCTARA_CAPTURE_AFTER_MS").unwrap_or(300));
        Some(Job {
            window,
            path,
            settle: Duration::from_millis(env("OCTARA_CAPTURE_SETTLE_MS").unwrap_or(5000)),
            scroll: env("OCTARA_CAPTURE_SCROLL"),
            pixels: if std::env::var("OCTARA_CAPTURE_PIXELS").as_deref() == Ok("display") { Pixels::Display } else { Pixels::Painted },
            stage: Stage::Start(Instant::now() + after),
            scale: None,
            reply: None,
            readback_active: Arc::new(AtomicBool::new(true)),
            retried_gpu_paint: false,
        })
    }

    pub(crate) fn live(
        window: WindowId,
        path: String,
        settle: Duration,
        scale: Option<f32>,
        pixels: Pixels,
        reply: super::inspect::Responder,
    ) -> Self {
        Self {
            window,
            path,
            settle,
            scroll: None,
            pixels,
            stage: Stage::Start(Instant::now()),
            scale,
            reply: Some(reply),
            readback_active: Arc::new(AtomicBool::new(true)),
            retried_gpu_paint: false,
        }
    }

    fn prepare_snapshots(&mut self, until: Instant, fallback: Option<String>) {
        self.stage = Stage::PreparingSnapshots { until, fallback, placement_requested: false };
    }

    fn retry_display(&mut self, until: Instant, reason: String) {
        if Instant::now() < until {
            self.stage = Stage::Settling { until, display_failure: Some(reason) };
        } else {
            self.prepare_snapshots(until, Some(reason));
        }
    }

    pub(crate) fn settling(&self) -> bool {
        matches!(self.stage, Stage::Settling { .. })
    }

    pub(crate) fn waits_for_draw(&self, window: WindowId) -> bool {
        self.window == window && matches!(self.stage, Stage::PreparingSnapshots { .. } | Stage::AwaitingSubmission { .. })
    }

    pub(crate) fn deadline(&self, app: &App) -> Option<Instant> {
        let _ = app;
        match self.stage {
            Stage::Start(at) | Stage::Display { at, .. } => Some(at),
            Stage::Settling { until, .. } | Stage::AwaitingSubmission { until, .. } => Some(until),
            Stage::PreparingSnapshots { until, .. } => {
                #[cfg(target_os = "macos")]
                if app.window(self.window).is_some_and(|window| super::ww(&window, |host| host.waiting_for_drawable()) == Some(true)) {
                    // The drawable worker already has a completion wake, including after the budget ends.
                    return None;
                }
                Some(until)
            }
            Stage::Snapshots { until, ready_at: None, .. } => Some(until),
            Stage::Snapshots { .. } | Stage::Readback { .. } | Stage::Output { .. } => None,
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.readback_active.store(false, Ordering::Relaxed);
        if let Stage::Snapshots { window, id, .. } = &self.stage {
            window.capture_cancel(*id);
        }
    }
}

impl Job {
    pub(crate) fn ready_to_render(&mut self) -> Option<(WindowId, Instant)> {
        let Stage::Snapshots { window, id, ready_at, .. } = &mut self.stage else { return None };
        if ready_at.is_none() && window.capture_awaiting(*id).is_empty() {
            *ready_at = Some(Instant::now());
        }
        ready_at.map(|at| (self.window, at))
    }

    fn output(
        &mut self,
        writer: &mut Option<CaptureWriter>,
        generation: u64,
        fallback: Option<String>,
        proxy: winit::event_loop::EventLoopProxy<ShellEvent>,
        write: impl FnOnce(&str) -> Result<octara::capture::Capture, String> + Send + 'static,
    ) -> Result<(), String> {
        if writer.is_none() {
            *writer = Some(CaptureWriter::new().map_err(|error| format!("{}: {error}", self.path))?);
        }
        let output = writer.as_mut().expect("capture writer started").submit(self.path.clone(), write, move || {
            let _ = proxy.send_event(ShellEvent::Capture);
        });
        self.stage = Stage::Output { output, generation, fallback };
        Ok(())
    }
}

impl Shell {
    fn capture_window(&mut self, job: &mut Job) -> Option<Window> {
        if job.reply.as_ref().is_some_and(|reply| !reply.connected.load(Ordering::Relaxed)) {
            return None;
        }
        let Some(window) = self.app.as_ref().and_then(|app| app.window(job.window)).filter(|window| !window.is_closed()) else {
            let message = "the target window closed while capturing";
            if let Some(reply) = job.reply.take() {
                let (code, message) = if reply.target_current() {
                    (&octara::diagnostics::CAPTURE_FAILED, message)
                } else {
                    (&octara::diagnostics::STALE_TARGET, "the observed window closed while capturing; inspect again")
                };
                reply.send(octara::inspect::Reply::fail(code, message));
            } else {
                eprintln!("octara-shell: capture {}: {message}", job.path);
                self.done = true;
            }
            return None;
        };
        if let Some(reply) = job.reply.take_if(|reply| !reply.target_current()) {
            reply.send(octara::inspect::Reply::fail(
                &octara::diagnostics::STALE_TARGET,
                "the observed window was rebuilt while capturing; inspect again",
            ));
            return None;
        }
        Some(window)
    }

    /// Advances settling, native snapshots or the output of an admitted capture.
    pub(crate) fn capture_step(&mut self) {
        let Some(mut job) = self.capture.take() else { return };
        let now = Instant::now();
        if matches!(job.stage, Stage::Start(at) | Stage::Settling { until: at, .. } if now >= at) {
            self.tick_at(now);
        }
        let Some(mut w) = self.capture_window(&mut job) else { return };
        if !self.shared.gpu_available() {
            self.capture = Some(job);
            return;
        }
        let output_generation = match &job.stage {
            Stage::Readback { generation, fallback, .. } | Stage::Output { generation, fallback, .. } => Some((*generation, fallback)),
            _ => None,
        };
        if let Some((generation, fallback)) = output_generation
            && self.shared.gpu.borrow().as_ref().map(|gpu| gpu.generation.number) != Some(generation)
        {
            if !job.retried_gpu_paint {
                job.retried_gpu_paint = true;
                job.stage = Stage::Settling { until: now, display_failure: fallback.clone() };
            } else {
                self.report(&w, Err("the GPU device changed again during capture".into()), job);
                return;
            }
        }
        if let Stage::Start(at) = job.stage {
            if now < at {
                self.capture = Some(job);
                return;
            }
            if let Some(dy) = job.scroll {
                let (width, height) = w.size();
                w.wheel(width / 2.0, height / 2.0, 0.0, dy);
            }
            job.stage = Stage::Settling { until: now + job.settle, display_failure: None };
        }
        if let Stage::Display { at, until, ref display, ref paint } = job.stage {
            if now < at {
                self.capture = Some(job);
                return;
            }
            let geometry = display.geometry;
            let picture = if display.has_submission(&w, paint) {
                let picture = window_picture(&w);
                display.has_submission(&w, paint).then_some(picture)
            } else {
                None
            };
            match picture {
                None => job.retry_display(until, DISPLAY_CHANGED.into()),
                Some(Err(why)) => job.prepare_snapshots(until, Some(why)),
                Some(Ok(picture)) if job.reply.is_some() && f64::from(picture.width) * f64::from(picture.height) > MAX_PIXELS => {
                    self.report(&w, Err("the display picture exceeds 32 megapixels".into()), job);
                    return;
                }
                Some(Ok(picture)) if (picture.width, picture.height) != geometry.size => {
                    let (width, height) = geometry.size;
                    let why = format!(
                        "the display picture dimensions {}×{} differ from the submitted surface dimensions {width}×{height}",
                        picture.width, picture.height
                    );
                    job.prepare_snapshots(until, Some(why));
                }
                Some(Ok(picture)) => {
                    let Stage::Display { display, paint, .. } = std::mem::replace(&mut job.stage, Stage::Start(now)) else {
                        unreachable!()
                    };
                    let PaintOrigin::Submitted { device_generation, .. } = paint.origin else {
                        unreachable!("display captures wait for a submitted frame");
                    };
                    let result = w.capture_display_report(display.frame, &picture, &job.path, geometry.scale, display.pending);
                    let result = result.and_then(|mut report| {
                        report.paint = Some(*paint);
                        job.output(&mut self.capture_writer, device_generation, None, self.shared.proxy.clone(), move |temporary| {
                            picture.save_png(temporary).map_err(|error| error.to_string())?;
                            Ok(report)
                        })
                    });
                    match result {
                        Ok(()) => self.capture = Some(job),
                        Err(error) => self.report(&w, Err(error), job),
                    }
                    return;
                }
            }
        }
        if let Stage::AwaitingSubmission { until, ref display } = job.stage
            && !display.is_current(&w)
        {
            job.retry_display(until, DISPLAY_SUPERSEDED.into());
        }
        if let Stage::Settling { until, ref display_failure } = job.stage {
            let pending = w.unsettled();
            if (!w.frame_is_settled() || !pending.is_empty()) && now < until {
                self.capture = Some(job);
                return;
            }
            if job.reply.is_some()
                && let Err(why) = validate_size(w.size(), job.scale.unwrap_or(w.scale()))
            {
                self.report(&w, Err(why), job);
                return;
            }
            if job.pixels == Pixels::Display && (display_failure.is_none() || Instant::now() < until) {
                match DisplayFrame::begin(&w, pending) {
                    Ok(display) => {
                        let needs_submission = super::ww(&w, |host| {
                            if host.has_submitted(display.frame.revision()) {
                                return false;
                            }
                            host.request_draw();
                            true
                        })
                        .unwrap_or(false);
                        job.stage = Stage::AwaitingSubmission { until, display };
                        if needs_submission {
                            self.capture = Some(job);
                            return;
                        }
                    }
                    Err(why) => {
                        job.retry_display(until, why);
                        if matches!(job.stage, Stage::Settling { .. }) {
                            // A surface requiring configuration may have no other draw demand.
                            super::ww(&w, |host| host.request_draw());
                        }
                    }
                }
            } else {
                job.prepare_snapshots(until, display_failure.clone());
            }
        }
        if let Stage::AwaitingSubmission { until, ref display } = job.stage {
            if let Some(paint) = display.submitted_paint(&w) {
                let Stage::AwaitingSubmission { display, .. } = std::mem::replace(&mut job.stage, Stage::Start(now)) else {
                    unreachable!()
                };
                job.stage = Stage::Display { at: Instant::now() + SHOWN, until, display, paint: Box::new(paint) };
                self.capture = Some(job);
                return;
            } else if now < until {
                self.capture = Some(job);
                return;
            } else {
                job.prepare_snapshots(until, Some(DISPLAY_NOT_SUBMITTED.into()));
            }
        }
        if matches!(job.stage, Stage::Settling { .. }) {
            self.capture = Some(job);
            return;
        }
        if let Stage::PreparingSnapshots { until, ref fallback, ref mut placement_requested } = job.stage {
            // Allow a first presentation opportunity even if the settling budget has expired.
            if prepare_native_placements(&w, !*placement_requested || now < until) {
                *placement_requested = true;
                self.capture = Some(job);
                return;
            }
            let scale = job.scale.unwrap_or(w.scale());
            let pending = w.unsettled();
            let until = Instant::now() + SNAPSHOT_LIMIT;
            let id = match w.capture_begin(until) {
                Ok(id) => id,
                Err(why) => {
                    self.report(&w, Err(why), job);
                    return;
                }
            };
            job.stage = Stage::Snapshots { window: w.clone(), id, until, scale, pending, fallback: fallback.clone(), ready_at: None };
        }
        if matches!(job.stage, Stage::Snapshots { .. }) {
            self.capture = Some(job);
            return;
        }
        if let Stage::Readback { generation, ref pixels, .. } = job.stage {
            let recording = match pixels.try_recv() {
                Ok(recording) => recording,
                Err(TryRecvError::Empty) => {
                    self.capture = Some(job);
                    return;
                }
                Err(TryRecvError::Disconnected) => Err("the GPU readback worker disconnected".into()),
            };
            let Stage::Readback { prepared, renderer, scale, pending, fallback, .. } = std::mem::replace(&mut job.stage, Stage::Start(now))
            else {
                unreachable!()
            };
            let tracing = self.shared.trace;
            let epoch = self.shared.trace_epoch;
            let window = w.id();
            let result = recording.and_then(|recording| {
                job.output(&mut self.capture_writer, generation, fallback.clone(), self.shared.proxy.clone(), move |temporary| {
                    let started = tracing.then(Instant::now);
                    let size = [recording.picture.width, recording.picture.height];
                    let result = prepared.finish(recording, renderer, temporary, scale, pending, fallback);
                    if let Some(started) = started {
                        super::trace::capture_output(epoch, window, size, started, result.is_ok());
                    }
                    result.map(|mut capture| {
                        if let Some(paint) = &mut capture.paint {
                            paint.origin = PaintOrigin::Rerecorded { device_generation: Some(generation) };
                        }
                        capture
                    })
                })
            });
            match result {
                Ok(()) => self.capture = Some(job),
                Err(error) => self.report(&w, Err(error), job),
            }
            return;
        }
        if let Stage::Output { ref mut output, .. } = job.stage {
            let Some(staged) = output.try_take() else {
                self.capture = Some(job);
                return;
            };
            let result = staged.and_then(|capture| capture.publish());
            if result.as_ref().is_ok_and(|capture| capture.pixels == Pixels::Painted)
                && let Some(gpu) = self.shared.gpu.borrow_mut().as_mut()
            {
                gpu.submitted();
            }
            self.report(&w, result, job);
        }
    }

    /// Admits a queued painted capture through the shared renderer scheduler.
    pub(crate) fn submit_ready_capture(&mut self) -> bool {
        if !self.shared.gpu.borrow().as_ref().is_some_and(|gpu| gpu.idle() && gpu.available()) {
            return false;
        }
        let Some(mut job) = self.capture.take_if(|job| job.ready_to_render().is_some()) else { return false };
        let Stage::Snapshots { id, scale, .. } = job.stage else { unreachable!("only queued snapshots can start painting") };
        let Some(w) = self.capture_window(&mut job) else { return false };
        // The window keeps the capture session until admission, so resize can invalidate snapshots.
        let prepared = match w.capture_prepare(id) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.report(&w, Err(error), job);
                return false;
            }
        };
        let list = prepared.display_list();
        if job.reply.is_some()
            && let Err(error) = validate_size(list.size, scale)
        {
            self.report(&w, Err(error), job);
            return false;
        }
        let (generation, renderer, pixels) = {
            let mut gpu = self.shared.gpu.borrow_mut();
            let gpu = gpu.as_mut().expect("the renderer is available");
            (
                gpu.generation.number,
                gpu.renderer_name(),
                gpu.start_capture(list, scale, prepared.paint_sample(), job.readback_active.clone(), self.shared.proxy.clone()),
            )
        };
        let pixels = match pixels {
            Ok(pixels) => pixels,
            Err(error) => {
                self.report(&w, Err(error), job);
                return false;
            }
        };
        let Stage::Snapshots { pending, fallback, .. } = std::mem::replace(&mut job.stage, Stage::Start(Instant::now())) else {
            unreachable!()
        };
        job.stage = Stage::Readback { prepared, pixels, generation, renderer, scale, pending, fallback };
        self.capture = Some(job);
        true
    }

    /// Prints the report (stdout) and each native view's AppKit frame (stderr), and exits.
    fn report(&mut self, w: &Window, result: Result<octara::capture::Capture, String>, mut job: Job) {
        if let Some(reply) = job.reply.take() {
            #[cfg(debug_assertions)]
            let destroy = self.destroy_device_after_capture && result.as_ref().is_ok_and(|report| report.pixels == Pixels::Painted);
            let response = match result {
                Ok(report) => octara::inspect::Reply { capture: Some(Box::new(report)), ..octara::inspect::Reply::ok() },
                Err(why) => octara::inspect::Reply::fail(&octara::diagnostics::CAPTURE_FAILED, why),
            };
            reply.send(response);
            #[cfg(debug_assertions)]
            if destroy && let Some(gpu) = self.shared.gpu.borrow_mut().as_mut() {
                self.destroy_device_after_capture = false;
                gpu.destroy_for_test();
            }
            return;
        }
        match result {
            Ok(report) => println!("{}", report.to_json()),
            Err(e) => eprintln!("octara-shell: capture {}: {e}", job.path),
        }
        #[cfg(target_os = "macos")]
        for line in super::ww(w, |x| x.native.as_ref().map(|n| n.frames())).flatten().unwrap_or_default() {
            eprintln!("octara-shell: {line}");
        }
        let _ = w;
        self.done = true;
    }
}

/// Applies placements for an occluded window, or asks to wait for the next presentation.
fn prepare_native_placements(window: &Window, may_wait: bool) -> bool {
    #[cfg(target_os = "macos")]
    {
        super::ww(window, |host| {
            if host.native.as_ref().is_none_or(|native| !native.has_pending_placements()) {
                return false;
            }
            if host.waiting_for_drawable() {
                return true;
            }
            if !host.can_present() {
                host.place_occluded_native_views();
                return false;
            }
            if may_wait {
                host.request_draw();
            }
            may_wait
        })
        .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, may_wait);
        false
    }
}

/// The window server's picture of the window, or why there is none.
fn window_picture(w: &Window) -> Result<Picture, String> {
    #[cfg(target_os = "macos")]
    if let Some(p) = super::ww(w, |x| x.native.as_ref().map(|n| n.window_picture())).flatten() {
        return p;
    }
    let _ = w;
    Err("the window server's picture is taken on macOS only so far".into())
}

#[cfg(test)]
mod tests {
    use super::validate_size;

    #[test]
    fn live_capture_bounds_the_rounded_pixel_allocation() {
        assert!(validate_size((8192.0, 4096.0), 1.0).is_ok());
        assert!(validate_size((8192.01, 4095.99), 1.0).is_err());
        assert!(validate_size((4096.0, 2048.0), 2.0).is_ok());
        assert!(validate_size((1.0, 1.0), f32::NAN).is_err());
        assert!(validate_size((f32::INFINITY, 1.0), 1.0).is_err());
        assert!(validate_size((1.0, 1.0), 8.01).is_err());
    }
}
