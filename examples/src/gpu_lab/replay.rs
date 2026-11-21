//! One Shader Lab workload for virtual and display-driven clocks.

use super::{DEFAULT_PHASE, flow, scene, scene_with_materials_height};
use octara::app::{App, Event, InputEvent, TargetGuard, Window};
use octara::diagnostics::STALE_TARGET;
use octara::inspect::{self, Delivery, Reply};
use octara::platform::WindowOptions;
use octara::prelude::{El, Signal, current_window, on_command, spawn, state};
use octara::serde_json::{Value, json};
use octara::task::sleep_until;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

pub const SIZE: (u32, u32) = (800, 620);
pub const STRESS_SIZE: (u32, u32) = (1440, 900);
pub const DURATION: Duration = Duration::from_millis(4500);
pub const START_COMMAND: &str = "shader-lab.replay.start";
const OPEN_PEER_COMMAND: &str = "shader-lab.replay.open-peer";
const PEER_TITLE: &str = "Octara Shader Lab Peer";
const STRESS_CYCLES: u32 = 6;
const STRESS_MATERIALS_HEIGHT: f32 = 560.0;
const TRACE_SCHEMA: &str = "octara.shader_lab_replay/1";

const PRESETS: &str = "gpu-materials";
const MATERIAL: &str = "gpu-material-copper";
const OPENER: &str = "gpu-glass-open";
const DIALOG: &str = "gpu-glass-dialog";
const CLOSE: &str = "gpu-glass-cancel";
const REFRACTION: &str = "gpu-glass-refraction";
const REFRACTION_TEXT: &str = "gpu-glass-refraction-value";
const SCROLL_START: f64 = 0.15;
const SCROLL_END: f64 = 0.70;
const SCROLL_DISTANCE: f32 = 180.0;
const FINAL_PHASE: f64 = 0.90;

#[derive(Clone, Copy)]
enum Action {
    SelectMaterial,
    Open,
    AdjustRefraction,
    Close,
}

// These deadlines are shared by both refresh rates. The first close has 250 ms to
// render a fade before reopening, with the old Glass image still visible.
const ACTIONS: [(u64, Action); 6] = [
    (750, Action::SelectMaterial),
    (1000, Action::Open),
    (1500, Action::AdjustRefraction),
    (2000, Action::Close),
    (2250, Action::Open),
    (3350, Action::Close),
];

pub struct Replay {
    phase: Signal<f64>,
    next_action: usize,
    requested_scroll: f32,
    first_glass: Option<(String, TargetGuard)>,
    pending_action: Option<Action>,
}

impl Replay {
    pub fn new(app: &App) -> Self {
        let phase = app.run(|| state("shader-lab-replay-phase", || DEFAULT_PHASE));
        Self::with_phase(phase)
    }

    fn with_phase(phase: Signal<f64>) -> Self {
        Self { phase, next_action: 0, requested_scroll: 0.0, first_glass: None, pending_action: None }
    }

    /// Use with a window in the same app. Phase is app-driven while the lens is modal,
    /// so the flow changes without sending input to the inert controls behind the dialog.
    pub fn view(&self) -> impl Fn() -> El + use<> {
        let phase = self.phase;
        move || scene(phase, flow::program(false))
    }

    /// Returns to the starting state on the same window, keeping its GPU programs warm.
    pub fn restart(&mut self, window: &mut Window) -> Result<(), String> {
        if window.inspect().find(DIALOG).is_some() {
            return Err("Close the Glass dialog before restarting the Shader Lab replay.".into());
        }
        window.dispatch(Event::Press("gpu-reset".into()))?;
        let offset = scroll_offset(window)?;
        if offset != 0.0 && scroll_by(window, -offset)? != 0.0 {
            return Err("The Shader Lab replay could not reset the materials scroll offset.".into());
        }
        window.dispatch(Event::Blur)?;
        window.event(InputEvent::PointerLeave);
        self.next_action = 0;
        self.requested_scroll = 0.0;
        self.first_glass = None;
        self.pending_action = None;
        Ok(())
    }

