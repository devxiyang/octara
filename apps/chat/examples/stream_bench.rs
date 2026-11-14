//! Release CPU baselines for real Chat Model ingestion and long Markdown tails.
//! No CLI/IPC, native event loop, GPU encoding/submission or presentation is measured.
//! `--overload` adds a 4 MiB diagnostic; it is not a product acceptance threshold.

#[path = "support/long_session.rs"]
mod long_session;

use long_session::{LongSession, Payload};
use octara::platform::{HeadlessPlatform, WindowOptions};
use octara::prelude::*;
use octara_chat::store::Store;
use serde_json::{Value, json};
use std::error::Error;
use std::hint::black_box;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct Fixture {
    window: Window,
    model: LongSession,
}

impl Fixture {
    fn open(store: &Store, payload: Payload, chunk_bytes: usize) -> Result<Self> {
        let model = LongSession::default();
        model.set_payload(payload, chunk_bytes)?;
        let engine = model.clone();
        let store = store.clone();
        let app = App::new(HeadlessPlatform::default());
        let mut window =
            app.open_window(WindowOptions::new("Chat stream CPU").size(1000.0, 720.0), move || engine.view(Some(store.clone())))?;
        app.poll();
        window.invoke("long.start")?;
        Ok(Self { window, model })
    }

    fn ingest(&mut self, count: usize, land: bool) -> Result<Value> {
        let before_bytes = self.model.sent_bytes();
        let before_chunks = self.model.sent();
        let before = self.window.kernel().counters();
        let closures = self.window.app().work().closures_run;
        let frames = self.window.kernel().frames_built();
        let start = Instant::now();
        let queued = self.model.emit_burst(count);
        let enqueued = Instant::now();
        if queued != count {
            return Err("the bounded transport refused benchmark priming/input".into());
        }
        self.window.app().poll();
        let accepted = Instant::now();
        if land {
            self.window.app().settle_motion();
        }
        let landed = Instant::now();
        let display = black_box(self.window.display_list());
        let end = Instant::now();
        let after = self.window.kernel().counters();
        Ok(json!({
            "enqueue_us": (enqueued-start).as_secs_f64()*1e6,
            "poll_us": (accepted-enqueued).as_secs_f64()*1e6,
            "land_paced_us": (landed-accepted).as_secs_f64()*1e6,
            "display_list_us": (end-landed).as_secs_f64()*1e6,
            "total_us": (end-start).as_secs_f64()*1e6,
            "input_chunks": self.model.sent()-before_chunks, "input_bytes": self.model.sent_bytes()-before_bytes,
            "sent_chunks": self.model.sent(), "sent_bytes": self.model.sent_bytes(),
            "work": {"closures":self.window.app().work().closures_run-closures,
                "nodes_created":after.nodes_created-before.nodes_created, "nodes_destroyed":after.nodes_destroyed-before.nodes_destroyed,
                "prop_changes":after.prop_changes-before.prop_changes, "layout_runs":after.layout_runs-before.layout_runs,
                "text_bytes":after.text_bytes-before.text_bytes, "frames_built":self.window.kernel().frames_built()-frames},
            "display_items":display.items().len(), "live_nodes":self.window.kernel().live_nodes(),
        }))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.model.pause();
        self.window.app().close_window(self.window.id());
    }
}

fn distribution(samples: &[Value], field: &str) -> Value {
    let mut values: Vec<_> = samples.iter().map(|sample| sample[field].as_f64().unwrap()).collect();
    values.sort_by(f64::total_cmp);
    let at = |p: f64| values[((values.len() as f64 * p).ceil() as usize).saturating_sub(1).min(values.len() - 1)];
    json!({"unit":"microseconds", "p50":at(0.5), "p95":at(0.95), "p99":at(0.99), "min":values[0], "max":values[values.len()-1]})
}

