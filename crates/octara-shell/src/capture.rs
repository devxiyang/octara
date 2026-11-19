//! Captures from a window ([design](../../../docs/design/capture.md)): settle on the wall clock,
//! take the native views' snapshots, paint the frame with the window's GPU renderer offscreen.
//!
//! `OCTARA_CAPTURE=shot.png` captures once and exits, printing the report (JSON) on stdout:
//! after `OCTARA_CAPTURE_AFTER_MS` (default 300), scrolled down `OCTARA_CAPTURE_SCROLL` pixels
//! at the window's center, settling for at most `OCTARA_CAPTURE_SETTLE_MS` (default 5000).
//! `OCTARA_CAPTURE_PIXELS=display` asks for the window server's picture (macOS), painted with a
//! stated `fallback` where there is none.

use super::{Shell, ShellEvent};
use octara::app::{Window, WindowId};
use octara::capture::{CaptureId, DisplayCapture, Pixels};
use octara_primitives::capture::{Painter, Picture};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long a native view's snapshot may take.
const SNAPSHOT_LIMIT: Duration = Duration::from_secs(5);
/// How often a capture in progress looks again.
const POLL: Duration = Duration::from_millis(50);
/// Give the submitted frame time to reach the window server (two 60 Hz refreshes and a margin).
const SHOWN: Duration = Duration::from_millis(40);
const MAX_PIXELS: f64 = 32.0 * 1024.0 * 1024.0;
const DISPLAY_CHANGED: &str = "the window changed while sampling the submitted frame";

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

/// Validate at paint time: the window can resize while native snapshots are pending.
struct CapturePainter<'a> {
    renderer: Option<&'a mut dyn Painter>,
    bounded: bool,
}

impl Painter for CapturePainter<'_> {
    fn renderer(&self) -> &'static str {
        self.renderer.as_ref().map_or("gpu", |renderer| renderer.renderer())
    }

    fn paint(&mut self, list: &octara_primitives::paint::DisplayList, scale: f32) -> Result<Picture, String> {
        if self.bounded {
            validate_size(list.size, scale)?;
        }
        self.renderer.as_mut().ok_or_else(|| "the window has no GPU renderer".to_owned())?.paint(list, scale)
    }
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
    waking: Arc<AtomicBool>,
}

enum Stage {
    Start,
    Settling {
        until: Instant,
        display_failure: Option<String>,
    },
    /// Waiting to sample the window server after submitting an Octara frame.
    Display {
        at: Instant,
        until: Instant,
        frame: DisplayCapture,
        geometry: DisplayGeometry,
        pending: Vec<String>,
    },
    Snapshots {
        window: Window,
        id: CaptureId,
        scale: f32,
        pending: Vec<String>,
        fallback: Option<String>,
    },
}

#[derive(Clone, Copy, PartialEq)]
struct DisplayGeometry {
    size: (u32, u32),
    scale: f32,
}

impl DisplayGeometry {
    fn current(window: &Window) -> Option<Self> {
        super::ww(window, |host| {
            let size = (host.config.width, host.config.height);
            let drawable = host.drawable_size();
            let scale = host.scale();
            (size == (drawable.width, drawable.height) && scale == window.scale()).then_some(Self { size, scale })
        })
        .flatten()
    }

    fn is_current(self, window: &Window, frame: &DisplayCapture) -> bool {
        frame.is_current(window) && Self::current(window) == Some(self)
    }
}

fn env<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

impl Job {
    /// The capture `OCTARA_CAPTURE` asks for, if any; a thread wakes the loop to drive it.
    pub(crate) fn from_env(window: WindowId, proxy: &winit::event_loop::EventLoopProxy<ShellEvent>) -> Option<Job> {
        let path = std::env::var("OCTARA_CAPTURE").ok()?;
        let after = Duration::from_millis(env("OCTARA_CAPTURE_AFTER_MS").unwrap_or(300));
        Some(Job {
            window,
            path,
            settle: Duration::from_millis(env("OCTARA_CAPTURE_SETTLE_MS").unwrap_or(5000)),
            scroll: env("OCTARA_CAPTURE_SCROLL"),
            pixels: if std::env::var("OCTARA_CAPTURE_PIXELS").as_deref() == Ok("display") { Pixels::Display } else { Pixels::Painted },
            stage: Stage::Start,
            scale: None,
            reply: None,
            waking: wake(proxy, after),
        })
    }