    /// Apply the work for this elapsed time after the host advances its clock. Call once per
    /// frame, in increasing time order, through `DURATION`. Returns the flow's paint parameter.
    pub fn update(&mut self, window: &mut Window, elapsed: Duration) -> Result<f32, String> {
        self.step(window, elapsed, true)
    }

    fn step(&mut self, window: &mut Window, elapsed: Duration, verify_inline: bool) -> Result<f32, String> {
        self.verify_action(window)?;
        let elapsed = elapsed.min(DURATION);
        let phase = DEFAULT_PHASE + (FINAL_PHASE - DEFAULT_PHASE) * elapsed.as_secs_f64() / DURATION.as_secs_f64();
        window.app().run(|| self.phase.set(phase));

        let scroll_progress = ((elapsed.as_secs_f64() - SCROLL_START) / (SCROLL_END - SCROLL_START)).clamp(0.0, 1.0);
        let scroll = SCROLL_DISTANCE * scroll_progress as f32;
        if scroll > self.requested_scroll {
            scroll_by(window, scroll - self.requested_scroll)?;
            self.requested_scroll = scroll;
        }
        while let Some(&(ms, action)) = ACTIONS.get(self.next_action)
            && elapsed >= Duration::from_millis(ms)
            && self.pending_action.is_none()
        {
            match action {
                Action::SelectMaterial => window.tap(MATERIAL)?,
                Action::Open => window.tap(OPENER)?,
                Action::AdjustRefraction => window.dispatch(Event::NumericInput(REFRACTION.into(), 0.15))?,
                Action::Close => window.tap(CLOSE)?,
            }
            self.next_action += 1;
            self.pending_action = Some(action);
            if verify_inline {
                self.verify_action(window)?;
            }
        }
        Ok(phase as f32)
    }

