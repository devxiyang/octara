//! CPU-side presentation receipts for a launch with tracing enabled.

use octara_gpu::FrameStageTimings;
use octara_model::capture::FrameReceipt;
use octara_model::identity::{FrameStamp, WindowId};
use std::time::Instant;

pub(crate) fn display_tick(window: WindowId, tick: crate::os::FrameTick, started: Instant) {
    let record = serde_json::json!({
        "event": "display_tick",
        "window": window,
        "target_ns": tick.target.saturating_duration_since(started).as_nanos(),
        "interval_ns": tick.interval.as_nanos(),
    });
    eprintln!("{record}");
}

pub(crate) fn presented(window: WindowId, receipt: FrameReceipt) {
    let frame = receipt.stamp.frame;
    let record = serde_json::json!({
        "event": "presented",
        "window": window,
        "frame": frame,
        "surface_generation": receipt.stamp.surface_generation,
        "device_generation": receipt.stamp.device_generation,
        "sampled_ns": receipt.stamp.sample.sampled_at.as_nanos(),
        "gpu_complete": receipt.gpu_complete,
    });
    eprintln!("{record}");
}

pub(crate) fn frame_stages(window: WindowId, stamp: FrameStamp, stages: FrameStageTimings) {
    let media_time = |time: Instant| {
        stages.clock_media_s + time.saturating_duration_since(stages.clock_at).as_secs_f64()
            - stages.clock_at.saturating_duration_since(time).as_secs_f64()
    };
    let record = serde_json::json!({
        "event": "frame_stages",
        "window": window,
        "frame": stamp.frame,
        "surface_generation": stamp.surface_generation,
        "tick_s": stages.tick_at.map(media_time),
        "cpu_done_s": media_time(stages.cpu_done_at),
        "encode_done_s": media_time(stages.encode_done_at),
        "gpu_start_s": stages.gpu_start_s,
        "gpu_end_s": stages.gpu_end_s,
        "presented_at_s": stages.presented_at_s,
    });
    eprintln!("{record}");
}
