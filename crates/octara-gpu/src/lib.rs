//! Native GPU execution and readback for Octara.

#![cfg(not(target_arch = "wasm32"))]

mod backend;
mod executor;
mod generation;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod metal_backend;
mod readback;
mod schedule;
#[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
mod wgpu_backend;
#[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
mod wgpu_executor;

pub use backend::{
    BackendCapabilities, DeviceDrawnFrame, DrawableAcquisition, DrawableTarget, FrameStageReport, FrameStageTimings, GpuBackend, GpuDevice,
    SurfaceSession,
};
pub use executor::{DrawWork, DrawnFrame, FrameProgress, GpuError, GpuTask};
pub use generation::{DeviceCapacity, DeviceGeneration, DeviceNotice, DeviceNotifier};
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use metal_backend::{MetalBackend, MetalDrawableTarget, MetalGpuDevice, MetalSurfaceLease, MetalSurfaceSession};
#[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
pub use octara_render::{SurfaceFrame, SurfaceRenderer, SurfaceRendererFactory};
pub use readback::{CaptureOutput, CapturePixels, CaptureWriter, StagedCapture};
#[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
pub use wgpu_backend::{WgpuBackend, WgpuDevice, WgpuDrawableTarget, WgpuSurfaceSession};
#[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
pub use wgpu_executor::{CanvasWork, DeviceExecutor, DrawTarget, SurfaceLease};
