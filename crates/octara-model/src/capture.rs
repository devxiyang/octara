//! Paint and presentation receipts, independent of how capture pixels are produced.

use crate::identity::{FrameStamp, NodeId};
use crate::paint::Picture;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// The immutable drawing revision and its sample time on the application clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaintSample {
    pub revision: u64,
    pub sampled_at: Duration,
}

/// The pixels actually used for an author paint site.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PaintOutcome {
    Rendered,
    /// Composited the sampled drawing with an earlier, host-accepted region image.
    Transition {
        old: PaintSample,
    },
    Retained {
        /// The drawing revision that produced this output, when supplied by the caller.
        #[serde(default, rename = "source_revision", skip_serializing_if = "Option::is_none")]
        revision: Option<u64>,
    },
    Fallback {
        reason: PaintFallback,
    },
}

/// Why ordinary paint supplied the pixels for an author effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaintFallback {
    ReferenceRenderer,
    UnsupportedProgram,
    InvalidSize,
    PreparationFailed,
    EncodingFailed,
    BackdropUnavailable,
    HistoryUnavailable,
    /// The region's ordinary paint requires a surface the local effect cannot sample.
    UnsupportedRegion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaintReceipt {
    pub source: NodeId,
    pub outcome: PaintOutcome,
}

/// A picture with the author paint outcomes observed while composing it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaintedPicture {
    pub picture: Picture,
    pub effects: Vec<PaintReceipt>,
}

/// Observations of a frame the host accepted for presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameReceipt {
    pub stamp: FrameStamp,
    /// The renderer observed completion of this frame's submission.
    pub gpu_complete: bool,
    /// Presentation time on the platform's monotonic clock, when observable.
    pub presented_at_ns: Option<u64>,
}

/// The producer of a capture's pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSource {
    /// Octara's composed drawing, with snapshots of native content when available.
    #[default]
    Painted,
    /// The platform's capture of the currently displayed window.
    Display,
}
