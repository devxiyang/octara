//! CPU/work scaling of the real Chat UI, not IPC latency or physical frame time.
//!
//! Run the release example with `--samples 40 --output /tmp/long-bench.json`.
//! The shared long-session fixture supplies the product Store, UI and Model channel.
//! Seed generation is outside the measurement. Initial load includes Store::load, JSON,
//! reactive data and mounting; it may scale with the complete history. Other samples time
//! the operation on the UI thread and display-list preparation, with layout enabled.
//! Streaming explicitly lands paced text after accepting each chunk, so the measurement
//! includes its complete visible update without pretending virtual time is wall time.

#[path = "support/long_session.rs"]
mod long_session;

use long_session::LongSession;
use octara::app::{Event, Window};
use octara::kernel::Counters;
use octara::platform::{HeadlessPlatform, WindowOptions};
use octara::prelude::App;
use octara_chat::store::Store;
use serde::Serialize;
use serde_json::{Value, json};
use std::error::Error;
use std::hint::black_box;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const SIZE: (f32, f32) = (1000.0, 720.0);
const NARROW: f32 = 820.0; // Same desktop UI branch, with different Markdown wrapping.
const HISTORY: [usize; 2] = [1_000, 10_000];
const LONG_ANSWER_CHUNKS: usize = 1_500;

struct Options {
    samples: usize,
    warmup: usize,
    output: Option<PathBuf>,
}

impl Options {
    fn read() -> Result<Self> {
        let mut options = Self { samples: 40, warmup: 4, output: None };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
            match arg.as_str() {
                "--samples" => options.samples = value.parse()?,
                "--warmup" => options.warmup = value.parse()?,
                "--output" => options.output = Some(value.into()),
                _ => return Err(format!("unknown option {arg}").into()),
            }
        }
        if !(10..=200).contains(&options.samples) || options.warmup > 20 {
            return Err("use 10..=200 samples and at most 20 warmup operations".into());
        }
        if cfg!(debug_assertions) {
            return Err("run long_bench in --release; debug reference checks change the workload".into());
        }
        Ok(options)
    }
}

#[derive(Default)]
struct Before {
    kernel: Counters,
    closures: u64,
    frames: u64,
    drawings: (u64, u64, u64),
}

impl Before {
    fn read(window: &Window) -> Self {
        let kernel = window.kernel();
        Self {
            kernel: kernel.counters(),
            closures: window.app().work().closures_run,
            frames: kernel.frames_built(),
            drawings: kernel.drawings(),
        }
    }
}

#[derive(Serialize)]
struct Work {
    closures: u64,
    nodes_created: u64,
    nodes_destroyed: u64,
    prop_changes: u64,
    structural_ops: u64,
    layout_runs: u64,
    text_bytes: u64,
    frames_built: u64,
    drawings_built: u64,
    drawings_reused: u64,
    glyph_runs_built: u64,
}

impl Work {
    fn since(window: &Window, before: &Before) -> Self {
        let after = Before::read(window);
        Self {
            closures: after.closures - before.closures,
            nodes_created: after.kernel.nodes_created - before.kernel.nodes_created,
            nodes_destroyed: after.kernel.nodes_destroyed - before.kernel.nodes_destroyed,
            prop_changes: after.kernel.prop_changes - before.kernel.prop_changes,
            structural_ops: after.kernel.structural_ops - before.kernel.structural_ops,
            layout_runs: after.kernel.layout_runs - before.kernel.layout_runs,
            text_bytes: after.kernel.text_bytes - before.kernel.text_bytes,
            frames_built: after.frames - before.frames,
            drawings_built: after.drawings.0 - before.drawings.0,
            drawings_reused: after.drawings.1 - before.drawings.1,
            glyph_runs_built: after.drawings.2 - before.drawings.2,
        }
    }
}

#[derive(Serialize)]
struct State {
    mounted_rows: usize,
    visible_rows: usize,
    mounted_row_limit: usize,
    live_kernel_nodes: usize,
    live_reactive_nodes: u64,
    display_items: usize,
    scroll_y: f32,
    viewport_height: f32,
    content_height: f32,
    follows_tail: bool,
}

