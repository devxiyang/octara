#[path = "replay.rs"]
mod replay;
#[path = "retention.rs"]
mod retention;

use octara::app::{App, InputEvent};
use octara::inspect::{self, Request};
use octara::platform::{HeadlessPlatform, PublishedFrame};
use octara_examples::gpu_lab;
use octara_primitives::{capture::PaintReceipt, paint::Item};
use octara_render::gpu::{FrameInput, FrameProfile, GpuTimings};
use octara_render::{GpuRenderer, wgpu};
use std::collections::BTreeMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpu_lab::replay::SIZE;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
const WARMUP: usize = 60;
const SAMPLES: usize = 120;
const EXIT_SAMPLE: Duration = Duration::from_millis(250);

struct Sample {
    profile: FrameProfile,
    record: Duration,
    submit: Duration,
    complete: Duration,
}

struct Measurement {
    scene: &'static str,
    samples: Vec<Sample>,
    gpu: Vec<GpuTimings>,
}

pub(super) fn run() -> Result<(), String> {
    let (mut renderer, info, features) = metal_renderer()?;
    println!("## Offscreen Shader Lab on {} ({:?}, {FORMAT:?})\n", info.name, info.backend);
    println!("{}×{} logical px, {WARMUP} warmup and {SAMPLES} measured frames per scene, one submission in flight.", SIZE.0, SIZE.1);
    print_features(features);
    println!("Times are in µs (P50 / P95). Completion measures encoder creation through draw submission completion.");
    println!("Each scene replays a fixed display list. The exit scene samples the fade {} ms after dismissal.\n", EXIT_SAMPLE.as_millis());

    for scale in [1, 2] {
        let measurements = measure_scale(&mut renderer, scale)?;
        print_measurements(scale, &measurements);
    }
    Ok(())
}

pub(super) fn run_replay() -> Result<(), String> {
    replay::run()
}

pub(super) fn run_retention() -> Result<(), String> {
    retention::run()
}

fn metal_renderer() -> Result<(GpuRenderer, wgpu::AdapterInfo, wgpu::Features), String> {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::METAL;
    let instance = wgpu::Instance::new(descriptor);
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .map_err(|e| format!("Could not obtain an offscreen Metal adapter ({e})."))?;
    let features = adapter.features().intersection(wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { required_features: features, ..Default::default() }))
            .map_err(|e| format!("Could not create the Metal device ({e})."))?;
    Ok((GpuRenderer::with_device(device, queue, FORMAT), adapter.get_info(), features))
}

fn print_features(features: wgpu::Features) {
    println!("Timestamp features {features:?}. Debug assertions {}.", if cfg!(debug_assertions) { "on" } else { "off" });
    if features.contains(wgpu::Features::TIMESTAMP_QUERY) && !features.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS) {
        println!(
            "This adapter times individual passes, including instrumented author passes. Encoder spans and copy durations are unavailable."
        );
    }
}

struct Offscreen {
    device: wgpu::Device,
    view: wgpu::TextureView,
    size: (u32, u32),
    scale: u32,
}

impl Offscreen {
    fn new(renderer: &GpuRenderer, scale: u32) -> Self {
        let size = (SIZE.0 * scale, SIZE.1 * scale);
        let device = renderer.device().clone();
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Shader Lab benchmark target"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self { device, view: target.create_view(&Default::default()), size, scale }
    }

    fn draw(
        &self,
        renderer: &mut GpuRenderer,
        drawing: &PublishedFrame,
        report: impl FnOnce(GpuTimings) + Send + 'static,
    ) -> Result<Sample, String> {
        self.draw_checked(renderer, drawing, report, |_| Ok(()))
    }

    fn wait(&self, submission_index: Option<wgpu::SubmissionIndex>) -> Result<(), String> {
        self.device
            .poll(wgpu::PollType::Wait { submission_index, timeout: Some(Duration::from_secs(10)) })
            .map_err(|error| format!("Waiting for GPU completion at {}× failed ({error}).", self.scale))?;
        Ok(())
    }

