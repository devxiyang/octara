//! Headless execution using the same turns, geometry, and hit testing as the native host.

use crate::{
    InspectHost, Inspector,
    protocol::{AgentReceipt, AgentRequest, Capture},
};
use octara::{
    App, AppDefinition,
    runtime::{
        HostCommand, Input, WindowRuntime,
        turn::{TurnError, TurnReceipt},
    },
};
use octara_model::{
    capture::{CaptureSource, PaintSample},
    identity::WindowId,
};
use octara_pipeline::{NativeEngine, NativeEngineConfig};
use octara_render::CpuRenderer;
use std::time::Duration;

pub struct HeadlessDriver {
    app: App,
    inspector: Inspector,
    host: PaintedCapture,
}

impl Default for HeadlessDriver {
    fn default() -> Self {
        Self::new(NativeEngineConfig::default())
    }
}

impl HeadlessDriver {
    pub fn new(config: NativeEngineConfig) -> Self {
        Self::with_app(App::new(move |_, _| Box::new(NativeEngine::new(config))))
    }

    pub fn with_namespace(namespace: [u64; 2], config: NativeEngineConfig) -> Self {
        Self::with_app(App::with_namespace(namespace, move |_, _| Box::new(NativeEngine::new(config))))
    }

    fn with_app(app: App) -> Self {
        Self { app, inspector: Inspector::default(), host: PaintedCapture::default() }
    }

    /// Install the application's audit rules, payload codec, or trace recorder before launch.
    pub fn with_inspector(mut self, inspector: Inspector) -> Self {
        self.inspector = inspector;
        self
    }

    pub fn launch(&mut self, definition: AppDefinition) -> Result<Vec<WindowId>, TurnError> {
        definition.launch(&mut self.app, None)
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

    pub fn dispatch(&mut self, window: WindowId, input: Input) -> Result<TurnReceipt, TurnError> {
        self.inspector.record_input(&self.app, window, &input);
        self.app.dispatch(window, input)
    }

    pub fn advance_to(&mut self, time: Duration) -> Result<TurnReceipt, TurnError> {
        self.app.advance_to(time)
    }

    pub fn replay(&mut self, recording: &crate::replay::Replay) -> Result<Vec<crate::replay::Step>, crate::replay::ReplayError> {
        recording.play(&mut self.app, &mut self.inspector, &mut self.host)
    }

    pub fn take_host_commands(&mut self) -> Vec<HostCommand> {
        self.app.runtime_mut().take_host_commands()
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
