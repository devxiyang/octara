//! Time GPU encoding and retained canvas output with the Shader Lab.

use octara::prelude::*;
use octara::{PaintOutcome, PaintSample};
use octara_examples_desktop::gpu_lab;
use octara_pipeline::NativeEngine;
use octara_render::{
    WgpuRenderer,
    gpu::{FrameInput, FrameTarget, OutputColor},
    wgpu,
};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let retained = match std::env::args().nth(1).as_deref() {
        None | Some("gpu") => false,
        Some("gpu-retain") => true,
        Some(mode) => return Err(format!("unknown GPU benchmark mode {mode:?}").into()),
    };
    gpu(retained)
}

fn gpu(retained: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut renderer = WgpuRenderer::new().ok_or("no compatible GPU")?;
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
