use super::{FORMAT, Offscreen, Sample, metal_renderer, percentiles, print_features, print_passes};
use octara::app::{App, InputEvent};
use octara::platform::HeadlessPlatform;
use octara_examples::gpu_lab::{
    self,
    replay::{DURATION, Replay, SIZE},
};
use octara_primitives::identity::NodeId;
use octara_primitives::paint::{DisplayList, Item, Part};
use octara_render::GpuRenderer;
use octara_render::gpu::{FrameProfile, GpuTimings};
use std::fmt::Display;
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct Frame {
    update: Duration,
    publish: Duration,
    draw: Sample,
}

struct Measurement {
    scene: &'static str,
    frames: Vec<Frame>,
    gpu: Vec<GpuTimings>,
}

const SCENES: [&str; 4] = ["UI + canvas", "Glass live", "Glass exit", "Glass live + exit"];

pub(super) fn run() -> Result<(), String> {
    let (mut renderer, info, features) = metal_renderer()?;
    println!("## Dynamic offscreen Shader Lab on {} ({:?}, {FORMAT:?})\n", info.name, info.backend);
    println!(
        "{}×{} logical px. Each case runs the same {:.1} s timeline once to warm up and once to measure, with one draw in flight.",
        SIZE.0,
        SIZE.1,
        DURATION.as_secs_f64()
    );
    println!("Virtual frame cadence drives the scene clock. Timings measure elapsed wall time in µs (P50 / P95).");
    println!("Update includes clock advance, actions and UI settling. Publish assembles the settled display list.");
    println!("CPU record includes encoder creation, renderer preparation, encoding and timestamp commands.");
    println!("Completion runs from encoder creation to draw submission completion. Timestamp readback follows that interval.");
    print_features(features);

    for scale in [1, 2] {
        let target = Offscreen::new(&renderer, scale);
        for hz in [60, 120] {
            let (measurements, offset) =
                measure(&mut renderer, &target, hz).map_err(|error| format!("{hz} Hz at {scale}× failed. {error}"))?;
            print_measurements(&target, hz, &measurements, offset);
        }
    }
    Ok(())
}

fn measure(renderer: &mut GpuRenderer, target: &Offscreen, hz: u32) -> Result<([Measurement; 4], f32), String> {
    let options = gpu_lab::options();
    let app = App::with_config(HeadlessPlatform::default(), options.app);
    let result = (|| {
        let mut replay = Replay::new(&app);
        let (width, height) = (SIZE.0 as f32, SIZE.1 as f32);
        let mut window = app.open_window(options.window.size(width, height), replay.view())?;
        window.event(InputEvent::Resize { width, height, scale: target.scale as f32, safe_area: None });
        let preview = window.kernel().find("gpu-preview").ok_or("The Shader Lab flow canvas is missing.")?;
        let (send, receive) = mpsc::channel();
        let mut measurements = SCENES.map(|scene| Measurement { scene, frames: Vec::new(), gpu: Vec::new() });
        let frames = (DURATION.as_millis() as u32 * hz) / 1_000;
        let mut offset = 0.0;
        for warmup in [true, false] {
            replay.restart(&mut window)?;
            let origin = app.now();
            let mut revision = window.published_revision().ok_or("The Shader Lab start frame did not settle.")?;
            for frame in 1..=frames {
                let elapsed = Duration::from_secs_f64(f64::from(frame) / f64::from(hz));
                let expected_time = origin + elapsed;
                let updating = Instant::now();
                app.advance_to(expected_time);
                let phase = replay.update(&mut window, elapsed).map_err(|error| format!("At {elapsed:?}, {error}"))?;
                let update = updating.elapsed();

                let publishing = Instant::now();
                let published = window.published_frame().ok_or_else(|| format!("No settled frame at {elapsed:?}."))?;
                let publish = publishing.elapsed();
                if published.revision <= revision
                    || published.sampled_at.abs_diff(expected_time) > Duration::from_nanos(1)
                    || published.scale != target.scale as f32
                    || published.list.size != (width, height)
                    || flow_phase(&published.list, preview) != Some(phase)
                {
                    return Err(format!("The frame at {elapsed:?} had unexpected time, size, scale, revision or flow phase."));
                }
                revision = published.revision;

                let send = send.clone();
                let draw = target.draw(renderer, &published, move |timings| {
                    if !warmup {
                        let _ = send.send(timings);
                    }
                })?;
                let index = verify_frame(elapsed, &draw.profile)?;
                if !warmup {
                    measurements[index].frames.push(Frame { update, publish, draw });
                    if let Ok(timings) = receive.try_recv() {
                        measurements[index].gpu.push(timings);
                    }
                }
            }
            offset = replay.finish(&window)?;
        }
        Ok((measurements, offset))
    })();
    app.shutdown();
    result
}

