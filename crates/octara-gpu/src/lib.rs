//! Native GPU execution and readback for Octara.

#![cfg(not(target_arch = "wasm32"))]

mod executor;
mod generation;
mod readback;
mod schedule;

pub use executor::{CanvasWork, DeviceExecutor, DrawTarget, DrawWork, DrawnFrame, FrameProgress, GpuError, GpuTask, SurfaceLease};
pub use generation::{DeviceCapacity, DeviceGeneration, DeviceNotice, DeviceNotifier};
pub use octara_render::{SurfaceFrame, SurfaceRenderer, SurfaceRendererFactory};
pub use readback::{CaptureOutput, CapturePixels, CaptureWriter, StagedCapture};
