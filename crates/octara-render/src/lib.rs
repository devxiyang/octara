//! Octara's device-free frame planner, CPU reference renderer, and optional wgpu backend.
//!
//! The default `plan` feature compiles display lists into render passes and supplies shared
//! rasterizers, instance packing, and damage tracking. The `wgpu` feature records those
//! plans into GPU commands on native and WebGPU devices.

#[cfg(all(feature = "plan", not(target_arch = "wasm32")))]
mod blocking;
#[cfg(feature = "plan")]
pub mod cpu;
#[cfg(feature = "plan")]
pub mod glyphs;
#[cfg(feature = "wgpu")]
pub mod gpu;
#[cfg(feature = "plan")]
mod image_mips;
#[cfg(feature = "plan")]
pub mod paths;
#[cfg(feature = "plan")]
mod pixels;
#[cfg(feature = "plan")]
pub mod plan;
#[cfg(feature = "plan")]
pub mod shape;

#[cfg(all(feature = "plan", not(target_arch = "wasm32")))]
#[doc(hidden)]
pub use blocking::block_on;
#[cfg(feature = "wgpu")]
pub use gpu::{RecordedFrame, SurfaceFrame, SurfaceRenderer, SurfaceRendererFactory, WgpuRenderer};
#[cfg(feature = "plan")]
pub use plan::{
    CpuRenderer, DamageRect, ExternalTextureHandle, ExternalTextureKind, FramePlan, FrameProfile, GlyphCache, GlyphInst, ImageInst,
    ImageMips, OutputColor, PlannedBatch, QuadInst, ShadowInst, SpatialTable, plan_frame, plan_frame_cached, plan_frame_spatial,
};
#[cfg(feature = "wgpu")]
pub use wgpu;
