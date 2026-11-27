//! Measure the real Chat event channel and committed Markdown projection.

#[path = "support/headless.rs"]
mod headless;
#[path = "support/long_session.rs"]
mod long_session;

use headless::Fixture;
use long_session::Payload;
use serde_json::json;
use std::error::Error;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let mut samples = 20;
    let mut output = None;
    let mut overload = false;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--help" => {
                println!("stream_bench [--samples 1..100] [--overload] [--output FILE]");
                return Ok(());
            }
            "--samples" => samples = args.next().ok_or("--samples needs a value")?.parse()?,
            "--output" => output = Some(PathBuf::from(args.next().ok_or("--output needs a path")?)),
            "--overload" => overload = true,
            _ => return Err(format!("unknown option {argument}").into()),
        }
    }
    if !(1..=100).contains(&samples) {
        return Err("use 1..=100 samples".into());
    }
    let mut cases = Vec::new();
    for (payload, batch) in [(Payload::Paragraph, 1), (Payload::Code, 1), (Payload::Mixed, 32)] {
        let mut fixture = Fixture::new(1_000, payload, 512)?;
        fixture.sample(1)?;
        let report = (0..samples).map(|_| fixture.sample(batch)).collect::<Result<Vec<_>, _>>()?;
        cases.push(json!({ "payload": payload, "batch": batch, "samples": report }));
    }
    if overload {
        let mut fixture = Fixture::new(1_000, Payload::Code, 16_384)?;
        cases.push(json!({ "payload": "code", "batch": 256, "samples": [fixture.sample(256)?] }));
    }
    let report = serde_json::to_string_pretty(&json!({ "schema": "octara.chat.stream-bench/2", "cases": cases }))?;
    if let Some(path) = output {
        std::fs::write(path, format!("{report}\n"))?;
    } else {
        println!("{report}");
    }
    Ok(())
}
