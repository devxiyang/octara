//! Shader Lab pacing driven by the application clock.

use super::{DEFAULT_PHASE, flow, scene_with_materials_height};
use octara::prelude::*;
use std::time::Duration;

pub const SIZE: (u32, u32) = (800, 620);
pub const STRESS_SIZE: (u32, u32) = (1440, 900);
pub const DURATION: Duration = Duration::from_millis(4500);
pub const START_COMMAND: &str = "shader-lab.replay.start";

pub fn live_view(cx: &mut BuildCx<'_>, hz: u32) -> El {
    replay(cx, hz, false)
}

pub fn stress_view(cx: &mut BuildCx<'_>, hz: u32) -> El {
    replay(cx, hz, true)
}

fn replay(cx: &mut BuildCx<'_>, hz: u32, stress: bool) -> El {
    let phase = cx.signal(DEFAULT_PHASE);
    let running = cx.signal(false);
    let start = move |w: &mut WriteCx<'_>| {
        if w.get(running) {
            return;
        }
        w.set(running, true);
        w.spawn("Shader Lab replay", move |task| async move {
            let start = task.now();
            let frames = (DURATION.as_secs_f64() * f64::from(hz)) as u32;
            let cycles = if stress { 6 } else { 1 };
            for frame in 1..=(frames * cycles) {
                task.sleep_until(start + Duration::from_secs_f64(f64::from(frame) / f64::from(hz))).await;
                let progress = f64::from((frame - 1) % frames + 1) / f64::from(frames);
                task.write(|w| w.set(phase, DEFAULT_PHASE + (0.90 - DEFAULT_PHASE) * progress));
            }
            task.write(|w| w.set(running, false));
        });
    };
    cx.spawn("start Shader Lab replay", move |task| async move {
        task.write(start);
    });
    scene_with_materials_height(cx, phase, flow::program(stress), stress.then_some(560.0)).on_command(START_COMMAND, start)
}
