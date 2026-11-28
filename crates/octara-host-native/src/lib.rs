//! Native windows, input and asynchronous GPU presentation for Octara.

mod config;
mod media;
pub mod os;
mod presentation;
mod shell;
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "linux", test))]
mod text_bridge;
mod trace;
mod window;

pub use config::{LaunchCapture, ShellConfig};
pub use media::{DiskMediaCache, MediaCancelRegistration, MediaFetchRequest, MediaFetchResponse, MediaTransport};
pub use octara_gpu::FrameStageTimings;
#[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
pub use octara_gpu::{SurfaceFrame, SurfaceRenderer, SurfaceRendererFactory};
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use os::apple_media::AppleImageSource;
pub use os::{HostWake, NativeSurfaceRendererFactory, PlatformEvent};

use octara::runtime::{BuildCx, Input, WindowId, WindowOptions};
use octara::{App, AppDefinition, IntoView, WindowDefinition};
use octara_model::capture::FrameReceipt;
use os::OsHost;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug)]
pub struct ShellError(pub String);

impl std::fmt::Display for ShellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ShellError {}

pub fn run<V: IntoView>(options: WindowOptions, root: impl FnOnce(&mut BuildCx<'_>) -> V + 'static) -> Result<(), ShellError> {
    run_app(AppDefinition::new().window(WindowDefinition::new(options, root)))
}

pub fn run_app(application: AppDefinition) -> Result<(), ShellError> {
    run_app_with_options(application, NativeAppOptions::default())
}

pub fn run_app_with_config(application: AppDefinition, config: ShellConfig) -> Result<(), ShellError> {
    run_app_with_options(application, NativeAppOptions { config: Some(config), ..Default::default() })
}

/// Launches the application with a surface backend shared by its native windows.
#[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
pub fn run_app_with_surface_renderer(application: AppDefinition, factory: SurfaceRendererFactory) -> Result<(), ShellError> {
    run_app_with_options(application, NativeAppOptions { surface_renderer: Some(factory), ..Default::default() })
}

pub fn run_app_with_first_frame(application: AppDefinition, first_frame: impl FnOnce(FrameReceipt) + 'static) -> Result<(), ShellError> {
    run_app_with_options(application, NativeAppOptions { first_frame: Some(Box::new(first_frame)), ..Default::default() })
}

/// Observes native window activity on the event-loop thread.
pub trait ShellHook {
    /// Runs immediately before the shell dispatches a host input to the app.
    fn before_input(&mut self, _app: &App, _window: WindowId, _input: &Input) {}

    /// Runs after publication. A returned delay schedules the next tick.
    fn tick(&mut self, app: &mut App, presented: &dyn Fn(WindowId) -> Option<FrameReceipt>) -> Result<Option<Duration>, ShellError>;

    /// Exit the event loop through its ordinary shutdown path after this tick.
    fn shutdown_requested(&self) -> bool {
        false
    }
}

pub fn run_app_with_hook(application: AppDefinition, hook: impl ShellHook + 'static) -> Result<(), ShellError> {
    run_app_with_options(application, NativeAppOptions { hook: Some(Box::new(hook)), ..Default::default() })
}

/// Native host and rendering extensions installed before the event loop starts.
#[derive(Default)]
pub struct NativeAppOptions {
    /// Defaults to `ShellConfig::from_env()`.
    pub config: Option<ShellConfig>,
    /// Defaults to the platform's built-in host.
    pub os_host: Option<Box<dyn OsHost>>,
    pub surface_renderer: Option<NativeSurfaceRendererFactory>,
    /// Overrides the platform's default HTTP image transport.
    pub media_transport: Option<Arc<dyn MediaTransport>>,
    pub hook: Option<Box<dyn ShellHook>>,
    pub first_frame: Option<Box<dyn FnOnce(FrameReceipt)>>,
}

impl NativeAppOptions {
    /// Uses the supplied transport for remote images. The native loader still owns disk caching and decoding.
    pub fn with_media_transport(mut self, transport: impl MediaTransport) -> Self {
        self.media_transport = Some(Arc::new(transport));
        self
    }
}

/// Runs a native app with the supplied OS host, renderer and event-loop hook.
pub fn run_app_with_options(application: AppDefinition, mut options: NativeAppOptions) -> Result<(), ShellError> {
    let host = options.os_host.take().unwrap_or_else(os::create_os_host);
    host.run(Box::new(shell::Driver::new(application, options))).map_err(ShellError)
}
