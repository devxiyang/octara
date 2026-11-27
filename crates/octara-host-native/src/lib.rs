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

use octara::runtime::{BuildCx, WindowId, WindowOptions};
use octara::{App, AppDefinition, IntoView, WindowDefinition};
use octara_model::capture::FrameReceipt;
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
    launch(application, ShellConfig::from_env(), None, None, None)
}

pub fn run_app_with_config(application: AppDefinition, config: ShellConfig) -> Result<(), ShellError> {
    launch(application, config, None, None, None)
}

/// Launches the application with a surface backend shared by its native windows.
pub fn run_app_with_surface_renderer(application: AppDefinition, factory: SurfaceRendererFactory) -> Result<(), ShellError> {
    launch(application, ShellConfig::from_env(), None, None, Some(factory))
}

pub fn run_app_with_first_frame(application: AppDefinition, first_frame: impl FnOnce(FrameReceipt) + 'static) -> Result<(), ShellError> {
    launch(application, ShellConfig::from_env(), Some(Box::new(first_frame)), None, None)
}

/// Runs after publication on the event-loop thread. A returned delay schedules the next tick.
pub trait ShellHook {
    fn tick(&mut self, app: &mut App, presented: &dyn Fn(WindowId) -> Option<FrameReceipt>) -> Result<Option<Duration>, ShellError>;

    /// Exit the event loop through its ordinary shutdown path after this tick.
    fn shutdown_requested(&self) -> bool {
        false
    }
}

pub fn run_app_with_hook(application: AppDefinition, hook: impl ShellHook + 'static) -> Result<(), ShellError> {
    launch(application, ShellConfig::from_env(), None, Some(Box::new(hook)), None)
}

fn launch(
    application: AppDefinition,
    config: ShellConfig,
    first_frame: Option<Box<dyn FnOnce(FrameReceipt)>>,
    hook: Option<Box<dyn ShellHook>>,
    surface_renderer: Option<SurfaceRendererFactory>,
) -> Result<(), ShellError> {
    let event_loop = EventLoop::<ShellEvent>::with_user_event().build().map_err(|error| ShellError(error.to_string()))?;
    let mut shell = shell::Shell::new(application, config, event_loop.create_proxy(), first_frame, hook, surface_renderer);
    event_loop.run_app(&mut shell).map_err(|error| ShellError(error.to_string()))?;
    shell.take_error().map_or(Ok(()), Err)
}
