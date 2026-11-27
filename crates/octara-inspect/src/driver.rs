//! Headless execution using the same cycles, geometry, and hit testing as the native host.

mod native;

use crate::{
    InspectHost, Inspector,
    protocol::{AgentReceipt, AgentRequest, Capture},
};
use native::NativeHost;
use octara::{
    App, AppDefinition,
    runtime::{
        HostCommand, Input, WindowRuntime,
        cycle::{AppError, CycleReceipt},
    },
};
use octara_model::{
    capture::{CaptureSource, PaintSample},
    identity::WindowId,
};
use octara_pipeline::NativeEngineConfig;
use octara_render::CpuRenderer;
use std::time::Duration;

pub struct HeadlessDriver {
    host: HeadlessHost,
    app: App,
    inspector: Inspector,
}

impl Default for HeadlessDriver {
    fn default() -> Self {
        Self::new(NativeEngineConfig::default())
    }
}

impl HeadlessDriver {
    pub fn new(config: NativeEngineConfig) -> Self {
        let host = HeadlessHost::default();
        let app = App::new(host.native.engine_factory(config));
        Self { app, inspector: Inspector::default(), host }
    }

    pub fn with_namespace(namespace: [u64; 2], config: NativeEngineConfig) -> Self {
        let host = HeadlessHost::default();
        let app = App::with_namespace(namespace, host.native.engine_factory(config));
        Self { app, inspector: Inspector::default(), host }
    }

    /// Install the application's audit rules, payload codec, or trace recorder before launch.
    pub fn with_inspector(mut self, inspector: Inspector) -> Self {
        self.inspector = inspector;
        self.inspector.attach_module_trace(&mut self.app);
        self
    }

    pub fn launch(&mut self, definition: AppDefinition) -> Result<Vec<WindowId>, AppError> {
        let windows = definition.launch(&mut self.app, None)?;
        self.sync_native_views()?;
        Ok(windows)
    }

    pub fn app(&self) -> &App {
        &self.app
    }

    pub fn app_mut(&mut self) -> &mut App {
        &mut self.app
    }

    pub fn execute(&mut self, request: AgentRequest) -> AgentReceipt {
        self.inspector.execute(&mut self.app, &mut self.host, request)
    }

    pub fn exchange(&mut self, payload: &[u8]) -> Vec<u8> {
        self.inspector.exchange(&mut self.app, &mut self.host, payload)
    }

    pub fn dispatch(&mut self, window: WindowId, input: Input) -> Result<CycleReceipt, AppError> {
        self.inspector.record_input(&self.app, window, &input);
        let receipt = self.app.dispatch(window, input)?;
        self.sync_native_views()?;
        Ok(receipt)
    }

    pub fn advance_to(&mut self, time: Duration) -> Result<CycleReceipt, AppError> {
        let receipt = self.app.advance_to(time)?;
        self.sync_native_views()?;
        Ok(receipt)
    }

    /// Synchronize registered native views after using the application directly through `app_mut`.
    pub fn sync_native_views(&mut self) -> Result<(), AppError> {
        self.host.synchronize(&mut self.app)
    }

    pub fn replay(&mut self, recording: &crate::replay::Replay) -> Result<Vec<crate::replay::Step>, crate::replay::ReplayError> {
        recording.play(&mut self.app, &mut self.inspector, &mut self.host)
    }

    pub fn take_host_commands(&mut self) -> Vec<HostCommand> {
        self.app.runtime_mut().take_host_commands()
    }
}

#[derive(Default)]
struct HeadlessHost {
    native: NativeHost,
    capture: PaintedCapture,
}

impl InspectHost for HeadlessHost {
    fn synchronize(&mut self, app: &mut App) -> Result<(), AppError> {
        self.native.sync(app)
    }

    fn capture(&mut self, window: &WindowRuntime, source: CaptureSource, now: Duration) -> Result<Capture, String> {
        self.capture.capture(window, source, now)
    }
}

/// CPU paint capture for an inspection bridge with a native display list.
pub struct PaintedCapture {
    renderer: CpuRenderer,
}

impl Default for PaintedCapture {
    fn default() -> Self {
        Self { renderer: CpuRenderer::new() }
    }
}

impl InspectHost for PaintedCapture {
    fn capture(&mut self, window: &WindowRuntime, source: CaptureSource, _now: Duration) -> Result<Capture, String> {
        if source != CaptureSource::Painted {
            return Err("this inspection bridge supplies painted capture only".into());
        }
        capture_painted(&mut self.renderer, window)
    }
}

/// Paint the most recent committed display list with the CPU renderer.
fn capture_painted(renderer: &mut CpuRenderer, window: &WindowRuntime) -> Result<Capture, String> {
    let commit = window.last_commit().ok_or("the window has no committed frame")?;
    let draw = commit.draw.as_deref().ok_or("the frame has no display list")?;
    let sample = PaintSample {
        revision: commit.frame.ok_or("the frame has no sequence")?,
        sampled_at: window.committed_at().ok_or("the committed frame has no sample time")?,
    };
    let painted = renderer.paint_with_receipts(draw, window.options().viewport.scale, Some(sample))?;
    Ok(Capture { source: CaptureSource::Painted, sample, picture: painted.picture, effects: painted.effects })
}
