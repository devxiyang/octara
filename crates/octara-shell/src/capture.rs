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
use std::time::{Duration, Instant};

/// How long a native view's snapshot may take.
const SNAPSHOT_LIMIT: Duration = Duration::from_secs(5);
/// How often a capture in progress looks again.
const POLL: Duration = Duration::from_millis(50);
/// After the last frame is presented, how long until the window server shows it (two refreshes
/// at 60 Hz, and a margin).
const SHOWN: Duration = Duration::from_millis(40);

/// A capture in progress.
pub(crate) struct Job {
    path: String,
    settle: Duration,
    scroll: Option<f32>,
    pixels: Pixels,
    stage: Stage,
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
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(after);
            // Until the loop is gone.
            while proxy.send_event(ShellEvent::Capture).is_ok() {
                std::thread::sleep(POLL);
            }
        });
        Some(Job {
            path,
            settle: Duration::from_millis(env("OCTARA_CAPTURE_SETTLE_MS").unwrap_or(5000)),
            scroll: env("OCTARA_CAPTURE_SCROLL"),
            pixels: if std::env::var("OCTARA_CAPTURE_PIXELS").as_deref() == Ok("display") { Pixels::Display } else { Pixels::Painted },
            stage: Stage::Start,
        })
    }
}

impl Shell {
    /// One step of the capture in progress (of the first window).
    pub(crate) fn capture_step(&mut self) {
        let Some(mut job) = self.capture.take() else { return };
        let Some(mut w) = self.main_window() else {
            self.capture = Some(job);
            return;
        };
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
            self.tick();
            let pending = w.unsettled();
            if !pending.is_empty() && now < until {
                self.capture = Some(job);
                self.show();
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
            match window_picture(&w) {
                Ok(picture) => {
                    let scale = super::ww(&w, |x| x.scale()).unwrap_or(1.0);
                    let result = w.capture_display(&picture, &job.path, scale, pending);
                    self.report(&w, result, &job.path);
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
            let scale = super::ww(&w, |x| x.scale()).unwrap_or(1.0);
            let result = {
                let mut gpu = self.shared.gpu.borrow_mut();
                let Some(g) = gpu.as_mut() else { return };
                w.capture_finish(&mut g.renderer, &job.path, scale, pending, fallback)
            };
            self.report(&w, result, &job.path);
        }
    }

    /// Prints the report (stdout) and each native view's AppKit frame (stderr), and exits.
    fn report(&mut self, w: &octara::app::Window, result: Result<octara::capture::Capture, String>, path: &str) {
        match result {
            Ok(report) => println!("{}", report.to_json()),
            Err(e) => eprintln!("octara-shell: capture {path}: {e}"),
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
fn window_picture(w: &octara::app::Window) -> Result<octara::capture::Picture, String> {
    #[cfg(target_os = "macos")]
    if let Some(p) = super::ww(w, |x| x.native.as_ref().map(|n| n.window_picture())).flatten() {
        return p;
    }
    let _ = w;
    Err("the window server's picture is taken on macOS only so far".into())
}
