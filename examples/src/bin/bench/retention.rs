use super::{FORMAT, Offscreen, SAMPLES, Sample, WARMUP, metal_renderer, percentiles, print_features};
use octara::app::{App, InputEvent, Window};
use octara::inspect::{self, Delivery, Reply, Request, Snapshot};
use octara::platform::{HeadlessPlatform, PublishedFrame, WindowOptions};
use octara::prelude::El;
use octara_examples::gpu_lab::{self, replay::SIZE};
use octara_primitives::capture::{PaintOutcome, PaintReceipt, Painter};
use octara_primitives::identity::NodeId;
use octara_render::gpu::{FrameProfile, GpuPassKind, GpuTimings};
use octara_render::{GpuRenderer, wgpu};
use std::sync::mpsc;
use std::time::Duration;

const HZ: f64 = 120.0;
const FLOW: &str = "gpu-preview";
const MATERIALS: &str = "gpu-materials";
const PHASE: &str = "gpu-phase";
const FLOW_PASSES: [&str; 3] = ["shader_lab_flow_compute", "shader_lab_color_resolve", "shader_lab_grid_composite"];

pub(super) fn run() -> Result<(), String> {
    let (mut renderer, info, features) = metal_renderer()?;
    println!("## Shader Lab Flow retention on {} ({:?}, {FORMAT:?})\n", info.name, info.backend);
    print_features(features);
    verify(&mut renderer)?;
    println!(
        "\n{}×{} logical px. Both modes scroll the same materials list at {HZ:.0} Hz virtual time with a fixed Flow phase.",
        SIZE.0, SIZE.1
    );
    println!("{WARMUP} warmup and {SAMPLES} measured frames per case, one draw in flight. Times are in µs (P50 / P95).\n");
    println!(
        "| Scale | Pair | Flow mode | CPU record | Completion | Canvas pixels recorded | Retained draws | Flow compute / resolve / grid | GPU reports |"
    );
    println!("| --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |");
    for scale in [1, 2] {
        let target = Offscreen::new(&renderer, scale);
        for pair in 1..=3 {
            let retained_first = pair % 2 == 0;
            for retained in [retained_first, !retained_first] {
                let run = measure(&mut renderer, &target, retained)?;
                let mode = if retained { "retained" } else { "live" };
                let counts = run.flow_passes.map(|count| count.to_string()).join(" / ");
                println!(
                    "| {scale}× | {pair} | {mode} | {} | {} | {} | {} | {} | {} / {SAMPLES} |",
                    percentiles(run.frames.iter().map(|sample| sample.record)),
                    percentiles(run.frames.iter().map(|sample| sample.complete)),
                    run.frames.iter().map(|sample| sample.profile.canvas_pixels).sum::<u64>(),
                    run.frames.iter().map(|sample| sample.profile.retained_canvases).sum::<u32>(),
                    if run.reports == 0 { "—" } else { &counts },
                    run.reports,
                );
            }
        }
    }
    Ok(())
}

struct Measurement {
    frames: Vec<Sample>,
    flow_passes: [usize; 3],
    reports: usize,
}

