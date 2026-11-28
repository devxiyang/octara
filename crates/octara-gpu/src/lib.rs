//! Native GPU execution and readback for Octara.

#![cfg(not(target_arch = "wasm32"))]

mod backend;
mod executor;
mod generation;
mod readback;
mod schedule;
mod wgpu_backend;

pub use backend::{BackendCapabilities, DeviceDrawnFrame, DrawableAcquisition, DrawableTarget, GpuBackend, GpuDevice, SurfaceSession};
pub use executor::{CanvasWork, DeviceExecutor, DrawTarget, DrawWork, DrawnFrame, FrameProgress, GpuError, GpuTask, SurfaceLease};
pub use generation::{DeviceCapacity, DeviceGeneration, DeviceNotice, DeviceNotifier};
pub use octara_render::{SurfaceFrame, SurfaceRenderer, SurfaceRendererFactory};
pub use readback::{CaptureOutput, CapturePixels, CaptureWriter, StagedCapture};
pub use wgpu_backend::{WgpuBackend, WgpuDevice, WgpuDrawableTarget, WgpuSurfaceSession};
