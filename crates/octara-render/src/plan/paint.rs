//! Device-free descriptors and geometry for custom paint.

use super::batch::Paint;
use octara_model::capture::PaintSample;
use octara_model::geometry::Rect;
use octara_model::identity::NodeId;
use octara_model::paint::{CustomPaint, PaintInput, PaintProgram};
use rustc_hash::FxBuildHasher;
use std::any::Any;
use std::hash::{BuildHasher, Hash, Hasher};
use std::ptr;
use std::time::Duration;

/// Source and stable batching identity of a WGSL material.
#[derive(Clone, Copy, Debug, Eq)]
pub struct Material {
    wgsl: &'static str,
    hash: u64,
}

impl PartialEq for Material {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash && (ptr::eq(self.wgsl, other.wgsl) || self.wgsl == other.wgsl)
    }
}

impl Hash for Material {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash);
    }
}

impl Material {
    pub fn new(wgsl: &'static str) -> Self {
        Self { wgsl, hash: FxBuildHasher.hash_one(wgsl) }
    }

    /// The shader source to compile for this material.
    pub fn wgsl(self) -> &'static str {
        self.wgsl
    }
}

pub struct CanvasPaint {
    pub program: PaintProgram,
    pub source: NodeId,
    pub size: [u32; 2],
    pub input: PaintInput,
    pub rect: Rect,
    pub radius: [f32; 4],
    pub sampled_at: Option<Duration>,
}

impl CanvasPaint {
    pub fn from_custom(
        paint: &CustomPaint,
        fallback: Paint,
        mode: CanvasMode,
        scale: f32,
        sample: Option<PaintSample>,
        max_dimension: u32,
    ) -> Option<Self> {
        let size = canvas_size(fallback.rect, max_dimension)?;
        let (rect, radius, sampled_at) = if mode == CanvasMode::Retained {
            let Rect { w, h, .. } = paint.fallback.rect;
            (Rect::new(0.0, 0.0, w, h), paint.fallback.radius, None)
        } else {
            let [x, y, w, h] = fallback.rect.map(|value| value / scale);
            (Rect::new(x, y, w, h), fallback.radius.map(|radius| radius / scale), sample.map(|sample| sample.sampled_at))
        };
        Some(Self {
            program: paint.program.clone(),
            source: paint.fallback.source,
            size,
            input: paint.input.clone(),
            rect,
            radius,
            sampled_at,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CanvasMode {
    Volatile,
    Retained,
    Backdrop,
    Transition,
}

/// Canvas planning metadata with a typed recording payload supplied by its backend.
pub struct CanvasProgram {
    pub mode: CanvasMode,
    payload: Box<dyn Any + Send + Sync>,
}

impl CanvasProgram {
    pub fn new(mode: CanvasMode, payload: impl Any + Send + Sync) -> Self {
        Self { mode, payload: Box::new(payload) }
    }

    pub fn payload<T: Any>(&self) -> Option<&T> {
        self.payload.downcast_ref()
    }
}

pub(crate) fn canvas_size(rect: [f32; 4], max: u32) -> Option<[u32; 2]> {
    let [x, y, width, height] = rect;
    let size = [width.ceil(), height.ceil()];
    (x.is_finite() && y.is_finite() && size.into_iter().all(|axis| axis >= 1.0 && axis <= max as f32)).then(|| size.map(|axis| axis as u32))
}

pub(crate) const CANVAS_BYTES: u64 = 32 * 1024 * 1024;

pub(crate) fn rgba_bytes([width, height]: [u32; 2]) -> u64 {
    // The shared canvas and transition working format uses four 16-bit channels.
    (u64::from(width) * u64::from(height)).saturating_mul(8)
}

pub(crate) fn can_retain_canvas(size: [u32; 2]) -> bool {
    rgba_bytes(size) <= CANVAS_BYTES
}