fn measure(renderer: &mut GpuRenderer, target: &Offscreen, retained: bool) -> Result<Measurement, String> {
    with_scene(retained, target.scale, |app, window| {
        let viewport = window.kernel().find(MATERIALS).ok_or("The materials viewport is missing.")?;
        let scroll_range = {
            let kernel = window.kernel();
            kernel.scroll_size(viewport).1 - kernel.client_size(viewport).1
        };
        if scroll_range <= 0.0 {
            return Err("The materials viewport has no scroll range.".into());
        }
        let distance = 0.8 * scroll_range.min(160.0);
        let step = distance / (WARMUP + SAMPLES) as f32;
        let first = published(window)?;
        let seed = target.draw(renderer, &first, |_| {})?;
        let _ = verify_work(seed.profile, None, false)?;

        let mut frames = Vec::with_capacity(SAMPLES);
        let mut flow_passes = [0; 3];
        let mut reports = 0;
        let mut offset = 0.0;
        let mut revision = first.revision;
        let start = app.now();
        let (send, receive) = mpsc::channel();
        for frame in 1..=WARMUP + SAMPLES {
            let time = start + Duration::from_secs_f64(frame as f64 / HZ);
            app.advance_to(time);
            let moved = scroll(window, step)?;
            let drawing = published(window)?;
            if moved <= offset
                || drawing.revision <= revision
                || drawing.scale != target.scale as f32
                || drawing.sampled_at.abs_diff(time) > Duration::from_nanos(1)
            {
                return Err(format!("Scroll frame {frame} did not advance its visible offset and settled drawing."));
            }
            offset = moved;
            revision = drawing.revision;
            let send = send.clone();
            let sample = target.draw(renderer, &drawing, move |timings| {
                let _ = send.send(timings);
            })?;
            let timings = receive.try_recv().ok();
            let counts = verify_work(sample.profile, timings.as_ref(), retained)?;
            if frame > WARMUP {
                frames.push(sample);
                if let Some(counts) = counts {
                    reports += 1;
                    for (total, count) in flow_passes.iter_mut().zip(counts) {
                        *total += count;
                    }
                }
            }
        }
        Ok(Measurement { frames, flow_passes, reports })
    })
}

fn verify(renderer: &mut GpuRenderer) -> Result<(), String> {
    let one = Offscreen::new(renderer, 1);
    let two = Offscreen::new(renderer, 2);
    let initial_pixels = with_scene(false, 1, |app, window| {
        let source = source(window)?;
        let rect = bounds(&window.inspect(), FLOW)?;
        draw_outcome(renderer, &one, &published(window)?, source, PaintOutcome::Rendered)?;
        let initial = read_pixels(renderer, &one, rect)?;
        let mut previous = initial.clone();
        for (index, phase) in [0.63, 0.84].into_iter().enumerate() {
            app.advance_to(Duration::from_secs_f64((index + 1) as f64 / HZ));
            set_phase(window, phase)?;
            draw_outcome(renderer, &one, &published(window)?, source, PaintOutcome::Rendered)?;
            let changed = read_pixels(renderer, &one, rect)?;
            if changed == previous {
                return Err("Updating the live Flow phase left its submitted pixels unchanged.".into());
            }
            previous = changed;
        }
        Ok(initial)
    })?;

    with_scene(true, 1, |app, window| {
        let source = source(window)?;
        let before = window.inspect();
        let flow = bounds(&before, FLOW)?;
        let materials = bounds(&before, MATERIALS)?;
        let initial = published(window)?;
        draw_outcome(renderer, &one, &initial, source, PaintOutcome::Rendered)?;
        if read_pixels(renderer, &one, flow)? != initial_pixels {
            return Err("Retained and live Flow disagree for identical inputs.".into());
        }
        let old_materials = read_pixels(renderer, &one, materials)?;

        app.advance_to(Duration::from_secs_f64(1.0 / HZ));
        if scroll(window, 32.0)? <= 0.0 {
            return Err("The materials viewport did not scroll.".into());
        }
        let scrolled = published(window)?;
        let retained = PaintOutcome::Retained { revision: Some(initial.revision) };
        draw_outcome(renderer, &one, &scrolled, source, retained)?;
        let after = window.inspect();
        for id in [FLOW, "gpu-grid", PHASE, "gpu-glass-open"] {
            let old = before.find(id).ok_or_else(|| format!("Missing control {id} before scrolling."))?;
            let new = after.find(id).ok_or_else(|| format!("Missing control {id} after scrolling."))?;
            if (old.bounds, &old.target) != (new.bounds, &new.target) {
                return Err(format!("Scrolling moved or remounted {id}."));
            }
        }
        if scrolled.revision <= initial.revision
            || read_pixels(renderer, &one, flow)? != initial_pixels
            || read_pixels(renderer, &one, materials)? == old_materials
        {
            return Err("Scroll did not publish changed materials pixels and identical retained Flow pixels.".into());
        }
        let capture = renderer.paint_with_receipts(&scrolled.list, scrolled.scale, Some(scrolled.paint_sample()))?;
        check_outcome(&capture.effects, &scrolled, source, retained)?;

        set_phase(window, 0.63)?;
        let changed = published(window)?;
        draw_outcome(renderer, &one, &changed, source, PaintOutcome::Rendered)?;
        let new_pixels = read_pixels(renderer, &one, flow)?;
        if new_pixels == initial_pixels {
            return Err("Updating the retained Flow phase left its submitted pixels unchanged.".into());
        }
        let replay = renderer.paint_with_receipts(&scrolled.list, scrolled.scale, Some(scrolled.paint_sample()))?;
        check_outcome(&replay.effects, &scrolled, source, PaintOutcome::Rendered)?;
        if replay.picture != capture.picture {
            return Err("Replaying the frozen old sample changed its captured pixels.".into());
        }
        let _ = scroll(window, -16.0)?;
        draw_outcome(renderer, &one, &published(window)?, source, PaintOutcome::Retained { revision: Some(changed.revision) })?;
        if read_pixels(renderer, &one, flow)? != new_pixels {
            return Err("The subsequent scroll did not keep the updated Flow pixels.".into());
        }

        set_scale(window, 2);
        let resized = published(window)?;
        if resized.scale != 2.0 {
            return Err("The resized Flow drawing did not publish at 2×.".into());
        }
        let flow = bounds(&window.inspect(), FLOW)?;
        draw_outcome(renderer, &two, &resized, source, PaintOutcome::Rendered)?;
        let scaled_pixels = read_pixels(renderer, &two, flow)?;
        let _ = scroll(window, -16.0)?;
        draw_outcome(renderer, &two, &published(window)?, source, PaintOutcome::Retained { revision: Some(resized.revision) })?;
        if read_pixels(renderer, &two, flow)? != scaled_pixels {
            return Err("The subsequent scroll did not keep the 2× Flow pixels.".into());
        }
        println!(
            "Verified live phase changes, matching initial pixels, unchanged Flow/control geometry during material scrolling, and retained source revisions {}, {}, and {} after first draw, phase change, and 2× scale change.",
            initial.revision, changed.revision, resized.revision
        );
        println!(
            "The capture at revision {} reused the first output. After the phase changed, re-recording that old sample reproduced its pixels.",
            scrolled.revision
        );
        Ok(())
    })
}