    pub(crate) fn live(
        window: WindowId,
        path: String,
        settle: Duration,
        scale: Option<f32>,
        pixels: Pixels,
        reply: super::inspect::Responder,
        proxy: &winit::event_loop::EventLoopProxy<ShellEvent>,
    ) -> Self {
        Self {
            window,
            path,
            settle,
            scroll: None,
            pixels,
            stage: Stage::Start,
            scale,
            reply: Some(reply),
            waking: wake(proxy, Duration::ZERO),
        }
    }
}

fn wake(proxy: &winit::event_loop::EventLoopProxy<ShellEvent>, after: Duration) -> Arc<AtomicBool> {
    let waking = Arc::new(AtomicBool::new(true));
    let active = waking.clone();
    let proxy = proxy.clone();
    std::thread::spawn(move || {
        let until = Instant::now() + after;
        while active.load(Ordering::Relaxed) && Instant::now() < until {
            std::thread::sleep(POLL.min(until.saturating_duration_since(Instant::now())));
        }
        while active.load(Ordering::Relaxed) && proxy.send_event(ShellEvent::Capture).is_ok() {
            std::thread::sleep(POLL);
        }
    });
    waking
}

impl Drop for Job {
    fn drop(&mut self) {
        self.waking.store(false, Ordering::Relaxed);
        if let Stage::Snapshots { window, id, .. } = &self.stage {
            window.capture_cancel(*id);
        }
    }
}

impl Shell {
    /// One step of the capture in progress.
    pub(crate) fn capture_step(&mut self) {
        let Some(mut job) = self.capture.take() else { return };
        self.tick();
        if let Some(app) = &self.app {
            app.poll();
        }
        if job.reply.as_ref().is_some_and(|reply| !reply.connected.load(Ordering::Relaxed)) {
            return;
        }
        let Some(mut w) = self.app.as_ref().and_then(|app| app.window(job.window)) else {
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
            return;
        };
        if job.reply.as_ref().is_some_and(|reply| !reply.target_current()) {
            if let Some(reply) = job.reply.take() {
                reply.send(octara::inspect::Reply::fail(
                    &octara::diagnostics::STALE_TARGET,
                    "the observed window was rebuilt while capturing; inspect again",
                ));
            }
            return;
        }
        let now = Instant::now();
        if let Stage::Start = job.stage {
            if let Some(dy) = job.scroll {
                let (width, height) = self.window_options.size;
                w.wheel(width / 2.0, height / 2.0, 0.0, dy);
                // Drawn, so the views are placed where the page now is.
                self.draw(&w);
            }
            job.stage = Stage::Settling { until: now + job.settle, display_failure: None };
        }
        if let Stage::Settling { until, ref display_failure } = job.stage {
            let pending = w.unsettled();
            if (!w.frame_is_settled() || !pending.is_empty()) && now < until {
                self.capture = Some(job);
                self.show();
                return;
            }
            if job.reply.is_some()
                && let Err(why) = validate_size(w.size(), job.scale.unwrap_or(w.scale()))
            {
                self.report(&w, Err(why), job);
                return;
            }
            let fallback = if job.pixels == Pixels::Display && (display_failure.is_none() || Instant::now() < until) {
                match self.present_for_capture(&w) {
                    Ok((frame, geometry)) => {
                        job.stage = Stage::Display { at: Instant::now() + SHOWN, until, frame, geometry, pending };
                        self.capture = Some(job);
                        return;
                    }
                    Err(why) if Instant::now() < until => {
                        job.stage = Stage::Settling { until, display_failure: Some(why) };
                        self.capture = Some(job);
                        return;
                    }
                    Err(why) => Some(why),
                }
            } else {
                display_failure.clone()
            };
            if let Err(why) = self.begin_capture_snapshots(&mut w, &mut job, fallback) {
                self.report(&w, Err(why), job);
                return;
            }
        }
        if let Stage::Display { at, until, ref frame, geometry, .. } = job.stage {
            if now < at {
                self.capture = Some(job);
                return;
            }
            let picture = if geometry.is_current(&w, frame) {
                let picture = window_picture(&w);
                geometry.is_current(&w, frame).then_some(picture)
            } else {
                None
            };
            let fallback = match picture {
                None if Instant::now() < until => {
                    job.stage = Stage::Settling { until, display_failure: Some(DISPLAY_CHANGED.into()) };
                    self.capture = Some(job);
                    self.show();
                    return;
                }
                None => DISPLAY_CHANGED.into(),
                Some(Err(why)) => why,
                Some(Ok(picture)) if job.reply.is_some() && f64::from(picture.width) * f64::from(picture.height) > MAX_PIXELS => {
                    self.report(&w, Err("the display picture exceeds 32 megapixels".into()), job);
                    return;
                }
                Some(Ok(picture)) if (picture.width, picture.height) != geometry.size => {
                    let (width, height) = geometry.size;
                    format!(
                        "the display picture dimensions {}×{} differ from the submitted surface dimensions {width}×{height}",
                        picture.width, picture.height
                    )
                }
                Some(Ok(picture)) => {
                    let Stage::Display { frame, pending, .. } = std::mem::replace(&mut job.stage, Stage::Start) else { unreachable!() };
                    let result = w.capture_display(frame, &picture, &job.path, geometry.scale, pending);
                    self.report(&w, result, job);
                    return;
                }
            };
            if let Err(why) = self.begin_capture_snapshots(&mut w, &mut job, Some(fallback)) {
                self.report(&w, Err(why), job);
                return;
            }
        }
        if let Stage::Snapshots { id, scale, ref pending, ref fallback, .. } = job.stage {
            if !w.capture_awaiting(id).is_empty() {
                self.capture = Some(job);
                return;
            }
            let (pending, fallback) = (pending.clone(), fallback.clone());
            // Painted with the window's renderer offscreen.
            let result = {
                let mut gpu = self.shared.gpu.borrow_mut();
                let renderer = gpu.as_mut().map(|g| &mut g.renderer as &mut dyn Painter);
                let mut painter = CapturePainter { renderer, bounded: job.reply.is_some() };
                w.capture_finish(id, &mut painter, &job.path, scale, pending, fallback)
            };
            self.report(&w, result, job);
        }
    }

