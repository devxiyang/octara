//! Renderers for Octara's display list ([`octara_model::paint`]).
//!
//! - [`CpuRenderer`]: the reference renderer. Deterministic (bundled fonts, plain float math), it
//!   gives agents and tests screenshots without a GPU and is the oracle the GPU renderer is
//!   compared with.
//! - [`GpuRenderer`] (feature `gpu`): wgpu, batched instances, materials, canvases, and backdrop effects.
//!   The built-in pipelines use the reference renderer's [`shape`] formulas, [`glyphs`] and
//!   [`paths`] rasterizers, and image mip generation.

#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
mod blocking;
pub mod cpu;
pub mod glyphs;
#[cfg(feature = "gpu")]
pub mod gpu;
mod image_mips;
pub mod paths;
mod pixels;
pub mod shape;

#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
#[doc(hidden)]
pub use blocking::block_on;
pub use cpu::CpuRenderer;
#[cfg(feature = "gpu")]
pub use gpu::{GpuRenderer, RecordedFrame, SurfaceFrame, SurfaceRenderer, SurfaceRendererFactory, plan};
#[cfg(feature = "gpu")]
pub use wgpu;