impl State {
    fn read(window: &Window, display_items: usize) -> Result<Self> {
        let kernel = window.kernel();
        let transcript = kernel.find("transcript").ok_or("the product transcript is missing")?;
        let (_, top, _, height) = kernel.shown_rect(transcript);
        let ids: Vec<_> = kernel.child_ids("transcript").into_iter().filter(|id| id.starts_with("msg-")).collect();
        let visible_rows = ids
            .iter()
            .filter_map(|id| kernel.find(id))
            .filter(|node| {
                let (_, y, _, h) = kernel.shown_rect(*node);
                h > 0.0 && y + h > top && y < top + height
            })
            .count();
        Ok(Self {
            mounted_rows: ids.len(),
            visible_rows,
            // A viewport-relative performance budget, including overscan and measurement
            // convergence room. It does not depend on total history or a fixed row height.
            mounted_row_limit: 3 * visible_rows.max(1) + 24,
            live_kernel_nodes: kernel.live_nodes(),
            live_reactive_nodes: window.app().work().reactive_nodes,
            display_items,
            scroll_y: kernel.scroll_offset(transcript).1,
            viewport_height: kernel.client_size(transcript).1,
            content_height: kernel.scroll_size(transcript).1,
            follows_tail: kernel.is_following_tail(transcript),
        })
    }
}

#[derive(Serialize)]
struct Sample {
    operation_cpu_us: f64,
    display_list_cpu_us: f64,
    total_cpu_us: f64,
    input_bytes: usize,
    work: Work,
    state: State,
}

fn measured(window: &mut Window, operation: impl FnOnce(&mut Window) -> Result<usize>) -> Result<Sample> {
    let before = Before::read(window);
    let start = Instant::now();
    let input_bytes = operation(window)?;
    let updated = Instant::now();
    let display = black_box(window.display_list());
    let end = Instant::now();
    let work = Work::since(window, &before);
    let state = State::read(window, display.items().len())?;
    Ok(Sample {
        operation_cpu_us: (updated - start).as_secs_f64() * 1e6,
        display_list_cpu_us: (end - updated).as_secs_f64() * 1e6,
        total_cpu_us: (end - start).as_secs_f64() * 1e6,
        input_bytes,
        work,
        state,
    })
}

struct Fixture {
    window: Window,
    model: LongSession,
}

impl Fixture {
    fn open(store: &Store) -> Result<Self> {
        let model = LongSession::default();
        let engine = model.clone();
        let store = store.clone();
        let app = App::new(HeadlessPlatform::default());
        let window =
            app.open_window(WindowOptions::new("Long Chat CPU benchmark").size(SIZE.0, SIZE.1), move || engine.view(Some(store.clone())))?;
        app.poll();
        app.settle_motion();
        if !window.kernel().exists("transcript") {
            return Err("the fixture did not open the seeded conversation".into());
        }
        Ok(Self { window, model })
    }

    fn start(&mut self) -> Result<()> {
        self.window.invoke("long.start")?;
        if self.model.requests() != 1 {
            return Err("the fixture must start exactly one real Model reply".into());
        }
        // Prime the first part and Markdown structure outside steady-state samples.
        self.chunk()?;
        self.window.app().settle_motion();
        black_box(self.window.display_list());
        Ok(())
    }

    fn chunk(&mut self) -> Result<usize> {
        accept_chunk(&mut self.window, &self.model)
    }

    fn scroll(&mut self, offset: f32) -> Result<()> {
        self.window.dispatch(Event::ScrollTo("transcript".into(), 0.0, offset))?;
        self.window.app().settle_motion();
        black_box(self.window.display_list());
        Ok(())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.model.pause();
        self.window.app().close_window(self.window.id());
    }
}

fn accept_chunk(window: &mut Window, model: &LongSession) -> Result<usize> {
    let before = model.sent_bytes();
    if !model.emit_chunk() {
        return Err("the real Model channel refused its streaming chunk".into());
    }
    window.app().poll();
    window.app().settle_motion();
    Ok(model.sent_bytes() - before)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    Idle,
    StreamVisible,
    StreamVisibleLongAnswer,
    StreamOffscreen,
    RevealStreamedTail,
    Wheel,
    Resize,
}