fn flow_phase(list: &DisplayList, preview: NodeId) -> Option<f32> {
    list.parts
        .iter()
        .flat_map(|part| match part {
            Part::Item(item) => std::slice::from_ref(item).iter(),
            Part::Drawing { items, .. } => items.iter(),
        })
        .find_map(|item| match item {
            Item::Custom(paint) if paint.fallback.source == preview => Some(paint.params[1]),
            _ => None,
        })
}

fn verify_frame(elapsed: Duration, profile: &FrameProfile) -> Result<usize, String> {
    if profile.canvas_pixels == 0 || profile.material_batches == 0 || profile.retained_backdrops > 1 {
        return Err(format!("The frame at {elapsed:?} had unexpected canvas, material or retained backdrop work."));
    }
    let scene = match (profile.backdrop_copy_pixels > 0, profile.retained_backdrops > 0) {
        (false, false) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (true, true) => 3,
    };
    // These elapsed times occur exactly at both 60 and 120 Hz. Check two independent
    // resources during overlap: a fresh copy for the new lens and the old retained lens.
    let checkpoint = match elapsed.as_millis() {
        100 | 4500 => Some((0, false)),
        1900 => Some((1, false)),
        2150 | 3500 => Some((2, true)),
        2500 => Some((3, true)),
        _ => None,
    };
    if let Some((expected_scene, expected_group)) = checkpoint
        && (scene != expected_scene || (profile.layer_pixels > 0) != expected_group)
    {
        return Err(format!("The frame at {elapsed:?} had unexpected backdrop or opacity-layer work ({profile:?})."));
    }
    Ok(scene)
}

fn print_measurements(target: &Offscreen, hz: u32, measurements: &[Measurement], offset: f32) {
    println!("\n### {hz} Hz, {}× ({}×{} device px)\n", target.scale, target.size.0, target.size.1);
    println!("Final state verified. Copper selected, materials scrolled {offset:.1} pt, phase 0.90, Glass closed and opener focused.");
    println!("The previous slider refused input after reopening with a fresh 75% value.\n");
    println!(
        "| Scene | Frames | Update | Publish | CPU prepare | CPU encode | CPU record | CPU submit | Completion | GPU span | GPU reports / scheduled | Truncated |"
    );
    println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    print_times("All", measurements);
    for m in measurements {
        print_times(m.scene, std::slice::from_ref(m));
    }

    println!(
        "\n| Scene | UI/layer passes | Draw batches | Material batches | Layer targets (px) | Author targets (px) | Backdrop copies (px) | Retained backdrops |"
    );
    println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for m in measurements {
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            m.scene,
            range(&m.frames, |p| p.render_passes),
            range(&m.frames, |p| p.draw_batches),
            range(&m.frames, |p| p.material_batches),
            range(&m.frames, |p| p.layer_pixels),
            range(&m.frames, |p| p.canvas_pixels),
            range(&m.frames, |p| p.backdrop_copy_pixels),
            range(&m.frames, |p| p.retained_backdrops),
        );
    }
    print_passes(measurements.iter().map(|m| (m.scene, m.gpu.as_slice())));
}

fn print_times(scene: &str, measurements: &[Measurement]) {
    let frames = || measurements.iter().flat_map(|m| &m.frames);
    let gpu = || measurements.iter().flat_map(|m| &m.gpu);
    println!(
        "| {scene} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} / {} | {} |",
        frames().count(),
        percentiles(frames().map(|f| f.update)),
        percentiles(frames().map(|f| f.publish)),
        percentiles(frames().map(|f| f.draw.profile.prepare)),
        percentiles(frames().map(|f| f.draw.profile.encode)),
        percentiles(frames().map(|f| f.draw.record)),
        percentiles(frames().map(|f| f.draw.submit)),
        percentiles(frames().map(|f| f.draw.complete)),
        percentiles(gpu().filter_map(|t| t.span)),
        gpu().count(),
        frames().filter(|f| f.draw.profile.gpu_sampled).count(),
        gpu().filter(|t| t.truncated).count(),
    );
}

fn range<T: Copy + Ord + Display>(frames: &[Frame], read: impl Fn(&FrameProfile) -> T) -> String {
    let values = || frames.iter().map(|f| read(&f.draw.profile));
    match (values().min(), values().max()) {
        (Some(min), Some(max)) if min == max => min.to_string(),
        (Some(min), Some(max)) => format!("{min}–{max}"),
        _ => "—".into(),
    }
}