    fn present_for_capture(&mut self, window: &Window) -> Result<(DisplayCapture, DisplayGeometry), String> {
        let frame = window.capture_display_begin()?;
        let geometry = DisplayGeometry::current(window).ok_or("the window and its drawing surface disagree on size or scale")?;
        if !self.draw(window) {
            return Err("the window's current frame could not be presented".into());
        }
        if !geometry.is_current(window, &frame) {
            return Err("the window changed while submitting the frame to display".into());
        }
        Ok((frame, geometry))
    }

    fn begin_capture_snapshots(&mut self, window: &mut Window, job: &mut Job, fallback: Option<String>) -> Result<(), String> {
        // Apply pending AppKit placements before requesting native snapshots.
        #[cfg(target_os = "macos")]
        if super::ww(window, |host| host.native.as_ref().is_some_and(|native| native.has_pending_placements())).unwrap_or(false) {
            self.draw(window);
        }
        let scale = job.scale.unwrap_or(window.scale());
        let pending = window.unsettled();
        let id = window.capture_begin(Instant::now() + SNAPSHOT_LIMIT)?;
        job.stage = Stage::Snapshots { window: window.clone(), id, scale, pending, fallback };
        Ok(())
    }

    /// Prints the report (stdout) and each native view's AppKit frame (stderr), and exits.
    fn report(&mut self, w: &Window, result: Result<octara::capture::Capture, String>, mut job: Job) {
        if let Some(reply) = job.reply.take() {
            let response = match result {
                Ok(report) => octara::inspect::Reply { capture: Some(Box::new(report)), ..octara::inspect::Reply::ok() },
                Err(why) => octara::inspect::Reply::fail(&octara::diagnostics::CAPTURE_FAILED, why),
            };
            reply.send(response);
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
