//! Renderers for Octara's display list ([`octara_primitives::paint`]).
//!
//! - [`CpuRenderer`]: the reference renderer. Deterministic (bundled fonts, plain float math), it
//!   gives agents and tests screenshots without a GPU and is the oracle the GPU renderer is
//!   compared with.
//! - [`GpuRenderer`] (feature `gpu`): wgpu, one WGSL shader on every platform, one instance per
//!   item and one draw call per frame. It implements the formulas of [`shape`] and uses the same
//!   [`glyphs`] and [`paths`] rasterization, and is tested against the reference renderer.

pub mod cpu;
pub mod glyphs;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod paths;
pub mod shape;

pub use cpu::CpuRenderer;
#[cfg(feature = "gpu")]
pub use gpu::GpuRenderer;
#[cfg(feature = "gpu")]
pub use wgpu;

/// The reference renderer as a reusable capture painter.
pub fn reference_painter() -> Box<dyn octara_primitives::capture::Painter> {
    Box::new(CpuRenderer::new())
}