fn run(samples: usize, overload: bool) -> Result<Value> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let directory = std::env::temp_dir().join(format!("octara-stream-bench-{}-{stamp}", std::process::id()));
    let store = long_session::seed(&directory, 1000)?;
    let mut cases = Vec::new();
    for payload in [Payload::Mixed, Payload::Paragraph, Payload::Code] {
        let mut fixture = Fixture::open(&store, payload, 1024)?;
        // Prime 64 KiB for paragraph/code through the actual transport, outside timing.
        fixture.ingest(64, true)?;
        let mut raw = Vec::new();
        for _ in 0..samples {
            raw.push(fixture.ingest(1, true)?);
        }
        cases.push(json!({"name":format!("{}_tail_increment_1x1024_land", payload.name()),
            "payload":payload, "prime_chunks":64, "chunk_bytes":1024,
            "total":distribution(&raw,"total_us"), "poll":distribution(&raw,"poll_us"),
            "land_paced":distribution(&raw,"land_paced_us"), "display_list":distribution(&raw,"display_list_us"), "samples":raw}));
    }
    for chunk_bytes in [256, 16 * 1024] {
        let is_overload = chunk_bytes == 16 * 1024;
        if is_overload && !overload {
            continue;
        }
        let count = 256;
        // A fresh answer per sample bounds memory. No prior response is stopped/saved
        // inside this ingestion case; setup is not part of its timing.
        let mut raw = Vec::new();
        let sample_count = if is_overload { samples.min(3) } else { samples };
        for _ in 0..sample_count {
            let mut fixture = Fixture::open(&store, Payload::Code, chunk_bytes)?;
            raw.push(fixture.ingest(count, false)?);
        }
        cases.push(json!({"name":format!("code_burst_{count}x{chunk_bytes}_poll_without_land"),
            "payload":"code", "chunk_bytes":chunk_bytes, "burst_chunks":count, "overload":is_overload,
            "total":distribution(&raw,"total_us"), "poll":distribution(&raw,"poll_us"),
            "samples":raw, "note":if is_overload {
                "Explicit 4 MiB overload diagnostic, not a product acceptance threshold; at most 3 raw samples, with descriptive p95/p99 order statistics rather than a robust latency estimate"
            } else {
                "64 KiB burst is a selected test workload, not a framework capacity limit"
            }}));
    }
    std::fs::remove_file(directory.join("1.json"))?;
    std::fs::remove_dir(&directory)?;
    Ok(json!({"schema":"octara.chat.stream_bench/1", "profile":"release", "history":1000,
        "host":"headless", "font_source":"bundled", "os":std::env::consts::OS, "arch":std::env::consts::ARCH,
        "measurement":"Instant elapsed in-process UI-thread work: bounded Model enqueue, one App::poll, optional complete paced landing, display-list preparation; no IPC, native loop, GPU or physical frame claim",
        "poll_boundary":"App::poll may currently drain the whole Model queue before returning; future/task yield does not prove return to the native event loop",
        "overload_enabled":overload,
        "workload_boundary":"Default tail and 64 KiB burst sizes are test selections, not framework capacity limits; --overload adds a 4 MiB diagnostic, not a product acceptance threshold",
        "limits":{"reply_bytes":long_session::MAX_REPLY_BYTES,"reply_chunks":long_session::MAX_REPLY_CHUNKS},
        "cases":cases}))
}

fn main() -> ExitCode {
    let result = (|| -> Result<()> {
        let mut samples = 20;
        let mut output: Option<PathBuf> = None;
        let mut overload = false;
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--samples" => samples = args.next().ok_or("--samples requires a value")?.parse()?,
                "--output" => output = Some(args.next().ok_or("--output requires a path")?.into()),
                "--overload" => overload = true,
                "--help" | "-h" => {
                    println!(
                        "Usage: stream_bench [--samples 5..=100] [--output PATH] [--overload]\n\
                         Default: mixed/paragraph/code tail increments and a 64 KiB code burst.\n\
                         --overload adds a 4 MiB diagnostic, not a product acceptance threshold.\n\
                         Workload sizes are test choices, not framework capacity limits. Run with --release."
                    );
                    return Ok(());
                }
                _ => return Err(format!("unknown option {arg}").into()),
            }
        }
        if cfg!(debug_assertions) {
            return Err("stream_bench requires --release".into());
        }
        if !(5..=100).contains(&samples) {
            return Err("use 5..=100 samples".into());
        }
        let report = run(samples, overload)?;
        let text = serde_json::to_string_pretty(&report)?;
        if let Some(path) = output {
            std::fs::OpenOptions::new().write(true).create_new(true).open(path)?.write_all(text.as_bytes())?;
        } else {
            println!("{text}");
        }
        Ok(())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("stream-bench: {error}");
            ExitCode::from(2)
        }
    }
}
