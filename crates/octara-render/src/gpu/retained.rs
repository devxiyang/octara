//! Render target compatibility for the retained wgpu frame.

use super::prepare::PreparedFrame;
use super::{FrameTarget, OutputColor};
use crate::plan::SpatialTable;

pub(super) struct CachedFrame {
    pub(super) scroll: SpatialTable,
    pub(super) target: RetainedTarget,
    pub(super) frame: PreparedFrame,
    pub(super) atlas_bytes: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct RetainedTarget {
    extent: [u32; 2],
    format: wgpu::TextureFormat,
    color: OutputColor,
    copyable: bool,
}

impl From<FrameTarget<'_>> for RetainedTarget {
    fn from(target: FrameTarget<'_>) -> Self {
        let texture = target.texture;
        Self {
            extent: [texture.width(), texture.height()],
            format: texture.format(),
            color: target.color,
            copyable: texture.usage().contains(wgpu::TextureUsages::COPY_SRC),
        }
    }
}
