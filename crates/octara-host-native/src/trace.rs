//! CPU-side presentation receipts for a launch with tracing enabled.

use octara_model::capture::FrameReceipt;
use octara_model::identity::WindowId;
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
