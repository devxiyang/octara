//! Time the v2 application and projection paths with seeded example state.

use octara::prelude::*;
use octara::{A11yAction, EditorEvent, PaintOutcome, PaintSample, TextEdit};
use octara_examples::{apps, gpu_lab, window_options};
use octara_inspect::{AgentAction, AgentCommand, AgentRequest, HeadlessDriver};
use octara_pipeline::NativeEngine;
use octara_render::{
    GpuRenderer,
    gpu::{FrameInput, FrameTarget, OutputColor},
    wgpu,
};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match std::env::args().nth(1).as_deref() {
        Some("gpu") => gpu(false),
        Some("gpu-retain") => gpu(true),
        None => {
            profile("counter", apps::counter_app(), "inc", 100)?;
            profile("todo (1,000)", app("Todo", |b| apps::todo(b, 1_000)), "toggle-500", 20)?;
            profile("chat (1,000) composer editing", app("Chat", |b| apps::chat(b, 1_000)), "composer", 20)?;
            profile("settings", app("Settings", apps::settings), "notifications", 20)
        }
        Some(mode) => Err(format!("unknown benchmark mode {mode:?}").into()),
    }
}

fn app(title: &str, view: fn(&mut BuildCx<'_>) -> El) -> AppDefinition {
    AppDefinition::new().window(WindowDefinition::new(window_options(title, 800.0, 600.0), view))
}

fn profile(name: &str, app: AppDefinition, id: &str, iterations: u32) -> Result<(), Box<dyn std::error::Error>> {
    let mut driver = HeadlessDriver::default();
    let start = Instant::now();
    let window = driver.launch(app)?[0];
    let init = start.elapsed();
    let snapshot = driver.execute(AgentRequest::new(Some(window), AgentCommand::Observe)).snapshot.ok_or("missing settled observation")?;
    let target = snapshot.nodes.iter().find(|node| node.test_id.as_deref() == Some(id)).ok_or("missing benchmark target")?.target;
    if id == "composer" {
        let focus = AgentAction::Accessibility { action: A11yAction::Focus };
        let receipt = driver.execute(AgentRequest::new(Some(window), AgentCommand::Act { target, action: focus }));
        if !receipt.ok || receipt.handled != Some(true) {
            return Err("the benchmark could not focus the composer".into());
        }
    }
    let start = Instant::now();
    for index in 0..iterations {
        let action = if id == "composer" {
            let snapshot = driver.execute(AgentRequest::new(Some(window), AgentCommand::Observe)).snapshot.ok_or("no observation")?;
            let editor = snapshot.editor.ok_or("the composer lost focus")?;
            let event = EditorEvent::ProposeEdit {
                session: editor.session,
                expected_rev: editor.document_rev,
                edit: TextEdit::Insert { text: if index % 2 == 0 { "a" } else { "b" }.into(), replacement: None },
            };
            AgentAction::Editor { event }
        } else {
            AgentAction::Accessibility { action: A11yAction::Press }
        };
        let receipt = driver.execute(AgentRequest::new(Some(window), AgentCommand::Act { target, action }));
        if !receipt.ok || receipt.handled != Some(true) {
            return Err(receipt.message.unwrap_or_else(|| "action refused".into()).into());
        }
    }
    let state = driver.app().runtime().window(window).expect("benchmark window");
    println!(
        "{name}\tlaunch {:.2?}\tupdate {:.2?}\t{} nodes\t{} transactions",
        init,
        start.elapsed() / iterations,
        snapshot.nodes.len(),
        state.transaction_count()
    );
    Ok(())
}

fn gpu(retained: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut renderer = GpuRenderer::new().ok_or("no compatible GPU")?;
    let mut app = App::new(|_, _| Box::new(NativeEngine::new(Default::default())));
    let window = app.open_window(gpu_lab::options(), if retained { gpu_lab::retained_view } else { gpu_lab::view })?;
    let state = app.runtime().window(window).expect("Shader Lab window");
    let frame = state.last_commit().ok_or("missing Shader Lab frame")?;
    let frame_number = frame.frame.ok_or("missing Shader Lab frame number")?;
    let draw = frame.draw.as_deref().ok_or("missing Shader Lab drawing")?;
    let flow_id = state.tree().test_ids("gpu-preview")[0];
    let device = renderer.device().clone();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Shader Lab benchmark target"),
        size: wgpu::Extent3d { width: draw.size.width as u32, height: draw.size.height as u32, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let start = Instant::now();
    let mut rendered = 0;
    let mut reused = 0;
    let sample = PaintSample { revision: frame_number, sampled_at: Duration::ZERO };
    for _ in 0..20 {
        let encoder = device.create_command_encoder(&Default::default());
        let target = FrameTarget { texture: &texture, color: OutputColor::SrgbOpaque };
        let (frame, _) = renderer.record(encoder, target, FrameInput { list: draw, scale: 1.0, sample: Some(sample) }, None)?;
        let flow =
            frame.paint_receipts().iter().find(|receipt| receipt.source == flow_id).ok_or("GPU frame omitted the Flow paint site")?;
        match flow.outcome {
            PaintOutcome::Rendered => rendered += 1,
            PaintOutcome::Retained { .. } => reused += 1,
            outcome => return Err(format!("Flow paint did not render: {outcome:?}").into()),
        }
        let (submission, presented) = frame.submit_for_presentation();
        device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: Some(Duration::from_secs(10)) })?;
        renderer.accept_presentation(presented);
    }
    if retained && reused == 0 {
        return Err("the GPU did not retain the unchanged Flow canvas".into());
    }
    println!("{}\t20 draws in {:.2?}\tFlow {rendered} rendered, {reused} retained", renderer.renderer(), start.elapsed());
    Ok(())
}