    fn draw_checked(
        &self,
        renderer: &mut GpuRenderer,
        drawing: &PublishedFrame,
        report: impl FnOnce(GpuTimings) + Send + 'static,
        check: impl FnOnce(&[PaintReceipt]) -> Result<(), String>,
    ) -> Result<Sample, String> {
        let started = Instant::now();
        let encoder = self.device.create_command_encoder(&Default::default());
        let input = FrameInput { list: &drawing.list, scale: drawing.scale, size: self.size, sample: Some(drawing.paint_sample()) };
        let (recorded, profile) = renderer.record_sampled(encoder, &self.view, input, Some(Box::new(report)));
        let profile = profile.expect("an offscreen benchmark requests profiling");
        check(recorded.paint_receipts())?;
        let submitting = Instant::now();
        let record = submitting.duration_since(started);
        let (submission, presentation) = recorded.submit_for_presentation();
        let submit = submitting.elapsed();
        self.wait(Some(submission))?;
        let complete = started.elapsed();
        renderer.accept_presentation(presentation);
        // Draining Metal's separate timestamp resolves is outside the completion measurement.
        while renderer.profile_pending() && renderer.poll_profile() {
            self.wait(None)?;
        }
        Ok(Sample { profile, record, submit, complete })
    }
}

fn measure_scale(renderer: &mut GpuRenderer, scale: u32) -> Result<[Measurement; 4], String> {
    let (width, height) = SIZE;
    let target = Offscreen::new(renderer, scale);
    let options = gpu_lab::options();
    let app = App::with_config(HeadlessPlatform::default(), options.app);
    let mut window = app.open_window(options.window.size(width as f32, height as f32), gpu_lab::view)?;
    window.event(InputEvent::Resize { width: width as f32, height: height as f32, scale: scale as f32, safe_area: None });

    let drawing = window.published_frame().ok_or("The normal scene did not publish its drawing.")?;
    let normal = measure(renderer, &target, &drawing, "UI + canvas")?;
    window.tap("gpu-glass-open")?;
    app.settle_motion();
    let refraction = |window: &mut octara::app::Window, value| {
        let reply = inspect::handle(window, Request::SetValue { id: Some("gpu-glass-refraction".into()), value });
        if reply.ok { Ok(()) } else { Err(reply.message.unwrap_or_else(|| "Could not set the Glass refraction.".into())) }
    };
    refraction(&mut window, 0.0)?;
    let drawing = window.published_frame().ok_or("Glass direct sample did not publish its drawing.")?;
    let direct = measure(renderer, &target, &drawing, "Glass direct sample")?;
    refraction(&mut window, 0.75)?;
    // Accept the live lens before retiring the mount so the fade uses its last output.
    let drawing = window.published_frame().ok_or("Glass refraction did not publish its drawing.")?;
    let glass = measure(renderer, &target, &drawing, "Glass refraction")?;
    if [&direct, &glass].into_iter().flat_map(|m| &m.samples).any(|sample| {
        sample.profile.backdrop_copy_pixels == 0 || sample.profile.backdrop_reuses == 0 || sample.profile.backdrop_allocations != 0
    }) {
        return Err(format!("The {scale}× Glass scene did not reuse its backdrop allocation in every measured frame."));
    }
    window.tap("gpu-glass-cancel")?;
    app.advance(EXIT_SAMPLE);
    let exit = window.published_frame().ok_or("Glass exit did not publish its drawing.")?;
    if !exit.list.items().iter().any(|item| matches!(item, Item::PushLayer { opacity, .. } if *opacity > 0.0 && *opacity < 1.0)) {
        return Err(format!("The {scale}× Glass exit scene did not produce a translucent layer."));
    }
    let exit = measure(renderer, &target, &exit, "Glass exit")?;
    if exit.samples.iter().any(|sample| sample.profile.retained_backdrops != 1 || sample.profile.backdrop_copy_pixels != 0) {
        return Err(format!("The {scale}× Glass exit scene did not reuse its backdrop in every frame."));
    }
    app.shutdown();
    Ok([normal, direct, glass, exit])
}