fn with_scene<T>(retained: bool, scale: u32, run: impl FnOnce(&App, &mut Window) -> Result<T, String>) -> Result<T, String> {
    let options = gpu_lab::options();
    let app = App::with_config(HeadlessPlatform::default(), options.app);
    let view: fn() -> El = if retained { gpu_lab::retained_view } else { gpu_lab::view };
    let result = (|| {
        let mut window = app.open_window(WindowOptions::new(options.title).size(SIZE.0 as f32, SIZE.1 as f32), view)?;
        set_scale(&mut window, scale);
        run(&app, &mut window)
    })();
    app.shutdown();
    result
}

fn set_scale(window: &mut Window, scale: u32) {
    window.event(InputEvent::Resize { width: SIZE.0 as f32, height: SIZE.1 as f32, scale: scale as f32, safe_area: None });
}

fn scroll(window: &mut Window, dy: f32) -> Result<f32, String> {
    let reply = delivered(inspect::handle(window, Request::Scroll { id: Some(MATERIALS.into()), dx: 0.0, dy }))?;
    reply.scroll.map(|scroll| scroll.offset[1]).ok_or("The materials scroll returned no offset.".into())
}

fn set_phase(window: &mut Window, value: f64) -> Result<(), String> {
    delivered(inspect::handle(window, Request::SetValue { id: Some(PHASE.into()), value }))?;
    let expected = format!("{value:.2}");
    if window.kernel().text_of("gpu-phase-value").as_deref() != Some(expected.as_str()) {
        return Err(format!("The Flow control did not accept phase {value:.2}."));
    }
    Ok(())
}

fn delivered(reply: Reply) -> Result<Reply, String> {
    if reply.ok && reply.delivery == Some(Delivery::Delivered) {
        Ok(reply)
    } else {
        Err(format!("Shader Lab input was not delivered ({:?}, {:?}).", reply.code, reply.message))
    }
}

