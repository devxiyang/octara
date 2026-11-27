//! Native windows, input and asynchronous GPU presentation for Octara.

mod config;
mod media;
pub mod os;
mod presentation;
mod shell;
mod trace;
mod window;

pub use config::{LaunchCapture, ShellConfig};
pub use media::DiskMediaCache;
pub use octara_gpu::{SurfaceFrame, SurfaceRenderer, SurfaceRendererFactory};
pub use shell::ShellEvent;

use octara::runtime::{BuildCx, Input, WindowId, WindowOptions};
use octara::{App, AppDefinition, IntoView, WindowDefinition};
use octara_model::capture::FrameReceipt;
use os::OsHost;
use std::time::Duration;
use winit::event_loop::EventLoop;

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
    pub surface_renderer: Option<SurfaceRendererFactory>,
    pub hook: Option<Box<dyn ShellHook>>,
    pub first_frame: Option<Box<dyn FnOnce(FrameReceipt)>>,
}

/// Runs a native app with the supplied OS host, renderer and event-loop hook.
pub fn run_app_with_options(application: AppDefinition, options: NativeAppOptions) -> Result<(), ShellError> {
    let event_loop = EventLoop::<ShellEvent>::with_user_event().build().map_err(|error| ShellError(error.to_string()))?;
    let mut shell = shell::Shell::new(application, event_loop.create_proxy(), options);
    event_loop.run_app(&mut shell).map_err(|error| ShellError(error.to_string()))?;
    shell.take_error().map_or(Ok(()), Err)
}
