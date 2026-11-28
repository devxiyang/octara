//! Render a small card with the available renderers.
//! `cargo run -p octara-gpu --example card_png -- <dir>`

#[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
use octara_gpu::DeviceExecutor;
use octara_gpu::{CaptureWriter, DeviceGeneration, StagedCapture};
#[cfg(all(not(feature = "wgpu"), any(target_os = "macos", target_os = "ios")))]
use octara_gpu::{GpuDevice, MetalGpuDevice};
use octara_model::geometry::{Rect, Size};
use octara_model::paint::{DisplayList, Item, Part, Quad};
use octara_model::theme::rgb;
use octara_render::CpuRenderer;
#[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
use octara_render::WgpuRenderer;
use std::error::Error;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn Error>> {
    let dir = std::env::args_os().nth(1).map_or_else(|| PathBuf::from("."), PathBuf::from);
    let card = Arc::new(card());
    let writer = CaptureWriter::spawn()?;
    let (wake, wakes) = mpsc::channel();
    let cpu_card = card.clone();
    let cpu_wake = wake.clone();
    let mut cpu = writer.stage(
        dir.join("card_cpu.png"),
        move || CpuRenderer::new().paint_with_receipts(&cpu_card, 2.0, None),
        move || {
            let _ = cpu_wake.send(());
        },
    );
    publish(wait(&wakes, || cpu.try_take())?, &wake, &wakes)?;

    let device_wake = wake.clone();
    let generation = Arc::new(DeviceGeneration::new(
        0,
        Some(Arc::new(move |_, _| {
            let _ = device_wake.send(());
        })),
    ));
    #[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
    let executor = WgpuRenderer::new().map(|renderer| DeviceExecutor::spawn(renderer, generation)).transpose()?;
    #[cfg(all(not(feature = "wgpu"), any(target_os = "macos", target_os = "ios")))]
    let executor = Some(MetalGpuDevice::new(generation, false, || {})?);
    if let Some(executor) = executor {
        let mut capture = executor.capture(card, 2.0, None)?;
        let pixels = wait(&wakes, || capture.try_take())?;
        let gpu_wake = wake.clone();
        let mut gpu = writer.stage(
            dir.join("card_gpu.png"),
            move || pixels.finish(),
            move || {
                let _ = gpu_wake.send(());
            },
        );
        publish(wait(&wakes, || gpu.try_take())?, &wake, &wakes)?;
        executor.shutdown();
    }
    writer.shutdown();
    Ok(())
}

fn publish(capture: StagedCapture, wake: &mpsc::Sender<()>, wakes: &mpsc::Receiver<()>) -> Result<(), Box<dyn Error>> {
    let wake = wake.clone();
    let mut output = capture.publish(move || {
        let _ = wake.send(());
    });
    let path = wait(wakes, || output.try_take())?;
    println!("{}", path.display());
    Ok(())
}

fn wait<T, E>(wakes: &mpsc::Receiver<()>, mut take: impl FnMut() -> Option<Result<T, E>>) -> Result<T, Box<dyn Error>>
where
    Box<dyn Error>: From<E>,
{
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = take() {
            return result.map_err(Into::into);
        }
        wakes.recv_timeout(deadline.saturating_duration_since(Instant::now()))?;
    }
}

fn card() -> DisplayList {
    let quad = |rect, radius, fill, border| {
        Part::Item(Item::Quad(Quad {
            source: Default::default(),
            rect,
            radius: [radius; 4],
            fill: Some(rgb(fill)),
            gradient: None,
            border: [border; 4],
            border_color: rgb(0x435975),
        }))
    };
    DisplayList {
        size: Size::new(300.0, 140.0),
        background: rgb(0x202a3c),
        parts: vec![
            quad(Rect::new(20.0, 20.0, 260.0, 100.0), 12.0, 0x293951, 1.0),
            quad(Rect::new(36.0, 38.0, 38.0, 38.0), 19.0, 0x7fd8cf, 0.0),
            quad(Rect::new(90.0, 43.0, 112.0, 12.0), 6.0, 0xf1f4f9, 0.0),
            quad(Rect::new(90.0, 65.0, 158.0, 8.0), 4.0, 0xa6b5cb, 0.0),
            quad(Rect::new(36.0, 92.0, 228.0, 9.0), 4.5, 0x435975, 0.0),
            quad(Rect::new(36.0, 92.0, 145.0, 9.0), 4.5, 0x7fd8cf, 0.0),
        ],
    }
}
