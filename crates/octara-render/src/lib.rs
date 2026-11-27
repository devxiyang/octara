//! Renderers for Octara's display list ([`octara_model::paint`]).
//!
//! - [`CpuRenderer`]: the reference renderer. Deterministic (bundled fonts, plain float math), it
//!   gives agents and tests screenshots without a GPU and is the oracle the GPU renderer is
//!   compared with.
//! - [`GpuRenderer`] (feature `gpu`): wgpu, batched instances, materials, canvases, and backdrop effects.
//!   The built-in shader implements the formulas of [`shape`] and uses the same [`glyphs`] and
//!   [`paths`] rasterization as the reference renderer.

pub mod cpu;
pub mod glyphs;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod paths;
mod pixels;
pub mod shape;

pub use cpu::CpuRenderer;
#[cfg(feature = "gpu")]
pub use gpu::{GpuRenderer, RecordedFrame, SurfaceFrame, SurfaceRenderer, SurfaceRendererFactory, plan};
#[cfg(feature = "gpu")]
pub use wgpu;