impl Scenario {
    const ALL: [Self; 7] = [
        Self::Idle,
        Self::StreamVisible,
        Self::StreamVisibleLongAnswer,
        Self::StreamOffscreen,
        Self::RevealStreamedTail,
        Self::Wheel,
        Self::Resize,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle_display_list",
            Self::StreamVisible => "stream_visible_accept_and_land",
            Self::StreamVisibleLongAnswer => "stream_visible_long_answer",
            Self::StreamOffscreen => "stream_offscreen_accept_and_land",
            Self::RevealStreamedTail => "reveal_streamed_tail",
            Self::Wheel => "wheel_one_window",
            Self::Resize => "width_roundtrip_1000_820_1000",
        }
    }

    fn sample(self, fixture: &mut Fixture, index: usize) -> Result<Sample> {
        if self == Self::RevealStreamedTail {
            fixture.scroll(0.0)?;
            fixture.chunk()?;
        }
        let model = fixture.model.clone();
        measured(&mut fixture.window, move |window| {
            match self {
                Self::Idle => {}
                Self::StreamVisible | Self::StreamVisibleLongAnswer | Self::StreamOffscreen => return accept_chunk(window, &model),
                Self::RevealStreamedTail => window.dispatch(Event::ScrollTo("transcript".into(), 0.0, f32::MAX))?,
                Self::Wheel => {
                    let (x, y, w, h) = {
                        let kernel = window.kernel();
                        kernel.shown_rect(kernel.find("transcript").ok_or("the transcript disappeared")?)
                    };
                    let dy = if index.is_multiple_of(2) { h * 0.75 } else { -h * 0.75 };
                    window.wheel(x + w * 0.5, y + h * 0.5, 0.0, dy);
                }
                Self::Resize => {
                    window.resize(NARROW, SIZE.1);
                    black_box(window.display_list());
                    window.resize(SIZE.0, SIZE.1);
                }
            }
            window.app().settle_motion();
            Ok(0)
        })
    }
}

#[derive(Serialize)]
struct AnswerPosition {
    chunks: usize,
    bytes: usize,
}

impl AnswerPosition {
    fn read(model: &LongSession) -> Self {
        Self { chunks: model.sent(), bytes: model.sent_bytes() }
    }
}

#[derive(Serialize)]
struct AnswerRange {
    before_warmup: AnswerPosition,
    before_samples: AnswerPosition,
    after_samples: AnswerPosition,
}

#[derive(Serialize)]
struct Case {
    history: usize,
    scenario: String,
    timing: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    answer: Option<AnswerRange>,
    samples: Vec<Sample>,
}

fn distribution(values: impl Iterator<Item = f64>) -> Value {
    let mut values: Vec<_> = values.collect();
    values.sort_by(f64::total_cmp);
    let quantile = |p: f64| values[((values.len() as f64 * p).ceil() as usize).saturating_sub(1).min(values.len() - 1)];
    json!({ "unit": "microseconds", "p50": quantile(0.50), "p95": quantile(0.95), "min": values[0], "max": values[values.len() - 1] })
}

fn case(history: usize, scenario: &str, samples: Vec<Sample>) -> Case {
    Case {
        history,
        scenario: scenario.into(),
        timing: json!({
            "operation_cpu": distribution(samples.iter().map(|s| s.operation_cpu_us)),
            "display_list_cpu": distribution(samples.iter().map(|s| s.display_list_cpu_us)),
            "total_cpu": distribution(samples.iter().map(|s| s.total_cpu_us)),
        }),
        answer: None,
        samples,
    }
}

