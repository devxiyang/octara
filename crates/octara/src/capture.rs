//! Captures: an agent's picture of the app, native views included, with a report of what the
//! picture can be trusted for ([design](../../../docs/design/capture.md)).
//!
//! The `capture` request of the [inspector protocol](crate::inspect::Request) settles the app,
//! asks the [native host](crate::native::NativeHost) for each shown view's snapshot, and paints
//! the frame with the snapshots in place through a [`Painter`]: the window's GPU renderer in a
//! shell, the reference renderer headless (`octara-render` implements both). The reply is a
//! [`Capture`] report.

use crate::native::NativeId;
use crate::paint::DisplayList;
use serde::{Deserialize, Serialize};

/// A painted frame: RGBA8, straight alpha, sRGB.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({}×{})", self.width, self.height)
    }
}

impl Picture {
    /// Writes it as an 8-bit sRGB PNG.
    pub fn save_png(&self, path: &str) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let mut w = enc.write_header().map_err(|e| format!("{path}: {e}"))?;
        w.write_image_data(&self.rgba).map_err(|e| format!("{path}: {e}"))?;
        w.finish().map_err(|e| format!("{path}: {e}"))
    }
}

/// What renders a capture's frame.
pub trait Painter {
    /// `gpu` or `reference`, for the report.
    fn renderer(&self) -> &'static str;
    /// The display list at `scale` device pixels per logical pixel.
    fn paint(&mut self, list: &DisplayList, scale: f32) -> Result<Picture, String>;
}

/// Where a capture's pixels come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pixels {
    /// Octara composed it: its frame with each native view's snapshot in place.
    #[default]
    Painted,
    /// The window server's picture of the window: what the user sees.
    Display,
}

/// What a native view shows in a capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shown {
    /// Its own snapshot, drawn in its place.
    Snapshot,
    /// The placeholder box: its host takes no snapshots (`reason` says why).
    Placeholder,
    /// The snapshot was asked for and did not come (`reason` says why).
    Failed,
    /// The view itself (the window server's picture).
    Live,
}

/// One native view in a capture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeShot {
    /// Its test id, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub kind: String,
    /// `[x, y, width, height]` in logical pixels.
    pub rect: [f32; 4],
    pub shown: Shown,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What the snapshot cannot show (WebKit: hardware video and WebGL come out black).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub misses: Vec<String>,
}

/// A capture's report: the picture, and what it can be trusted for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub pixels: Pixels,
    /// Why `display` was asked and `painted` given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    /// `gpu`, `reference` or `display`.
    pub renderer: String,
    /// Nothing was running when the picture was taken.
    pub settled: bool,
    /// What was still running when the wait ended: tasks, motion, native views loading.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native: Vec<NativeShot>,
}

impl Capture {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a capture serializes")
    }
}

/// A capture in progress: the snapshots asked for, and what each host answered.
#[derive(Default)]
pub(crate) struct Capturing {
    /// Each shown view: `Ok(misses)` when its snapshot is coming, `Err(reason)` when not.
    pub asked: Vec<(NativeId, Result<Vec<String>, String>)>,
}
