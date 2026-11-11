//! `octara vocab` and `octara explain`: the catalog answers from api.json and the kernel's tables,
//! and the binary prints it as text and JSON.

use octara::app::BUILTIN_COMMANDS;
use octara::kernel::{Theme, Token};
use octara::motion::Spring;
use octara_cli::api::{self, Kind};
use octara_cli::catalog::{Catalog, compress};
use std::process::Command;

fn catalog() -> Catalog {
    Catalog::build(&api::load())
}

fn run(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_octara")).args(args).output().unwrap();
    (out.status.success(), String::from_utf8(out.stdout).unwrap() + &String::from_utf8(out.stderr).unwrap())
}

#[test]
fn lookups_take_paths_suffixes_and_prelude_names() {
    let cat = catalog();
    let found = cat.lookup("state");
    assert_eq!(found.matches[0].path, "octara::reactive::graph::state", "the prelude's `state` comes first");
    assert!(found.matches[0].prelude);
    assert_eq!(cat.lookup("Styled::p_4").matches[0].path, "octara::view::Styled::p_4");
    assert_eq!(cat.lookup("octara::prelude::Spring").matches[0].path, "octara::motion::Spring");
    assert_eq!(cat.lookup(".hover()").matches[0].path, "octara::view::El::hover", "a written call is found too");
    assert!(cat.lookup("El::bg").matches[0].prelude, "members of prelude types are in reach");
    assert!(cat.lookup("hover").similar.contains(&"octara::view::El::hover_alone"));
}

#[test]
fn kernel_tables_are_merged_in() {
    let cat = catalog();
    let accent = cat.get("octara::kernel::Token::Accent").unwrap();
    let hex = |c: octara::kernel::Color| {
        let c = c.to_rgba8();
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    };
    assert_eq!(accent.facts["light"], hex(Theme::light().get(Token::Accent)));
    assert_eq!(accent.facts["dark"], hex(Theme::dark().get(Token::Accent)));
    for t in Token::BUILTIN {
        assert!(cat.get(&format!("octara::kernel::Token::{t:?}")).unwrap().facts.contains_key("light"), "{t:?}");
    }
    for (command, _) in BUILTIN_COMMANDS {
        assert_eq!(cat.get(command).unwrap().kind, "command");
    }
    assert_eq!(cat.get("edit.undo").unwrap().facts["keys.macos"], "mod+z (in text_input)", "chords as `bind` takes them");
    let overflow = cat.lookup("overflow");
    assert!(overflow.matches.iter().any(|e| e.kind == "diagnostic"), "codes are found in any case");
}

#[test]
fn every_named_spring_is_in_the_table() {
    // `Spring::NAMED` is written by hand next to the constants; api.json sees every constant.
    let api = api::load();
    let consts: Vec<&str> = api
        .items()
        .filter(|i| i.kind == Kind::AssocConst && i.path.starts_with("octara::motion::Spring::"))
        .map(|i| i.path.rsplit("::").next().unwrap())
        .filter(|n| *n != "NAMED")
        .collect();
    assert!(!consts.is_empty());
    for c in consts {
        assert!(Spring::NAMED.iter().any(|(n, _)| *n == c), "Spring::{c} is missing from Spring::NAMED");
    }
    assert_eq!(catalog().get("octara::motion::Spring::POP").unwrap().facts["bounce"], "0.15");
}

#[test]
fn diagnostics_link_to_the_api_their_fix_names() {
    let cat = catalog();
    let e = cat.code_in("thread 'main' panicked at '[READ_DISPOSED] at src/app.rs:3:9'").unwrap();
    assert_eq!(e.path, "READ_DISPOSED");
    let overflow = cat.get("OVERFLOW").unwrap();
    assert!(overflow.related.iter().any(|p| p == "octara::view::Styled::flex_shrink"), "{:?}", overflow.related);
    for e in cat.entries.iter().filter(|e| e.kind == "diagnostic") {
        for r in &e.related {
            assert!(cat.get(r).is_some(), "{}: related {r} is an entry", e.path);
        }
    }
}

#[test]
fn members_are_listed_compactly() {
    let names: Vec<String> = ["p", "p_0", "p_1", "p_2", "px_1", "hover"].iter().map(|s| s.to_string()).collect();
    assert_eq!(compress(&names), ["p", "p_{0,1,2}", "px_1", "hover"]);
    let styled = catalog().get("octara::view::Styled").cloned().unwrap();
    assert!(styled.members.len() > 500);
}

#[test]
fn the_binary_prints_text_and_json() {
    let (ok, text) = run(&["vocab", "Spring::SNAP"]);
    assert!(ok && text.contains("duration: 0.25 s"), "{text}");
    let (ok, json) = run(&["vocab", "focus.next", "--json"]);
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["matches"][0]["facts"]["keys.macos"], "tab");
    let (ok, text) = run(&["explain", "collapsed"]);
    assert!(ok && text.contains("fix: Give the parent a size"), "{text}");
    let (ok, _) = run(&["explain", "not_a_code"]);
    assert!(!ok, "an unknown code fails");
    let (ok, _) = run(&["vocab", "no_such_name_anywhere"]);
    assert!(!ok, "nothing found fails");
    let (ok, text) = run(&["vocab"]);
    assert!(ok && text.lines().any(|l| l.contains(" state ")), "the prelude listing names `state`");
}

#[test]
fn help_lists_every_command() {
    let (ok, help) = run(&["--help"]);
    assert!(ok);
    for c in ["vocab", "explain", "index", "skill", "api"] {
        assert!(help.lines().any(|l| l.trim_start().starts_with(c)), "--help lists `{c}`:\n{help}");
    }
}