fn published(window: &Window) -> Result<PublishedFrame, String> {
    window.published_frame().ok_or("The Shader Lab drawing did not settle.".into())
}

fn source(window: &Window) -> Result<NodeId, String> {
    window.kernel().find(FLOW).ok_or("The Flow canvas is missing.".into())
}

fn bounds(snapshot: &Snapshot, id: &str) -> Result<[f32; 4], String> {
    snapshot.find(id).map(|node| node.bounds).ok_or_else(|| format!("The Shader Lab element {id} is missing."))
}

fn check_outcome(receipts: &[PaintReceipt], drawing: &PublishedFrame, source: NodeId, expected: PaintOutcome) -> Result<(), String> {
    let actual = receipts.iter().find(|receipt| receipt.source == source).map(|receipt| receipt.outcome);
    if actual != Some(expected) {
        return Err(format!("Drawing {} has Flow outcome {actual:?}, expected {expected:?}.", drawing.revision));
    }
    Ok(())
}

fn draw_outcome(
    renderer: &mut GpuRenderer,
    target: &Offscreen,
    drawing: &PublishedFrame,
    source: NodeId,
    expected: PaintOutcome,
) -> Result<(), String> {
    let (send, receive) = mpsc::channel();
    let sample = target.draw_checked(
        renderer,
        drawing,
        move |timings| {
            let _ = send.send(timings);
        },
        |receipts| check_outcome(receipts, drawing, source, expected),
    )?;
    let _ = verify_work(sample.profile, receive.try_recv().ok().as_ref(), matches!(expected, PaintOutcome::Retained { .. }))?;
    Ok(())
}

fn verify_work(profile: FrameProfile, timing: Option<&GpuTimings>, retained: bool) -> Result<Option<[usize; 3]>, String> {
    if profile.retained_canvases != u32::from(retained) || (profile.canvas_pixels > 0) == retained || profile.material_batches == 0 {
        return Err(format!("Unexpected work for retained={retained}: {profile:?}"));
    }
    let Some(timing) = timing else { return Ok(None) };
    let counts = FLOW_PASSES.map(|name| timing.passes.iter().filter(|pass| pass.kind == GpuPassKind::Author(name)).count());
    if timing.truncated || counts != [usize::from(!retained); 3] {
        return Err(format!("Unexpected Flow compute/resolve/grid counts {counts:?} for retained={retained}."));
    }
    Ok(Some(counts))
}

/// Reads the submitted target, without recording the canvas a second time.
fn read_pixels(renderer: &GpuRenderer, target: &Offscreen, [x, y, width, height]: [f32; 4]) -> Result<Vec<u8>, String> {
    let scale = target.scale as f32;
    let left = (x * scale).ceil() as u32;
    let top = (y * scale).ceil() as u32;
    let width = (((x + width) * scale).floor() as u32).min(target.size.0).saturating_sub(left);
    let height = (((y + height) * scale).floor() as u32).min(target.size.1).saturating_sub(top);
    if width == 0 || height == 0 {
        return Err("The submitted comparison region is empty.".into());
    }
    let row = width * 4;
    let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = target.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Shader Lab retention test readback"),
        size: u64::from(padded) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = target.device.create_command_encoder(&Default::default());
    let texture = target.view.texture();
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { origin: wgpu::Origin3d { x: left, y: top, z: 0 }, ..texture.as_image_copy() },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(height) },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    let (send, receive) = mpsc::channel();
    encoder.map_buffer_on_submit(&buffer, wgpu::MapMode::Read, .., move |mapped| {
        let _ = send.send(mapped);
    });
    target.wait(Some(renderer.queue().submit([encoder.finish()])))?;
    receive.try_recv().map_err(|error| error.to_string())?.map_err(|error| error.to_string())?;
    let mapped = buffer.get_mapped_range(..).map_err(|error| error.to_string())?;
    let pixels = mapped.chunks_exact(padded as usize).flat_map(|bytes| bytes[..row as usize].iter().copied()).collect();
    drop(mapped);
    buffer.unmap();
    Ok(pixels)
}
