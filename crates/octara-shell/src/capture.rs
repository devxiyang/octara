//! Captures from a window ([design](../../../docs/design/capture.md)): settle on the wall clock,
//! take the native views' snapshots, paint the frame with the window's GPU renderer offscreen.
//!
//! `OCTARA_CAPTURE=shot.png` captures once and exits, printing the report (JSON) on stdout:
//! after `OCTARA_CAPTURE_AFTER_MS` (default 300), scrolled down `OCTARA_CAPTURE_SCROLL` pixels
//! at the window's center, settling for at most `OCTARA_CAPTURE_SETTLE_MS` (default 5000).
//! `OCTARA_CAPTURE_PIXELS=display` asks for the window server's picture (macOS), painted with a
//! stated `fallback` where there is none.

use super::{Shell, ShellEvent};
use octara::capture::Pixels;
use octara_primitives::capture::{Painter, Picture};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long a native view's snapshot may take.
const SNAPSHOT_LIMIT: Duration = Duration::from_secs(5);
/// How often a capture in progress looks again.
const POLL: Duration = Duration::from_millis(50);
/// After the last frame is presented, how long until the window server shows it (two refreshes
/// at 60 Hz, and a margin).
const SHOWN: Duration = Duration::from_millis(40);
const MAX_PIXELS: f64 = 32.0 * 1024.0 * 1024.0;

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
/// Going through capture_finish even on failure also clears those snapshots.
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
    window: Option<octara::app::WindowId>,
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
    },
    /// Waiting for the presented frame to reach the window server.
    Display {
        at: Instant,
        pending: Vec<String>,
    },
    Snapshots {
        until: Instant,
        pending: Vec<String>,
        fallback: Option<String>,
    },
}

fn env<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

impl Job {
    /// The capture `OCTARA_CAPTURE` asks for, if any; a thread wakes the loop to drive it.
    pub(crate) fn from_env(proxy: &winit::event_loop::EventLoopProxy<ShellEvent>) -> Option<Job> {
        let path = std::env::var("OCTARA_CAPTURE").ok()?;
        let after = Duration::from_millis(env("OCTARA_CAPTURE_AFTER_MS").unwrap_or(300));
        Some(Job {
            window: None,
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
        window: octara::app::WindowId,
        path: String,
        settle: Duration,
        scale: Option<f32>,
        pixels: Pixels,
        reply: super::inspect::Responder,
        proxy: &winit::event_loop::EventLoopProxy<ShellEvent>,
    ) -> Self {
        Self {
            window: Some(window),
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
    }
}

impl Shell {
    /// One step of the capture in progress (of the first window).
    pub(crate) fn capture_step(&mut self) {
        let Some(mut job) = self.capture.take() else { return };
        self.tick();
        if let Some(app) = &self.app {
            app.poll();
        }
        let window = match job.window {
            Some(id) => self.app.as_ref().and_then(|app| app.window(id)),
            None => self.main_window(),
        };
        let Some(mut w) = window else {
            if let Some(reply) = job.reply.take() {
                let (code, message) = if reply.target_current() {
                    (&octara::diagnostics::CAPTURE_FAILED, "the target window closed while capturing")
                } else {
                    (&octara::diagnostics::STALE_TARGET, "the observed window closed while capturing; inspect again")
                };
                reply.send(octara::inspect::Reply::fail(code, message));
            } else {
                self.capture = Some(job);
            }
            return;
        };
        if job.reply.as_ref().is_some_and(|reply| !reply.target_current()) {
            // Finish through a painter that cannot write a picture, so any outstanding
            // native snapshots are cleared without saving pixels from the replacement tree.
            if matches!(job.stage, Stage::Snapshots { .. }) {
                let mut painter = CapturePainter { renderer: None, bounded: true };
                let _ = w.capture_finish(&mut painter, &job.path, 1.0, Vec::new(), None);
            }
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
                let (width, height) = self.options.size;
                w.wheel(width / 2.0, height / 2.0, 0.0, dy);
                // Drawn, so the views are placed where the page now is.
                self.draw(&w);
            }
            job.stage = Stage::Settling { until: now + job.settle };
        }
        if let Stage::Settling { until } = job.stage {
            let pending = w.unsettled();
            if !pending.is_empty() && now < until {
                self.capture = Some(job);
                self.show();
                return;
            }
            if job.reply.is_some()
                && let Err(why) = validate_size(w.size(), job.scale.unwrap_or_else(|| super::ww(&w, |x| x.scale()).unwrap_or(1.0)))
            {
                self.report(&w, Err(why), job);
                return;
            }
            if job.pixels == Pixels::Display {
                // The latest frame, presented; the window server shows it a little later.
                self.draw(&w);
                job.stage = Stage::Display { at: now + SHOWN, pending };
                self.capture = Some(job);
                return;
            }
            w.capture_begin();
            job.stage = Stage::Snapshots { until: now + SNAPSHOT_LIMIT, pending, fallback: None };
        }
        if let Stage::Display { at, ref pending } = job.stage {
            if now < at {
                self.capture = Some(job);
                return;
            }
            let pending = pending.clone();
            let scale = super::ww(&w, |x| x.scale()).unwrap_or(1.0);
            if job.reply.is_some()
                && let Err(why) = validate_size(w.size(), scale)
            {
                self.report(&w, Err(why), job);
                return;
            }
            match window_picture(&w) {
                Ok(picture) => {
                    let result = if job.reply.is_some() && f64::from(picture.width) * f64::from(picture.height) > MAX_PIXELS {
                        Err("the display picture exceeds 32 megapixels".into())
                    } else {
                        w.capture_display(&picture, &job.path, scale, pending)
                    };
                    self.report(&w, result, job);
                    return;
                }
                Err(why) => {
                    w.capture_begin();
                    job.stage = Stage::Snapshots { until: now + SNAPSHOT_LIMIT, pending, fallback: Some(why) };
                }
            }
        }
        if let Stage::Snapshots { until, ref pending, ref fallback } = job.stage {
            if !w.capture_awaiting().is_empty() && now < until {
                self.capture = Some(job);
                return;
            }
            let (pending, fallback) = (pending.clone(), fallback.clone());
            // Painted with the window's renderer offscreen.
            let scale = job.scale.unwrap_or_else(|| super::ww(&w, |x| x.scale()).unwrap_or(1.0));
            let result = {
                let mut gpu = self.shared.gpu.borrow_mut();
                let renderer = gpu.as_mut().map(|g| &mut g.renderer as &mut dyn Painter);
                let mut painter = CapturePainter { renderer, bounded: job.reply.is_some() };
                w.capture_finish(&mut painter, &job.path, scale, pending, fallback)
            };
            self.report(&w, result, job);
        }
    }

    /// Prints the report (stdout) and each native view's AppKit frame (stderr), and exits.
    fn report(&mut self, w: &octara::app::Window, result: Result<octara::capture::Capture, String>, mut job: Job) {
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
fn window_picture(w: &octara::app::Window) -> Result<octara_primitives::capture::Picture, String> {
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
