//! Optional CPU timings. A present call returning is neither GPU completion nor physical display.
//!
//! `OCTARA_TRACE_INPUT=1` writes `octara.frame_trace/1` JSON lines on stderr. Frame totals
//! start before DisplayList preparation; event-loop work before that is not in the frame total.
//! Input times start at shell dispatch (or an owned native text callback), not at the hardware
//! event. Coalesced counts are dispatches, including nested key/native edits, not keystrokes.
//! No input content, sample history, GPU timestamps or physical display times are collected.
//! `octara.update_trace/1` separately times shell `advance_to` and wake `poll`, including their
//! timers/tasks, reactive work and layout. Input handlers can contain these intervals; don't add
//! them to handler times. The logical app clock labels a sample; elapsed CPU/wait time is real.

use octara::serde_json::{Value, json};
use std::time::Instant;

fn us(start: Instant, end: Instant) -> f64 {
    end.saturating_duration_since(start).as_secs_f64() * 1_000_000.0
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
}

pub(crate) struct Frame {
    pub(crate) started: Instant,
    pub(crate) app_clock_ms: u64,
    pub(crate) display_list: Instant,
    pub(crate) surface_start: Instant,
    pub(crate) surface_ready: Instant,
    pub(crate) encoded: Instant,
    pub(crate) native_placed: Instant,
    pub(crate) present_start: Option<Instant>,
    pub(crate) present_returned: Option<Instant>,
}

impl Frame {
    pub(crate) fn new(started: Instant, app_clock_ms: u64) -> Self {
        let display_list = Instant::now();
        Self {
            started,
            app_clock_ms,
            display_list,
            surface_start: display_list,
            surface_ready: display_list,
            encoded: display_list,
            native_placed: display_list,
            present_start: None,
            present_returned: None,
        }
    }

    pub(crate) fn cpu_json(&self, finished: Instant) -> Value {
        json!({
            "display_list": us(self.started, self.display_list),
            "frame_setup": us(self.display_list, self.surface_start),
            "surface_acquire": us(self.surface_start, self.surface_ready),
            // Includes renderer preparation/uploads, command encoding and queue.submit.
            "renderer_encode_submit": us(self.surface_ready, self.encoded),
            "native_place": us(self.encoded, self.native_placed),
            "pre_present_notify": self.present_start.map(|at| us(self.native_placed, at)),
            "present_call": self.present_start.zip(self.present_returned).map(|(a, b)| us(a, b)),
            "frame_finish": us(self.present_returned.unwrap_or(self.native_placed), finished),
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
            display_list: t(10),
            surface_start: t(20),
            surface_ready: t(70),
            encoded: t(90),
            native_placed: t(95),
            present_start: Some(t(96)),
            present_returned: Some(t(100)),
        };
        let value = frame.cpu_json(t(110));
        assert_eq!(value["surface_acquire"], 50.0);
        assert_eq!(value["renderer_encode_submit"], 20.0);
        assert_eq!(value["present_call"], 4.0);
        let sum: f64 = value.as_object().unwrap().iter().filter(|(k, _)| *k != "total").map(|(_, v)| v.as_f64().unwrap()).sum();
        assert!((sum - value["total"].as_f64().unwrap()).abs() < 0.001);
        frame.present_start = None;
        frame.present_returned = None;
        assert!(frame.cpu_json(t(110))["present_call"].is_null());
    }
}