fn initial_load(history: usize, store: &Store) -> Result<Case> {
    let mut samples = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let fixture = Fixture::open(store)?;
        let updated = Instant::now();
        let display = black_box(fixture.window.display_list());
        let end = Instant::now();
        samples.push(Sample {
            operation_cpu_us: (updated - start).as_secs_f64() * 1e6,
            display_list_cpu_us: (end - updated).as_secs_f64() * 1e6,
            total_cpu_us: (end - start).as_secs_f64() * 1e6,
            input_bytes: 0,
            work: Work::since(&fixture.window, &Before::default()),
            state: State::read(&fixture.window, display.items().len())?,
        });
    }
    Ok(case(history, "initial_load_store_and_ui", samples))
}

fn run_case(history: usize, store: &Store, scenario: Scenario, options: &Options) -> Result<Case> {
    let mut fixture = Fixture::open(store)?;
    fixture.start()?;
    match scenario {
        Scenario::StreamVisibleLongAnswer => {
            // The first part is already primed by start(). Queue the remainder through the
            // same real Model channel, then poll and land once, outside all timing/warmup.
            for _ in fixture.model.sent()..LONG_ANSWER_CHUNKS {
                if !fixture.model.emit_chunk() {
                    return Err("the real Model channel refused long-answer priming".into());
                }
            }
            fixture.window.app().poll();
            fixture.window.app().settle_motion();
            black_box(fixture.window.display_list());
        }
        Scenario::StreamOffscreen => fixture.scroll(0.0)?,
        Scenario::Wheel | Scenario::Resize => {
            let end = {
                let kernel = fixture.window.kernel();
                kernel.scroll_size(kernel.find("transcript").unwrap()).1
            };
            fixture.scroll(end * 0.5)?;
        }
        _ => {}
    }
    let before_warmup = AnswerPosition::read(&fixture.model);
    for index in 0..options.warmup {
        scenario.sample(&mut fixture, index)?;
    }
    let before_samples = AnswerPosition::read(&fixture.model);
    if scenario == Scenario::StreamOffscreen && fixture.window.kernel().exists(&format!("msg-{}", history + 3)) {
        return Err("the supposed offscreen streaming answer is still mounted".into());
    }
    let mut samples = Vec::new();
    for index in 0..options.samples {
        samples.push(scenario.sample(&mut fixture, options.warmup + index)?);
    }
    if !fixture.window.app().diagnostics().is_empty() {
        return Err(format!("{} produced diagnostics: {:?}", scenario.name(), fixture.window.app().diagnostics()).into());
    }
    if matches!(scenario, Scenario::StreamVisible | Scenario::StreamVisibleLongAnswer)
        && samples.iter().any(|sample| !sample.state.follows_tail)
    {
        return Err("the supposed visible-stream case stopped following its tail".into());
    }
    let mut report = case(history, scenario.name(), samples);
    report.answer = Some(AnswerRange { before_warmup, before_samples, after_samples: AnswerPosition::read(&fixture.model) });
    Ok(report)
}

fn gates(cases: &[Case]) -> (Vec<Value>, bool) {
    let mut checks = Vec::new();
    for case in cases {
        let bounded = case.samples.iter().all(|s| s.state.mounted_rows <= s.state.mounted_row_limit);
        checks.push(
            json!({ "history": case.history, "scenario": case.scenario, "gate": "viewport_relative_mounted_rows", "passed": bounded }),
        );
        if case.scenario == Scenario::StreamOffscreen.name() {
            let unchanged =
                case.samples.iter().all(|s| s.work.nodes_created == 0 && s.work.nodes_destroyed == 0 && s.work.structural_ops == 0);
            checks.push(json!({ "history": case.history, "scenario": case.scenario, "gate": "offscreen_chunk_creates_no_ui_nodes", "passed": unchanged }));
        }
    }
    // Compare structural work at matched viewport, message pattern and sample sequence.
    // Initial model/index construction is deliberately excluded. Times are reported, never
    // used as a machine-dependent pass/fail millisecond threshold.
    type Count = fn(&Sample) -> u64;
    let fields: [(&str, Count); 3] =
        [("closures", |s| s.work.closures), ("nodes_created", |s| s.work.nodes_created), ("structural_ops", |s| s.work.structural_ops)];
    for scenario in Scenario::ALL {
        let small = cases.iter().find(|c| c.history == HISTORY[0] && c.scenario == scenario.name()).unwrap();
        let large = cases.iter().find(|c| c.history == HISTORY[1] && c.scenario == scenario.name()).unwrap();
        for (name, field) in fields {
            let a = small.samples.iter().map(field).max().unwrap();
            let b = large.samples.iter().map(field).max().unwrap();
            let limit = a * 2 + 32;
            checks.push(json!({
                "scenario": scenario.name(), "gate": "10x_history_keeps_structural_work_bounded", "metric": name,
                "small_max": a, "large_max": b, "limit": limit, "passed": b <= limit,
            }));
        }
    }
    let passed = checks.iter().all(|check| check["passed"] == true);
    (checks, passed)
}