    fn verify_action(&mut self, window: &mut Window) -> Result<(), String> {
        match self.pending_action.take() {
            Some(Action::Open) => self.verify_open_glass(window),
            Some(Action::AdjustRefraction) if window.kernel().text_of(REFRACTION_TEXT).as_deref() != Some("15%") => {
                Err("The Shader Lab replay did not apply the first Glass refraction value.".into())
            }
            Some(Action::Close) => {
                let snapshot = window.inspect();
                if snapshot.focus.as_deref() != Some(OPENER) || snapshot.find(DIALOG).is_some() {
                    return Err("Closing Glass did not retire its controls and restore opener focus.".into());
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Checks the settled scene after both Glass mounts have finished their fades.
    pub fn finish(&self, window: &Window) -> Result<f32, String> {
        self.verify_finished(window, true)
    }

    fn verify_finished(&self, window: &Window, require_app_idle: bool) -> Result<f32, String> {
        let snapshot = window.inspect();
        let offset = scroll_offset(window)?;
        if self.next_action != ACTIONS.len()
            || self.pending_action.is_some()
            || self.first_glass.is_some()
            || snapshot.focus.as_deref() != Some(OPENER)
            || snapshot.find(DIALOG).is_some()
            || snapshot.find(OPENER).and_then(|n| n.expanded) != Some(false)
            || snapshot.find(MATERIAL).is_none_or(|n| !n.states.iter().any(|s| s == "selected"))
            || window.kernel().text_of("gpu-phase-value").as_deref() != Some("0.90")
            || offset <= 0.0
            || require_app_idle && window.app().next_deadline().is_some()
        {
            return Err(
                "The Shader Lab replay did not finish with scrolled materials, Copper, phase 0.90 and focus on the Glass opener.".into()
            );
        }
        Ok(offset)
    }

    fn verify_open_glass(&mut self, window: &mut Window) -> Result<(), String> {
        let snapshot = window.inspect();
        let dialog = snapshot.find(DIALOG).ok_or("The Shader Lab Glass dialog did not open.")?;
        let slider = snapshot.find(REFRACTION).ok_or("The Shader Lab Glass slider is missing.")?;
        if snapshot.focus.as_deref() != Some(CLOSE) || slider.numeric_value != Some(0.75) {
            return Err("Opening Glass did not focus Close and initialize refraction to 75%.".into());
        }
        // Bind a request to this allocation through the same guard used by the live Inspector.
        let probe = r#"{"op":"set_value","id":"gpu-glass-refraction","value":0.7}"#;
        let bound = inspect::parse_guarded_request(window, probe).map_err(|reply| request_error(&reply))?;
        let guard = bound.guard.ok_or("The Shader Lab Glass slider request did not bind.")?;
        if let Some((old_dialog, old_slider)) = self.first_glass.take() {
            let refused = inspect::handle_guarded(window, bound.request, Some(&old_slider));
            if old_dialog == dialog.target
                || old_slider.node() == guard.node()
                || refused.ok
                || refused.code.as_deref() != Some(STALE_TARGET.id)
                || refused.delivery != Some(Delivery::Refused)
                || window.kernel().text_of(REFRACTION_TEXT).as_deref() != Some("75%")
            {
                return Err("The reopened Glass did not keep its new state when refusing the retired slider target.".into());
            }
        } else {
            self.first_glass = Some((dialog.target.clone(), guard));
        }
        Ok(())
    }
}

/// Launches idle and runs the same replay on the App clock when Inspector invokes
/// `shader-lab.replay.start`. The shell controls actual presentation opportunities.
pub fn live_view(hz: u32) -> El {
    live_replay(hz, 1, None)
}

/// Repeats the live replay at 120 Hz. The materials column stays scrollable in a tall window.
pub fn stress_view() -> El {
    let peer_id = state("shader-lab-replay-peer", || None);
    on_command(OPEN_PEER_COMMAND, move || {
        let app = App::current();
        if peer_id.get().is_some_and(|id| app.window(id).is_some()) {
            return;
        }
        let options = WindowOptions::new(PEER_TITLE).size(SIZE.0 as f32, SIZE.1 as f32);
        match app.open_window(options, sustained_view) {
            Ok(peer) => peer_id.set(Some(peer.id())),
            Err(error) => eprintln!("Could not open the Shader Lab peer window. {error}"),
        }
    });
    sustained_view()
}

fn sustained_view() -> El {
    live_replay(120, STRESS_CYCLES, Some(STRESS_MATERIALS_HEIGHT))
}

fn live_replay(hz: u32, cycles: u32, materials_max_height: Option<f32>) -> El {
    let phase = state("shader-lab-replay-phase", || DEFAULT_PHASE);
    let id = current_window();
    let running = Rc::new(Cell::new(false));
    on_command(START_COMMAND, move || {
        if running.replace(true) {
            return;
        }
        let running = running.clone();
        spawn(async move {
            let _running = std::mem::DropGuard::new(running, |running| running.set(false));
            let app = App::current();
            let Some(mut window) = app.window(id) else { return };
            let result = run_live(&app, &mut window, Replay::with_phase(phase), hz, cycles).await;
            let (event, offset, error) = match result {
                Ok(offset) => ("finish", Some(offset), None),
                Err(error) => ("error", None, Some(error)),
            };
            log_replay(
                &window,
                cycles,
                json!({
                    "schema": TRACE_SCHEMA,
                    "event": event,
                    "app_clock_ms": app.now().as_millis(),
                    "hz": hz,
                    "scroll": offset,
                    "error": error,
                }),
            );
        });
    });
    scene_with_materials_height(phase, flow::program(false), materials_max_height)
}

async fn run_live(app: &App, window: &mut Window, mut replay: Replay, hz: u32, cycles: u32) -> Result<f32, String> {
    replay.restart(window)?;
    let origin = app.now();
    let frames = (DURATION.as_millis() as u32 * hz) / 1_000;
    let sample = |frame| Duration::from_secs_f64(f64::from(frame) / f64::from(hz));
    log_replay(
        window,
        cycles,
        json!({
            "schema": TRACE_SCHEMA,
            "event": "start",
            "app_clock_ms": origin.as_millis(),
            "duration_ms": (DURATION * cycles).as_millis(),
            "hz": hz,
        }),
    );
    let mut scroll = 0.0;
    for cycle in 0..cycles {
        let start = origin + DURATION * cycle;
        for frame in 1..=frames {
            let elapsed = sample(frame);
            sleep_until(start + elapsed).await;
            // The enclosing App update prepares actions after this task yields. Verify the last
            // action at the start of the next step, while retaining the exact action deadlines.
            replay.step(window, elapsed, false).map_err(|error| format!("At {elapsed:?}, {error}"))?;
        }
        sleep_until(start + sample(frames + 1)).await;
        replay.verify_action(window)?;
        scroll = replay.verify_finished(window, cycles == 1)?;
        if cycles > 1 {
            log_replay(
                window,
                cycles,
                json!({
                    "schema": TRACE_SCHEMA,
                    "event": "checkpoint",
                    "app_clock_ms": app.now().as_millis(),
                    "cycle": cycle + 1,
                    "viewport": window.size(),
                    "scroll": scroll,
                }),
            );
        }
        if cycle + 1 < cycles {
            replay.restart(window)?;
        }
    }
    Ok(scroll)
}

fn log_replay(window: &Window, cycles: u32, mut record: Value) {
    if cycles > 1 {
        record["window"] = json!(window.id().number());
    }
    eprintln!("{record}");
}

fn scroll_offset(window: &Window) -> Result<f32, String> {
    let kernel = window.kernel();
    let presets = kernel.find(PRESETS).ok_or("The Shader Lab materials viewport is missing.")?;
    Ok(kernel.scroll_offset(presets).1)
}

fn scroll_by(window: &mut Window, dy: f32) -> Result<f32, String> {
    window.dispatch(Event::Scroll(PRESETS.into(), 0.0, dy))?;
    scroll_offset(window)
}

fn request_error(reply: &Reply) -> String {
    format!("Shader Lab replay input failed ({}) {}", reply.code.as_deref().unwrap_or("no code"), reply.message.as_deref().unwrap_or(""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara::platform::HeadlessPlatform;
    use octara_primitives::paint::Item;

    #[test]
    fn both_cadences_replay_the_same_glass_lifecycle() {
        let options = crate::gpu_lab::options();
        let app = App::with_config(HeadlessPlatform::default(), options.app);
        let mut replay = Replay::new(&app);
        let mut window = app.open_window(options.window.size(SIZE.0 as f32, SIZE.1 as f32), replay.view()).unwrap();
        let mut offsets = Vec::new();
        for hz in [60, 120] {
            replay.restart(&mut window).unwrap();
            let origin = app.now();
            let mut revision = window.published_revision().unwrap();
            for frame in 1..=(DURATION.as_millis() as u32 * hz / 1_000) {
                let elapsed = Duration::from_secs_f64(f64::from(frame) / f64::from(hz));
                app.advance_to(origin + elapsed);
                replay.update(&mut window, elapsed).unwrap_or_else(|error| panic!("{hz} Hz at {elapsed:?}: {error}"));
                let published = window.published_frame().unwrap();
                assert!(published.revision > revision);
                revision = published.revision;
                if elapsed == Duration::from_millis(2500) {
                    let live = window.kernel().find(DIALOG).unwrap();
                    let items = published.list.items();
                    assert!(items.iter().any(|item| matches!(item, Item::Custom(paint) if paint.exiting)));
                    assert!(
                        items.iter().any(|item| matches!(item, Item::Custom(paint) if paint.fallback.source == live && !paint.exiting))
                    );
                }
            }
            offsets.push(replay.finish(&window).unwrap());
        }
        assert_eq!(offsets[0], offsets[1]);
        app.shutdown();
    }

    #[test]
    fn the_live_command_runs_at_both_app_clock_cadences() {
        for hz in [60, 120] {
            let options = crate::gpu_lab::options();
            let app = App::with_config(HeadlessPlatform::default(), options.app);
            let mut window = app.open_window(options.window.size(SIZE.0 as f32, SIZE.1 as f32), move || live_view(hz)).unwrap();
            assert_eq!(window.kernel().text_of("gpu-phase-value").as_deref(), Some("0.38"));
            window.invoke(START_COMMAND).unwrap();
            app.poll();
            let origin = app.now();
            app.advance_to(origin + Duration::from_millis(2500));
            let opened = window.inspect();
            assert_eq!(opened.focus.as_deref(), Some(CLOSE));
            assert_eq!(opened.find(REFRACTION).unwrap().numeric_value, Some(0.75));
            assert!(window.published_frame().unwrap().list.items().iter().any(|item| matches!(item, Item::Custom(paint) if paint.exiting)));

            app.advance_to(origin + DURATION + Duration::from_millis(20));
            let finished = window.inspect();
            assert_eq!(finished.focus.as_deref(), Some(OPENER));
            assert!(finished.find(DIALOG).is_none());
            assert_eq!(window.kernel().text_of("gpu-phase-value").as_deref(), Some("0.90"));
            assert!(finished.find(MATERIAL).unwrap().states.iter().any(|state| state == "selected"));
            assert!(scroll_offset(&window).unwrap() > 0.0);
            assert!(app.pending().is_empty(), "{:?}", app.pending());
            assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
            app.shutdown();
        }
    }

    #[test]
    fn sustained_replays_survive_resizes_and_settle_both_windows() {
        let options = crate::gpu_lab::options();
        let app = App::with_config(HeadlessPlatform::default(), options.app);
        let mut window = app.open_window(options.window.size(STRESS_SIZE.0 as f32, STRESS_SIZE.1 as f32), stress_view).unwrap();
        window.invoke(OPEN_PEER_COMMAND).unwrap();
        window.invoke(OPEN_PEER_COMMAND).unwrap();
        let windows = app.windows();
        assert_eq!(windows.len(), 2);
        let mut peer = app.window(windows.into_iter().find(|&id| id != window.id()).unwrap()).unwrap();
        assert_eq!(peer.options().title, PEER_TITLE);
        assert_eq!(peer.size(), (SIZE.0 as f32, SIZE.1 as f32));
        assert!(!peer.command_enabled(OPEN_PEER_COMMAND));

        window.invoke(START_COMMAND).unwrap();
        app.poll();
        let origin = app.now();
        let peer_delay = Duration::from_millis(400);
        app.advance_to(origin + peer_delay);
        assert!(scroll_offset(&window).unwrap() > 0.0);
        peer.invoke(START_COMMAND).unwrap();
        app.poll();

        for cycle in 0..STRESS_CYCLES {
            let start = origin + DURATION * cycle;
            app.advance_to(start + Duration::from_millis(2_150));
            let size = [(1320.0, 820.0), (1440.0, 900.0)][(cycle % 2) as usize];
            window.resize(size.0, size.1);
            app.advance_to(start + Duration::from_millis(2_550));
            let reopened = window.inspect();
            assert_eq!(reopened.focus.as_deref(), Some(CLOSE), "cycle {}", cycle + 1);
            assert_eq!(reopened.find(REFRACTION).unwrap().numeric_value, Some(0.75));
            app.advance_to(start + DURATION + Duration::from_millis(20));
        }

        app.advance_to(origin + DURATION * STRESS_CYCLES + peer_delay + Duration::from_millis(20));
        for finished in [window, peer] {
            let snapshot = finished.inspect();
            assert_eq!(snapshot.focus.as_deref(), Some(OPENER));
            assert!(snapshot.find(DIALOG).is_none());
            assert!(snapshot.find(MATERIAL).unwrap().states.iter().any(|state| state == "selected"));
            assert!(scroll_offset(&finished).unwrap() > 0.0);
        }
        assert!(app.pending().is_empty(), "{:?}", app.pending());
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }
}
