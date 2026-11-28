#![cfg(not(target_arch = "wasm32"))]

use octara_gpu::{CaptureWriter, DeviceExecutor, DeviceGeneration, GpuError};
use octara_model::{
    geometry::{Rect, Size},
    paint::{DisplayList, Item, Part, Quad},
    theme::rgb,
};
use octara_render::{CpuRenderer, WgpuRenderer};
use std::fmt::Debug;
use std::fs::{self, File};
use std::io::BufReader;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

fn wait<T, E: Debug>(wakes: &mpsc::Receiver<()>, mut take: impl FnMut() -> Option<Result<T, E>>) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = take() {
            return result.unwrap();
        }
        wakes.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("the worker should notify its caller");
    }
}

#[test]
fn gpu_capture_stages_a_cpu_equivalent_png_until_publication() {
    let Some(renderer) = WgpuRenderer::new() else {
        eprintln!("No GPU adapter available for capture parity");
        return;
    };
    let list = Arc::new(DisplayList {
        size: Size::new(7.0, 4.0),
        background: rgb(0x0000ff),
        parts: vec![Part::Item(Item::Quad(Quad {
            source: Default::default(),
            rect: Rect::new(2.0, 1.0, 3.0, 2.0),
            radius: [0.0; 4],
            fill: Some(rgb(0x00ff00)),
            gradient: None,
            border: [0.0; 4],
            border_color: rgb(0),
        }))],
    });
    let cpu = CpuRenderer::new().render(&list, 1.0);
    assert_eq!(cpu.pixel(0, 0), Some([0, 0, 255, 255]));
    assert_eq!(cpu.pixel(3, 2), Some([0, 255, 0, 255]));

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("capture.png");
    fs::write(&path, b"previous capture").unwrap();
    let writer = CaptureWriter::spawn().unwrap();
    let (wake, wakes) = mpsc::channel();
    let device_wake = wake.clone();
    let generation = Arc::new(DeviceGeneration::new(
        1,
        Some(Arc::new(move |_, _| {
            let _ = device_wake.send(());
        })),
    ));
    let executor = DeviceExecutor::spawn(renderer, generation.clone()).unwrap();
    let mut capture = executor.capture(list.clone(), 1.0, None).unwrap();
    let mapped = wait(&wakes, || capture.try_take());
    assert!(matches!(executor.capture(list, 1.0, None), Err(GpuError::Busy)));

    let stage_wake = wake.clone();
    let mut stage = writer.stage(
        &path,
        move || mapped.finish(),
        move || {
            let _ = stage_wake.send(());
        },
    );
    let staged = wait(&wakes, || stage.try_take());
    assert_eq!(staged.size, [7, 4]);
    assert_eq!(fs::read(&path).unwrap(), b"previous capture");
    wait(&wakes, || generation.capacity().readback_available.then_some(Ok::<(), ()>(())));
    executor.shutdown();

    let mut publish = staged.publish(move || {
        let _ = wake.send(());
    });
    assert_eq!(wait(&wakes, || publish.try_take()), path);
    writer.shutdown();
    let mut png = png::Decoder::new(BufReader::new(File::open(&path).unwrap())).read_info().unwrap();
    let mut bytes = vec![0; png.output_buffer_size().unwrap()];
    let frame = png.next_frame(&mut bytes).unwrap();
    assert_eq!((frame.width, frame.height), (7, 4));
    assert_eq!(&bytes[..frame.buffer_size()], cpu.rgba());
    assert_eq!(temp.path().read_dir().unwrap().count(), 1);
}