fn run(options: &Options) -> Result<Value> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let directory = std::env::temp_dir().join(format!("octara-long-bench-{}-{stamp}", std::process::id()));
    std::fs::create_dir(&directory)?;
    let mut cases = Vec::new();
    for history in HISTORY {
        let path = directory.join(history.to_string());
        let store = long_session::seed(&path, history)?;
        eprintln!("long-bench: {history} messages, initial load");
        cases.push(initial_load(history, &store)?);
        for scenario in Scenario::ALL {
            eprintln!("long-bench: {history} messages, {}", scenario.name());
            cases.push(run_case(history, &store, scenario, options)?);
        }
        // This benchmark never completes a turn or saves. Remove only its known seed file
        // and now-empty directory; unexpected files are retained and reported by failure.
        std::fs::remove_file(path.join("1.json"))?;
        std::fs::remove_dir(path)?;
    }
    std::fs::remove_dir(&directory)?;
    let (checks, passed) = gates(&cases);
    Ok(json!({
        "schema": "octara.chat.long_bench/1", "passed": passed,
        "profile": "release", "host": "headless", "font_source": "bundled", "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH, "viewport": [SIZE.0, SIZE.1], "histories": HISTORY,
        "executable": std::env::current_exe()?.display().to_string(),
        "available_parallelism": std::thread::available_parallelism()?.get(),
        "samples_per_case": options.samples, "warmup_per_case": options.warmup,
        "measurement": "in-process UI-thread operation plus display-list preparation; layout enabled; no CLI transport, GPU raster/submission, native event loop or physical presentation",
        "streaming": "real Model Sender -> Chat::on_event -> TextBuffer -> production paced Markdown; each chunk explicitly landed with App::settle_motion; no virtual time or wall-clock frame-rate claim",
        "long_answer": "stream_visible_long_answer starts with 1500 total chunks before warmup; after the first part, remaining priming chunks are queued through the same Model channel, then one poll, settle_motion and display-list preparation; every streaming case records answer chunks/bytes before warmup, before samples and after samples",
        "initial_load": "five fresh app instances; Store::load/JSON and retained data/index construction included; seed generation excluded; O(history) allowed; the seed was just written and reused, so OS file caches are expected to be warm, not cold-disk startup",
        "resize": "one 1000 -> 820 -> 1000 logical-width roundtrip including both display lists; measured rows reflow and index estimates can need additional layout; historical TextBuffers remain resident",
        "gates": checks, "cases": cases,
        "references": {
            "exact": "4c16fa59f74b1920af7dbc861a15a8105ed0ad13: apps/messages-stress/README.md (UI virtualization does not bound retained history; report the actual host and timing boundary)",
            "rune": "f1f54ce8bf485ee3a95804deace315f53a8970f6: crates/rune/src/el/virtual_rows.rs (only mounted rows are built; key checks can still scan history; distinguish those costs from production Chat)",
        },
    }))
}

fn main() -> ExitCode {
    let result = Options::read().and_then(|options| {
        let report = run(&options)?;
        let text = serde_json::to_string_pretty(&report)?;
        if let Some(path) = options.output {
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
            file.write_all(text.as_bytes())?;
            eprintln!("long-bench: report {}", path.display());
        } else {
            println!("{text}");
        }
        Ok(report["passed"] == true)
    });
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("long-bench: {error}");
            ExitCode::from(2)
        }
    }
}
