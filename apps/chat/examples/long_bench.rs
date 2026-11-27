//! Compare the retained transcript's stream update cost at 1,000 and 10,000 messages.

#[path = "support/headless.rs"]
mod headless;
#[path = "support/long_session.rs"]
mod long_session;

use headless::Fixture;
use long_session::Payload;
use serde_json::json;
use std::error::Error;
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<(), Box<dyn Error>> {
    let mut samples = 40;
    let mut warmup = 4;
    let mut output = None;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        if argument == "--help" {
            println!("long_bench [--samples 10..200] [--warmup 0..20] [--output FILE]");
            return Ok(());
        }
        let value = args.next().ok_or_else(|| format!("{argument} needs a value"))?;
        match argument.as_str() {
            "--samples" => samples = value.parse()?,
            "--warmup" => warmup = value.parse()?,
            "--output" => output = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown option {argument}").into()),
        }
    }
    if !(10..=200).contains(&samples) || warmup > 20 {
        return Err("use 10..=200 samples and at most 20 warmup operations".into());
    }
    let mut cases = Vec::new();
    for history in [1_000, 10_000] {
        for payload in [Payload::Mixed, Payload::Paragraph, Payload::Code] {
            let started = Instant::now();
            let mut fixture = Fixture::new(history, payload, 1024)?;
            let fixture_setup_ms = started.elapsed().as_secs_f64() * 1e3;
            for _ in 0..warmup {
                fixture.sample(1)?;
            }
            let report = (0..samples).map(|_| fixture.sample(1)).collect::<Result<Vec<_>, _>>()?;
            cases.push(json!({ "history": history, "payload": payload, "fixture_setup_ms": fixture_setup_ms, "samples": report }));
        }
    }
    let report = serde_json::to_string_pretty(&json!({ "schema": "octara.chat.long-bench/2", "cases": cases }))?;
    if let Some(path) = output {
        std::fs::write(path, format!("{report}\n"))?;
    } else {
        println!("{report}");
    }
    Ok(())
}
