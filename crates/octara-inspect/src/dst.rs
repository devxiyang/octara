//! Seeded simulation over independent retained trees and one virtual application clock.

use crate::{HeadlessDriver, inspector::snapshot, protocol::TargetRef};
use octara::{
    prelude::*,
    runtime::{HostCommand, Input, cycle::AppError},
    tree::{editor::EditorSession, overlay::LayerPolicy},
};
use octara_model::{
    ask::{Answer, Ask, AskId, PromptLevel},
    input::{ContactTarget, EditorEvent, HostReply, InteractionEvent, PointerId, PointerKind},
    semantics::A11yAction,
    style::Overflow,
};
use octara_pipeline::NativeEngineConfig;
use std::{
    cell::Cell,
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    time::Duration,
};

#[derive(Clone, Copy, Debug)]
pub struct SimConfig {
    pub seed: u64,
    pub steps: usize,
    pub windows: usize,
    pub inject_resizes: bool,
    pub inject_ime_interleavings: bool,
    pub inject_list_reorders: bool,
    pub inject_async_delays: bool,
    pub inject_focus_losses: bool,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            seed: 0,
            steps: 128,
            windows: 2,
            inject_resizes: true,
            inject_ime_interleavings: true,
            inject_list_reorders: true,
            inject_async_delays: true,
            inject_focus_losses: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimReport {
    pub steps: usize,
    /// Logical final observations in window order for exact comparison across reruns.
    pub state: Vec<String>,
}

/// Reproduce the failure using this configuration and its first failing prefix.
#[derive(Debug)]
pub struct SimFailure {
    pub config: SimConfig,
    pub trace: Vec<String>,
    pub reason: String,
}

impl fmt::Display for SimFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "seed {} failed: {} ({:?})", self.config.seed, self.reason, self.config)?;
        for (index, event) in self.trace.iter().enumerate() {
            writeln!(f, "{index}: {event}")?;
        }
        Ok(())
    }
}

impl std::error::Error for SimFailure {}

pub struct SimHarness;

impl SimHarness {
    pub fn run(config: SimConfig) -> Result<SimReport, SimFailure> {
        let mut trace = Vec::with_capacity(config.steps);
        let result = catch_unwind(AssertUnwindSafe(|| run(config, &mut trace)));
        let reason = match result {
            Ok(Ok(report)) => return Ok(report),
            Ok(Err(error)) => error,
            Err(panic) => panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|message| (*message).to_owned()))
                .unwrap_or_else(|| "simulation panicked".into()),
        };
        Err(SimFailure { config, trace, reason })
    }

    /// Run independently constructed apps in parallel, with results in configuration order.
    pub fn run_parallel(configs: &[SimConfig]) -> Vec<Result<SimReport, SimFailure>> {
        let workers = std::thread::available_parallelism().map_or(1, usize::from);
        let chunk = configs.len().div_ceil(workers).max(1);
        std::thread::scope(|scope| {
            let workers: Vec<_> = configs
                .chunks(chunk)
                .map(|configs| scope.spawn(move || configs.iter().copied().map(Self::run).collect::<Vec<_>>()))
                .collect();
            workers.into_iter().flat_map(|worker| worker.join().expect("simulation worker")).collect()
        })
    }
}

#[derive(Clone, Copy)]
struct Fixture {
    rows: StateList<u64, String>,
    modal: Signal<bool>,
}

struct SimWindow {
    id: WindowId,
    fixture: Fixture,
    pointer: Option<Point>,
}