fn measure(renderer: &mut GpuRenderer, target: &Offscreen, display: &PublishedFrame, scene: &'static str) -> Result<Measurement, String> {
    let (report, results) = mpsc::channel();
    let mut samples = Vec::with_capacity(SAMPLES);
    for frame in 0..WARMUP + SAMPLES {
        let report = report.clone();
        let sample = target.draw(renderer, display, move |timings| {
            if frame >= WARMUP {
                let _ = report.send(timings);
            }
        })?;
        if frame >= WARMUP {
            samples.push(sample);
        }
    }
    if samples.iter().any(|sample| sample.profile.canvas_pixels == 0 || sample.profile.material_batches == 0) {
        return Err(format!("The {}× {scene} scene did not render its canvas and materials in every frame.", target.scale));
    }
    Ok(Measurement { scene, samples, gpu: results.try_iter().collect() })
}

fn print_measurements(scale: u32, measurements: &[Measurement]) {
    println!("### {scale}× ({}×{} device px)\n", SIZE.0 * scale, SIZE.1 * scale);
    println!("| Scene | CPU prepare | CPU encode | CPU submit | Completion | GPU span | GPU reports / scheduled | Truncated |");
    println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for m in measurements {
        println!(
            "| {} | {} | {} | {} | {} | {} | {} / {} | {} |",
            m.scene,
            percentiles(m.samples.iter().map(|s| s.profile.prepare)),
            percentiles(m.samples.iter().map(|s| s.profile.encode)),
            percentiles(m.samples.iter().map(|s| s.submit)),
            percentiles(m.samples.iter().map(|s| s.complete)),
            percentiles(m.gpu.iter().filter_map(|t| t.span)),
            m.gpu.len(),
            m.samples.iter().filter(|s| s.profile.gpu_sampled).count(),
            m.gpu.iter().filter(|t| t.truncated).count(),
        );
    }
    println!(
        "\n| Scene | UI/layer passes | Draw batches | Material batches | Layer targets (px) | Author targets (px) | Backdrop copies (px) | Retained backdrops | Backdrop allocations / reuses | Backdrop cache peak (bytes) |"
    );
    println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for m in measurements {
        let p = &m.samples[0].profile;
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} / {} | {} |",
            m.scene,
            p.render_passes,
            p.draw_batches,
            p.material_batches,
            p.layer_pixels,
            p.canvas_pixels,
            p.backdrop_copy_pixels,
            p.retained_backdrops,
            m.samples.iter().map(|s| s.profile.backdrop_allocations).sum::<u32>(),
            m.samples.iter().map(|s| s.profile.backdrop_reuses).sum::<u32>(),
            m.samples.iter().map(|s| s.profile.backdrop_cache_peak_bytes).max().unwrap_or_default(),
        );
    }
    print_passes(measurements.iter().map(|m| (m.scene, m.gpu.as_slice())));
}

fn print_passes<'a>(measurements: impl IntoIterator<Item = (&'a str, &'a [GpuTimings])>) {
    println!("\n| Scene | GPU scope | Timed scopes | Sampled frames | Time per frame (µs) P50 / P95 |");
    println!("| --- | --- | ---: | ---: | ---: |");
    for (scene, frames) in measurements {
        let mut scopes: BTreeMap<&str, (usize, Vec<Duration>)> = BTreeMap::new();
        for frame in frames.iter().filter(|t| !t.truncated) {
            let mut totals: BTreeMap<&str, (usize, Duration)> = BTreeMap::new();
            for pass in &frame.passes {
                let (count, duration) = totals.entry(pass.kind.as_str()).or_default();
                *count += 1;
                *duration += pass.duration;
            }
            for (scope, (count, duration)) in totals {
                let (total_count, frame_times) = scopes.entry(scope).or_default();
                *total_count += count;
                frame_times.push(duration);
            }
        }
        for (scope, (count, durations)) in scopes {
            println!("| {scene} | {scope} | {count} | {} | {} |", durations.len(), percentiles(durations));
        }
    }
    println!();
}

fn percentiles(times: impl IntoIterator<Item = Duration>) -> String {
    let mut times: Vec<_> = times.into_iter().collect();
    if times.is_empty() {
        return "—".into();
    }
    times.sort_unstable();
    let us = |percentile: usize| times[(times.len() * percentile).div_ceil(100) - 1].as_secs_f64() * 1e6;
    format!("{:.1} / {:.1}", us(50), us(95))
}
