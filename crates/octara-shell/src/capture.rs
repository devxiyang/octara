//! Captures from a window ([design](../../../docs/design/capture.md)): settle on the wall clock,
//! take the native views' snapshots, paint the frame with the window's GPU renderer offscreen.
//!
//! `OCTARA_CAPTURE=shot.png` captures once and exits, printing the report (JSON) on stdout:
//! after `OCTARA_CAPTURE_AFTER_MS` (default 300), scrolled down `OCTARA_CAPTURE_SCROLL` pixels
//! at the window's center, settling for at most `OCTARA_CAPTURE_SETTLE_MS` (default 5000).

use super::{Shell, ShellEvent};
use std::time::{Duration, Instant};

/// How long a native view's snapshot may take.
const SNAPSHOT_LIMIT: Duration = Duration::from_secs(5);
/// How often a capture in progress looks again.
const POLL: Duration = Duration::from_millis(50);

/// A capture in progress.
pub(crate) struct Job {
    path: String,
    settle: Duration,
    scroll: Option<f32>,
    stage: Stage,
}

enum Stage {
    Start,
    Settling { until: Instant },
    Snapshots { until: Instant, pending: Vec<String> },
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
            stage: Stage::Start,
        })
    }
}

impl Shell {
    /// One step of the capture in progress.
    pub(crate) fn capture_step(&mut self) {
        let Some(mut job) = self.capture.take() else { return };
        if self.app.is_none() || self.gpu.is_none() {
            self.capture = Some(job);
            return;
        }
        let now = Instant::now();
        if let Stage::Start = job.stage {
            if let Some(dy) = job.scroll {
                let (w, h) = self.options.size;
                if let Some(app) = &mut self.app {
                    app.wheel(w / 2.0, h / 2.0, 0.0, dy);
                }
                // Drawn, so the views are placed where the page now is.
                self.draw();
            }
            job.stage = Stage::Settling { until: now + job.settle };
        }
        if let Stage::Settling { until } = job.stage {
            self.tick();
            let app = self.app.as_mut().expect("app");
            let pending = app.unsettled();
            if !pending.is_empty() && now < until {
                self.capture = Some(job);
                self.redraw();
                return;
            }
            app.capture_begin();
            job.stage = Stage::Snapshots { until: now + SNAPSHOT_LIMIT, pending };
        }
        if let Stage::Snapshots { until, ref pending } = job.stage {
            let app = self.app.as_mut().expect("app");
            if !app.capture_awaiting().is_empty() && now < until {
                self.capture = Some(job);
                return;
            }
            let pending = pending.clone();
            self.finish(&job.path, pending);
        }
    }

    /// Paints the frame with the window's renderer offscreen, prints the report, and exits.
    fn finish(&mut self, path: &str, pending: Vec<String>) {
        let scale = self.scale();
        let (Some(g), Some(app)) = (&mut self.gpu, &mut self.app) else { return };
        match app.capture_finish(&mut g.renderer, path, scale, pending, None) {
            Ok(report) => println!("{}", report.to_json()),
            Err(e) => eprintln!("octara-shell: capture {path}: {e}"),
        }
        #[cfg(target_os = "macos")]
        if let Some(n) = &self.native {
            for line in n.frames() {
                eprintln!("octara-shell: {line}");
            }
        }
        self.done = true;
    }
}
