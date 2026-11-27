//! CPU-side presentation receipts for a launch with tracing enabled.

use octara_model::capture::FrameReceipt;
use octara_model::identity::WindowId;

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
