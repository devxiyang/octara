//! `octara vocab` and `octara explain`: the catalog answers from the API read from the source and
//! the model's tables, and the binary prints it as text and JSON.

use octara_cli::api::{self, Kind};
use octara_cli::catalog::Catalog;
use octara_model::motion::Spring;
use octara_model::theme::{Color, ColorToken, Theme};
use std::process::Command;

fn catalog() -> Catalog {
    Catalog::build(api::load())
}

fn run(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_octara")).args(args).output().unwrap();
    (out.status.success(), String::from_utf8(out.stdout).unwrap() + &String::from_utf8(out.stderr).unwrap())
}

#[test]
fn lookups_take_paths_suffixes_and_prelude_names() {
    let cat = catalog();
    let found = cat.lookup("el");
    assert_eq!(found.matches[0].path, "octara::el", "the prelude lists the constructor");
    assert!(found.matches[0].prelude);
    assert_eq!(cat.lookup("Styled::p").matches[0].path, "octara::Styled::p");
    assert_eq!(cat.lookup("octara::prelude::Spring").matches[0].path, "octara_model::motion::Spring");
    assert_eq!(cat.lookup("octara::prelude::Spring::SNAP").matches[0].path, "octara_model::motion::Spring::SNAP");
    assert!(cat.lookup(".on_press()").matches.iter().any(|item| item.path == "octara::El::on_press"), "a written call is found too");
    assert!(cat.lookup("El::bg").matches[0].prelude, "members of prelude types are in reach");
    assert!(!cat.lookup("style").similar.is_empty());
}

#[test]
fn model_tables_are_merged_in() {
    let cat = catalog();
    let accent = cat.get("octara_model::theme::ColorToken::Accent").unwrap();
    let hex = |c: Color| {
        let c = c.to_rgba8();
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    };
    assert_eq!(accent.facts["light"], hex(Theme::light().get(ColorToken::Accent)));
    assert_eq!(accent.facts["dark"], hex(Theme::dark().get(ColorToken::Accent)));
    for t in ColorToken::BUILTIN {
        assert!(cat.get(&format!("octara_model::theme::ColorToken::{t:?}")).unwrap().facts.contains_key("light"), "{t:?}");
    }
    let stale = cat.lookup("stale_target");
    assert_eq!(stale.matches[0].kind, "diagnostic");
}

#[test]
fn every_named_spring_is_in_the_table() {
    // `Spring::NAMED` is written by hand next to the constants; the API sees every constant.
    let api = api::load();
    let consts: Vec<&str> = api
        .items()
        .filter(|i| i.kind == Kind::AssocConst && i.path.starts_with("octara_model::motion::Spring::"))
        .map(|i| i.path.rsplit("::").next().unwrap())
        .filter(|n| *n != "NAMED")
        .collect();
    assert!(!consts.is_empty());
    for c in consts {
        assert!(Spring::NAMED.iter().any(|(n, _)| *n == c), "Spring::{c} is missing from Spring::NAMED");
    }
    assert_eq!(catalog().get("octara_model::motion::Spring::POP").unwrap().facts["bounce"], "0.15");
}

#[test]
fn diagnostics_are_found_in_messages_and_reference_known_entries() {
    let cat = catalog();
    let e = cat.code_in("thread 'main' panicked at '[READ_DISPOSED] at src/app.rs:3:9'").unwrap();
    assert_eq!(e.path, "READ_DISPOSED");
    for e in cat.entries.iter().filter(|e| e.kind == "diagnostic") {
        for r in &e.related {
            assert!(cat.get(r).is_some(), "{}: related {r} is an entry", e.path);
        }
    }
}

#[test]
fn a_type_lists_its_members() {
    let styled = catalog().get("octara::Styled").cloned().unwrap();
    for name in ["p", "gap", "text_size", "text_overflow", "rounded", "shadow"] {
        assert!(styled.members.iter().any(|member| member == name), "missing style method {name}");
    }
}

#[test]
fn the_binary_prints_text_and_json() {
    let (ok, text) = run(&["vocab", "Spring::SNAP"]);
    assert!(ok && text.contains("duration: 0.25 s"), "{text}");
    let (ok, json) = run(&["vocab", "AgentRequest", "--json"]);
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["matches"][0]["path"], "octara_inspect::AgentRequest");
    let (ok, text) = run(&["explain", "[STALE_TARGET]"]);
    assert!(ok && text.contains("tree incarnation"), "{text}");
    let (ok, _) = run(&["explain", "not_a_code"]);
    assert!(!ok, "an unknown code fails");
    let (ok, _) = run(&["vocab", "no_such_name_anywhere"]);
    assert!(!ok, "nothing found fails");
    let (ok, text) = run(&["vocab"]);
    assert!(ok && text.lines().any(|l| l.contains(" BuildCx ")), "the prelude listing names `BuildCx`");
}

#[test]
fn help_lists_every_command() {
    let (ok, help) = run(&["--help"]);
    assert!(ok);
    for c in ["vocab", "explain", "check", "inspect", "run", "launch", "release", "web", "skill"] {
        assert!(help.lines().any(|l| l.trim_start().starts_with(c)), "--help lists `{c}`:\n{help}");
    }
}

#[test]
fn a_standalone_cli_has_the_public_api_and_skill() {
    let temp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_octara"))
            .current_dir(temp.path())
            .env("CARGO", temp.path().join("unavailable-cargo"))
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        output.stdout
    };
    let found: serde_json::Value = serde_json::from_slice(&run(&["vocab", "octara::prelude::BuildCx::signal", "--json"])).unwrap();
    let item = &found["matches"][0];
    assert!(item["sig"].as_str().unwrap().contains("Signal<T>"));
    assert!(item["file"].as_str().unwrap().starts_with("crates/octara-runtime/src/"));
    assert!(String::from_utf8(run(&["explain", "STALE_TARGET"])).unwrap().contains("tree incarnation"));
    run(&["skill", "export", "."]);
    let guide = std::fs::read_to_string(temp.path().join("octara/references/start-here.md")).unwrap();
    assert!(guide.contains("BuildCx"));
    assert!(guide.contains("octara.inspect/2"));
    run(&["skill", "install", "--agent", "agents", "--no-pointer"]);
    let index = std::fs::read_to_string(temp.path().join(".agents/skills/octara/references/api/index.md")).unwrap();
    assert!(index.contains("Source: `crates/octara-runtime`"));
}