fn open(driver: &mut HeadlessDriver) -> Result<SimWindow, AppError> {
    let capture = Rc::new(Cell::new(None));
    let result = capture.clone();
    let options = WindowOptions {
        touch: true,
        viewport: WindowViewport { size: Size::new(240.0, 180.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 },
        ..Default::default()
    };
    let id = driver.app_mut().open_window(options, move |cx| {
        let rows = cx.list::<u64, String>();
        let modal = cx.signal(false);
        let answers = cx.signal(0_u32);
        result.set(Some(Fixture { rows, modal }));
        cx.spawn("initial rows", move |task| async move {
            task.write(|w| {
                for key in 0..16 {
                    w.push_row(rows, key, format!("file-{key}"));
                }
            });
        });
        col()
            .child(el().h(24).test_id("launcher").focusable().on_press(move |w| w.set(modal, true)).portal(
                move |r| r.get(modal),
                move |_| {
                    col()
                        .test_id("modal")
                        .w(100)
                        .h(60)
                        .layer(LayerPolicy::modal(), move |w| w.set(modal, false))
                        .child(el().focusable().autofocus().test_id("modal-focus").h(30))
                },
            ))
            .child(el().h(20).test_id("ask").on_press(move |w| {
                w.ask(
                    Ask::Prompt {
                        level: PromptLevel::Info,
                        message: "Confirm simulation action".into(),
                        detail: None,
                        answers: vec!["OK".into()],
                    },
                    move |w, answer| {
                        if answer == Answer::Prompt(Some(0)) {
                            w.update(answers, |value| *value += 1);
                        }
                    },
                );
            }))
            .child(text(bind(move |r| format!("answers: {}", r.get(answers)))).h(20))
            .child(col().h(90).overflow_y(Overflow::Scroll).test_id("rows").child(virtual_each(
                rows,
                VirtualOptions { estimated_row_height: 24.0, overscan: 1, ..Default::default() },
                move |_, key, item| {
                    let key = *key;
                    col().h(24).test_id(format!("row-{key}")).child(
                        input()
                            .value(bind(move |r| r.try_get(item).unwrap_or_default()))
                            .on_input(move |w, value| w.update_row(rows, &key, |name| *name = value.into()))
                            .h(24)
                            .test_id(format!("editor-{key}")),
                    )
                },
            )))
    })?;
    Ok(SimWindow { id, fixture: capture.get().expect("window built during open"), pointer: None })
}

#[derive(Clone, Copy, Debug)]
enum Action {
    Resize(u16, u16),
    Focus(u64),
    Compose,
    Commit,
    MoveRow { key: u64, rank: usize },
    RemoveRow(u64),
    InsertRow(u64),
    Modal,
    Escape,
    Outside,
    Reveal(u64),
    PointerDown,
    PointerMove(i16),
    PointerUp,
    PointerCancel,
    Active(bool),
    Ask,
    Advance(u64),
    Reopen,
}

fn run(config: SimConfig, trace: &mut Vec<String>) -> Result<SimReport, String> {
    let mut rng = Rng(config.seed);
    let mut driver = HeadlessDriver::with_namespace(
        [0x4473_7400, config.seed],
        NativeEngineConfig { check_paint_cache: true, check_a11y_cache: true, ..NativeEngineConfig::default() },
    );
    let app_nodes = driver.app().runtime().world().live_nodes();
    let mut windows = (0..config.windows).map(|_| open(&mut driver)).collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())?;
    let mut delayed = Vec::<(Duration, WindowId, AskId)>::new();
    validate(&driver)?;
    for _ in 0..config.steps {
        if windows.is_empty() {
            break;
        }
        let index = rng.below(windows.len() as u64) as usize;
        let action = generate(config, &mut rng);
        trace.push(format!("window {index}, {action:?}"));
        apply(&mut driver, &mut windows[index], action).map_err(|error| error.to_string())?;
        validate(&driver)?;
        for command in driver.take_host_commands() {
            if let HostCommand::Ask { window, id, .. } = command {
                let delay = Duration::from_millis(1 + rng.below(50));
                delayed.push((driver.app().runtime().now() + delay, window, id));
            }
        }
        let now = driver.app().runtime().now();
        for (due, id, ask) in delayed.extract_if(.., |(due, _, _)| *due <= now) {
            if driver.app().runtime().window(id).is_some() {
                trace.push(format!("host reply {ask:?} to {id:?} due at {due:?}"));
                driver
                    .dispatch(id, Input::Host(HostReply::AskAnswer { id: ask, answer: Answer::Prompt(Some(0)) }))
                    .map_err(|error| error.to_string())?;
                validate(&driver)?;
            }
        }
    }

    let state = driver
        .app()
        .runtime()
        .windows()
        .map(|(_, window)| serde_json::to_string(&snapshot(window, false)).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    for window in &windows {
        trace.push(format!("close {:?}", window.id));
        driver.app_mut().close_window(window.id).map_err(|error| error.to_string())?;
        validate(&driver)?;
    }
    let world = driver.app().runtime().world();
    if world.live_scopes() != 1 || world.live_nodes() != app_nodes {
        return Err("closing all windows left live window scopes or reactive nodes".into());
    }
    driver.app_mut().shutdown();
    let world = driver.app().runtime().world();
    if world.live_scopes() != 0 || world.live_nodes() != 0 {
        return Err("shutdown left live scopes or reactive nodes".into());
    }
    Ok(SimReport { steps: if windows.is_empty() { 0 } else { config.steps }, state })
}

fn validate(driver: &HeadlessDriver) -> Result<(), String> {
    let app = driver.app().runtime();
    if !app.world().validate_integrity() {
        return Err("reactive graph references a dead allocation".into());
    }
    for (id, window) in app.windows() {
        window.tree().validate_integrity().map_err(|error| format!("{id:?}: {error}"))?;
        if !window.validate_handler_integrity(app.world()) {
            return Err(format!("{id:?}: a live tree handler has a missing or orphaned closure"));
        }
    }
    Ok(())
}

fn generate(config: SimConfig, rng: &mut Rng) -> Action {
    loop {
        let key = rng.below(16);
        let action = match rng.below(19) {
            0 if config.inject_resizes => Action::Resize(140 + rng.below(180) as u16, 110 + rng.below(130) as u16),
            1 if config.inject_ime_interleavings => Action::Focus(key),
            2 if config.inject_ime_interleavings => Action::Compose,
            3 if config.inject_ime_interleavings => Action::Commit,
            4 if config.inject_list_reorders => Action::MoveRow { key, rank: rng.below(16) as usize },
            5 if config.inject_list_reorders => Action::RemoveRow(key),
            6 if config.inject_list_reorders => Action::InsertRow(key),
            7 => Action::Modal,
            8 => Action::Escape,
            9 => Action::Outside,
            10 => Action::Reveal(key),
            11 => Action::PointerDown,
            12 => Action::PointerMove(rng.below(160) as i16 - 80),
            13 => Action::PointerUp,
            14 => Action::PointerCancel,
            15 if config.inject_focus_losses => Action::Active(rng.below(2) == 0),
            16 if config.inject_async_delays => Action::Ask,
            17 => Action::Advance(1 + rng.below(40)),
            18 => Action::Reopen,
            _ => continue,
        };
        return action;
    }
}

fn apply(driver: &mut HeadlessDriver, window: &mut SimWindow, action: Action) -> Result<(), AppError> {
    let id = window.id;
    let rows = window.fixture.rows;
    let state = driver.app().runtime().window(id).expect("simulation window");
    match action {
        Action::Resize(width, height) => {
            let size = Size::new(f32::from(width), f32::from(height));
            let viewport = WindowViewport { size, ..state.options().viewport };
            driver.dispatch(id, Input::Host(HostReply::ViewportChanged(viewport)))?;
        }
        Action::Focus(key) => {
            driver.app_mut().update_window(id, |w| w.reveal_row("rows", key))?;
            driver.app_mut().update_window(id, |w| w.focus(&format!("editor-{key}")))?;
        }
        Action::Compose | Action::Commit => {
            if let Some(session) = EditorSession::active(state.tree()) {
                let expected_rev = session.editor().document_rev();
                let session = session.id();
                let event = match action {
                    Action::Compose => {
                        EditorEvent::Composition {
                            session, expected_rev, text: "文件".into(), selection: Some((6, 3)), replacement: None
                        }
                    }
                    _ => EditorEvent::Commit { session, expected_rev, text: "文書".into() },
                };
                driver.dispatch(id, Input::Editor(event))?;
            }
        }
        Action::MoveRow { key, rank } => driver.app_mut().update_window(id, |w| w.move_row(rows, &key, rank))?,
        Action::RemoveRow(key) => driver.app_mut().update_window(id, |w| {
            w.remove_row(rows, &key);
        })?,
        Action::InsertRow(key) => driver.app_mut().update_window(id, |w| {
            if w.get_row(rows, &key).is_none() {
                w.push_row(rows, key, format!("file-{key}"));
            }
        })?,
        Action::Modal => driver.app_mut().update_window(id, |w| {
            let previous = w.get(window.fixture.modal);
            w.set(window.fixture.modal, !previous);
        })?,
        Action::Escape => {
            driver.dispatch(id, Input::Interaction(InteractionEvent::Key(KeyEvent::new(Key::Escape, Modifiers::empty()))))?;
        }
        Action::Outside | Action::PointerDown => {
            let outside = matches!(action, Action::Outside);
            let pos = if outside { Point::new(210.0, 10.0) } else { Point::new(35.0, 80.0) };
            let target = state.hit_test(pos).unwrap_or(ContactTarget { hit: TargetRef::for_window(state.tree().id()), action: None });
            let event = InteractionEvent::PointerDown {
                id: PointerId(1),
                kind: PointerKind::Touch,
                target,
                pos,
                button: Button::Primary,
                mods: Modifiers::empty(),
            };
            driver.dispatch(id, Input::Interaction(event))?;
            window.pointer = Some(pos);
            if outside {
                apply(driver, window, Action::PointerUp)?;
            }
        }
        Action::PointerMove(delta) => {
            let pos = window.pointer.unwrap_or(Point::new(35.0, 80.0));
            let pos = Point::new(pos.x, pos.y + f32::from(delta));
            driver.dispatch(id, Input::Interaction(InteractionEvent::PointerMove { id: PointerId(1), pos, mods: Modifiers::empty() }))?;
            window.pointer = Some(pos);
        }
        Action::PointerUp => {
            let pos = window.pointer.take().unwrap_or(Point::new(35.0, 80.0));
            let target = state.hit_test(pos);
            driver.dispatch(
                id,
                Input::Interaction(InteractionEvent::PointerUp { id: PointerId(1), target, pos, mods: Modifiers::empty() }),
            )?;
        }
        Action::PointerCancel => {
            driver.dispatch(id, Input::Interaction(InteractionEvent::PointerCancel { id: PointerId(1) }))?;
            window.pointer = None;
        }
        Action::Reveal(key) => driver.app_mut().update_window(id, |w| w.reveal_row("rows", key))?,
        Action::Active(active) => {
            driver.dispatch(id, Input::Interaction(InteractionEvent::WindowActive(active)))?;
        }
        Action::Ask => {
            let node = state.tree().test_ids("ask")[0];
            let target = state.tree().guard(node);
            driver.dispatch(id, Input::Host(HostReply::AccessibilityAction { target, action: A11yAction::Press }))?;
        }
        Action::Advance(ms) => {
            let next = driver.app().runtime().now() + Duration::from_millis(ms);
            driver.advance_to(next)?;
        }
        Action::Reopen => {
            driver.app_mut().close_window(id)?;
            *window = open(driver)?;
        }
    }
    Ok(())
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, max: u64) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        (value ^ (value >> 31)) % max
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_multi_window_runs_are_deterministic_in_parallel() {
        let configs: Vec<_> = (0..std::thread::available_parallelism().map_or(2, usize::from).max(2))
            .map(|seed| SimConfig { seed: seed as u64, steps: 60, ..Default::default() })
            .collect();
        let first = SimHarness::run_parallel(&configs);
        let second = SimHarness::run_parallel(&configs);
        for (first, second) in first.into_iter().zip(second) {
            let first = first.unwrap_or_else(|error| panic!("{error}"));
            let second = second.unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(first, second);
        }
    }
}
