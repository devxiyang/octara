//! The diagnostics registry is complete: every code the source, tests and docs mention is
//! registered, every registered code is reported somewhere, and every doc anchor resolves.

use octara::diagnostics::{self, CODES};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "target") {
                files(&p, ext, out);
            }
        } else if p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
}

/// `OVERFLOW`, `WRITE_IN_BUILD`: upper case words joined by `_`, at least four characters.
fn code_shaped(s: &str) -> bool {
    s.len() >= 4
        && s.split('_').all(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()))
        && s.starts_with(|c: char| c.is_ascii_uppercase())
}

/// Upper-case strings that are not diagnostic codes: the environment variables cargo sets and
/// Octara reads.
fn not_a_code(s: &str) -> bool {
    ["CARGO", "RUST", "OCTARA_"].iter().any(|p| s.starts_with(p))
}

/// String literals in Rust source, leaving out environment variable names and argument
/// placeholders (`var_os("HOME")`, `value_name = "AGENT"`).
fn rust_literals(text: &str) -> Vec<&str> {
    let parts: Vec<&str> = text.split('"').collect();
    (1..parts.len())
        .step_by(2)
        .filter(|&i| {
            let before = parts[i - 1].trim_end();
            !["var(", "var_os(", "env(", "env_remove(", "value_name ="].iter().any(|p| before.ends_with(p))
        })
        .map(|i| parts[i])
        .collect()
}

/// The contents of `"..."` literals (Rust) or `` `...` `` spans (Markdown) in `text`.
fn quoted(text: &str, quote: char) -> Vec<&str> {
    text.split(quote).skip(1).step_by(2).collect()
}

/// GitHub's heading anchor: lower case, punctuation dropped, spaces to `-`.
fn slug(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

#[test]
fn codes_are_unique_and_explained() {
    let mut seen = BTreeSet::new();
    for c in CODES {
        assert!(seen.insert(c.id), "{} registered twice", c.id);
        assert!(code_shaped(c.id), "{} is not an upper-case code", c.id);
        for (what, text) in [("summary", c.summary), ("cause", c.cause)] {
            assert!(text.ends_with('.'), "{}: the {what} is a sentence ending in a period: {text:?}", c.id);
        }
        assert_eq!(diagnostics::code(c.id), Some(*c));
    }
}

#[test]
fn doc_anchors_resolve() {
    let root = workspace();
    for c in CODES {
        let (file, anchor) = c.doc.split_once('#').unwrap_or_else(|| panic!("{}: doc {:?} has no #anchor", c.id, c.doc));
        let text = std::fs::read_to_string(root.join(file)).unwrap_or_else(|e| panic!("{}: {file}: {e}", c.id));
        let anchors: Vec<String> = text.lines().filter_map(|l| l.strip_prefix('#')).map(|h| slug(h.trim_start_matches('#'))).collect();
        assert!(anchors.iter().any(|a| a == anchor), "{}: {file} has no heading #{anchor}; it has {anchors:?}", c.id);
    }
}

#[test]
fn every_code_mentioned_anywhere_is_registered() {
    let root = workspace();
    let mut rs = Vec::new();
    for dir in ["crates", "examples"] {
        files(&root.join(dir), "rs", &mut rs);
    }
    let mut md = Vec::new();
    files(&root.join("docs/design"), "md", &mut md);
    for f in ["docs/ARCHITECTURE.md", "docs/ROADMAP.md", "docs/TODO.md", "AGENTS.md"] {
        md.push(root.join(f));
    }
    // Source and docs also name constants (`Spring::SNAP`, `State::FOCUSED`): what the source
    // declares as a `const` or `static` is not a code.
    let sources: Vec<String> = rs.iter().map(|f| std::fs::read_to_string(f).unwrap()).collect();
    let constants: BTreeSet<&str> = sources
        .iter()
        .flat_map(|s| s.split("const ").skip(1).chain(s.split("static ").skip(1)))
        .filter_map(|rest| rest.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).next())
        .filter(|n| code_shaped(n))
        .collect();
    let mut unknown = BTreeSet::new();
    let mut check = |path: &Path, tokens: Vec<&str>| {
        for t in tokens {
            if code_shaped(t) && !not_a_code(t) && !constants.contains(t) && diagnostics::code(t).is_none() {
                unknown.insert(format!("{t} ({})", path.strip_prefix(&root).unwrap().display()));
            }
        }
    };
    for (f, text) in rs.iter().zip(&sources) {
        check(f, rust_literals(text));
    }
    for f in &md {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        check(f, quoted(&text, '`'));
    }
    assert!(unknown.is_empty(), "codes used but not registered in octara::diagnostics: {unknown:#?}");
}

#[test]
fn every_registered_code_is_reported() {
    let root = workspace();
    let mut rs = Vec::new();
    files(&root.join("crates/octara/src"), "rs", &mut rs);
    let source: String = rs.iter().filter(|f| !f.ends_with("diagnostics.rs")).map(|f| std::fs::read_to_string(f).unwrap()).collect();
    let unused: Vec<&str> = CODES.iter().map(|c| c.id).filter(|id| !source.contains(&format!("codes::{id}"))).collect();
    assert!(unused.is_empty(), "registered codes nothing reports (remove them or report them): {unused:?}");
}
