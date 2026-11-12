//! The `octara` skill: what an agent needs to write Octara apps, generated for this version and
//! installed where coding agents look ([Agent Skills](https://agentskills.io/specification)).
//!
//! The canonical copy goes to `.agents/skills/octara/`, which most agents read (Codex, Cursor,
//! Copilot, Gemini CLI, OpenCode, Amp, Windsurf, Zed, Junie, Goose); agents with their own
//! directory (Claude Code, Kiro; Amp's user directory) get a link to it. A short block in the
//! project's `AGENTS.md` points at it, since a skill alone is often never invoked.

use crate::api::Api;
use std::path::{Path, PathBuf};

pub const NAME: &str = "octara";

/// The hand-written part of `SKILL.md`.
const BODY: &str = include_str!("../skill/body.md");
/// The one page to read, shared with the docs (its code blocks run as doctests there).
const START_HERE: &str = include_str!("../../../docs/start-here.md");

const DESCRIPTION: &str = "Build and change native desktop and mobile UI with the Octara Rust framework: \
elements, styles, state, lists, input, motion, async work, headless tests and diagnostics. Use whenever \
code uses the `octara` crates (`use octara::prelude::*`), the task touches Octara views or tests, or an \
Octara diagnostic code (such as OVERFLOW or STRICT_READ_UNTRACKED) appears.";

/// The skill's files, relative to its directory; with `api`, its reference under
/// `references/api/` (generated where the skill is installed, for the version the project uses).
pub fn files(api: Option<&Api>) -> Vec<(String, String)> {
    let skill = format!(
        "---\nname: {NAME}\ndescription: {}\nlicense: MIT OR Apache-2.0\nmetadata:\n  octara-version: {}\n---\n\n{BODY}",
        yaml_string(DESCRIPTION),
        yaml_string(env!("CARGO_PKG_VERSION"))
    );
    let mut out = vec![("SKILL.md".to_string(), skill), ("references/start-here.md".to_string(), for_readers(START_HERE))];
    if let Some(api) = api {
        out.extend(crate::reference::files(api).into_iter().map(|(p, t)| (format!("references/api/{p}"), t)));
    }
    out
}

/// A YAML double-quoted string: safe for any text (a plain scalar may not contain `: `).
fn yaml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `start-here.md` for reading: the lines doctests hide (`# use ..`) dropped, fences plain `rust`.
fn for_readers(doc: &str) -> String {
    let mut out = String::new();
    let mut in_rust = false;
    for line in doc.lines() {
        if let Some(info) = line.strip_prefix("```") {
            if in_rust {
                in_rust = false;
            } else if info.starts_with("rust") {
                in_rust = true;
                out += "```rust\n";
                continue;
            }
        } else if in_rust && (line == "#" || line.starts_with("# ")) {
            continue;
        }
        out += line;
        out += "\n";
    }
    out
}

/// Writes the skill into `dir/octara/` without the API reference (a copy for `npx skills add`;
/// `octara skill install` writes the reference).
pub fn export(dir: &Path) -> Result<PathBuf, String> {
    let root = dir.join(NAME);
    write_files(None, &root)?;
    Ok(root)
}

fn write_files(api: Option<&Api>, root: &Path) -> Result<(), String> {
    if api.is_some() {
        // Modules come and go between versions: the reference is written afresh.
        let reference = root.join("references/api");
        if reference.is_dir() {
            std::fs::remove_dir_all(&reference).map_err(|e| format!("{}: {e}", reference.display()))?;
        }
    }
    for (rel, text) in files(api) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

/// The files of the skill at `root` that differ from this version's (missing ones included).
pub fn stale(api: Option<&Api>, root: &Path) -> Vec<String> {
    files(api)
        .into_iter()
        .filter(|(rel, text)| std::fs::read_to_string(root.join(rel)).ok().as_deref() != Some(text.as_str()))
        .map(|(rel, _)| rel)
        .collect()
}

/// An agent that does not read `.agents/skills`: where it looks instead.
pub struct Agent {
    pub name: &'static str,
    /// Its directory under the home directory; present means it is installed.
    pub detect: &'static str,
    /// Its project-level skills directory, if it does not read `.agents/skills`.
    pub project: Option<&'static str>,
    /// Its user-level skills directory (under the home directory), if not `~/.agents/skills`.
    pub user: Option<&'static str>,
}

/// Agents needing a link; every other agent reads `.agents/skills` (see the module docs).
pub const AGENTS: &[Agent] = &[
    Agent { name: "claude", detect: ".claude", project: Some(".claude/skills"), user: Some(".claude/skills") },
    Agent { name: "kiro", detect: ".kiro", project: Some(".kiro/skills"), user: Some(".kiro/skills") },
    Agent { name: "amp", detect: ".config/amp", project: None, user: Some(".config/agents/skills") },
];

pub struct Install {
    /// The project directory (ignored for `global`).
    pub project: PathBuf,
    pub home: PathBuf,
    /// Into the home directory, for every project.
    pub global: bool,
    /// Link for these agents only; `None` links for the agents found in `home`.
    pub agents: Option<Vec<String>>,
    /// Write the pointer block into the project's `AGENTS.md`.
    pub pointer: bool,
}

/// What an install did, one line per change.
pub type Report = Vec<String>;

pub const POINTER_START: &str = "<!-- octara:start -->";
pub const POINTER_END: &str = "<!-- octara:end -->";

/// The block an installed project's `AGENTS.md` carries.
pub fn pointer() -> String {
    format!(
        "{POINTER_START}\n## Octara\n\nThis project's UI is built with Octara (Rust; native desktop and mobile, not web). Before writing \
         or changing UI, read the `octara` skill: `.agents/skills/octara/SKILL.md`, then its `references/start-here.md`. Look names \
         up in `references/api/` or with `octara vocab <name>`, and diagnostic codes with `octara explain <CODE>` instead of guessing.\n{POINTER_END}\n"
    )
}

/// `text` with the pointer block added or replaced.
pub fn with_pointer(text: &str) -> String {
    let block = pointer();
    match (text.find(POINTER_START), text.find(POINTER_END)) {
        (Some(s), Some(e)) if e > s => format!("{}{block}{}", &text[..s], text[e + POINTER_END.len()..].trim_start_matches('\n')),
        _ if text.is_empty() => block,
        _ => format!("{}\n\n{block}", text.trim_end()),
    }
}

/// Installs the skill: the canonical copy, links for agents that look elsewhere, and the pointer.
pub fn install(api: &Api, opts: &Install) -> Result<Report, String> {
    let api = Some(api);
    let mut report = Vec::new();
    let base = if opts.global { opts.home.clone() } else { opts.project.clone() };
    let canonical = base.join(".agents/skills").join(NAME);
    guard(&canonical)?;
    if !stale(api, &canonical).is_empty() {
        write_files(api, &canonical)?;
        report.push(format!("wrote {}", canonical.display()));
    }

    let wanted: Vec<&Agent> = match &opts.agents {
        Some(names) => {
            let mut v = Vec::new();
            for n in names {
                match AGENTS.iter().find(|a| a.name == n) {
                    Some(a) => v.push(a),
                    None if n == "agents" => {}
                    None => {
                        return Err(format!(
                            "unknown agent `{n}`: {} need a link; the others read .agents/skills",
                            AGENTS.iter().map(|a| a.name).collect::<Vec<_>>().join(", ")
                        ));
                    }
                }
            }
            v
        }
        None => AGENTS.iter().filter(|a| opts.home.join(a.detect).is_dir()).collect(),
    };
    for a in wanted {
        let dir = if opts.global { a.user.map(|d| opts.home.join(d)) } else { a.project.map(|d| opts.project.join(d)) };
        let Some(dir) = dir else { continue };
        let at = dir.join(NAME);
        if link(api, &canonical, &at)? {
            report.push(format!("linked {} ({})", at.display(), a.name));
        }
    }

    if opts.pointer && !opts.global {
        let agents_md = opts.project.join("AGENTS.md");
        let old = std::fs::read_to_string(&agents_md).unwrap_or_default();
        let new = with_pointer(&old);
        if new != old {
            std::fs::write(&agents_md, &new).map_err(|e| format!("{}: {e}", agents_md.display()))?;
            report.push(format!("updated {}", agents_md.display()));
        }
        // Claude Code reads CLAUDE.md instead of AGENTS.md when both exist.
        let claude_md = opts.project.join("CLAUDE.md");
        if let Ok(text) = std::fs::read_to_string(&claude_md)
            && !text.lines().any(|l| l.trim() == "@AGENTS.md")
        {
            std::fs::write(&claude_md, format!("{}\n\n@AGENTS.md\n", text.trim_end()))
                .map_err(|e| format!("{}: {e}", claude_md.display()))?;
            report.push(format!("updated {} (imports AGENTS.md)", claude_md.display()));
        }
    }
    Ok(report)
}

/// Refuses to overwrite a directory that is not an `octara` skill.
fn guard(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    let ours = std::fs::read_to_string(dir.join("SKILL.md")).is_ok_and(|t| t.contains(&format!("\nname: {NAME}\n")));
    if ours { Ok(()) } else { Err(format!("{} exists and is not the octara skill; move it away first", dir.display())) }
}

/// Points `at` at `canonical`: a relative symlink, or a copy where links are not possible.
/// Returns whether anything changed.
fn link(api: Option<&Api>, canonical: &Path, at: &Path) -> Result<bool, String> {
    let parent = at.parent().expect("a skills directory");
    let target = relative(canonical, parent);
    if std::fs::read_link(at).is_ok_and(|t| t == target) {
        return Ok(false);
    }
    if at.is_symlink() {
        std::fs::remove_file(at).map_err(|e| format!("{}: {e}", at.display()))?;
    } else if at.exists() {
        guard(at)?;
        // A copy from an earlier install where links failed: refresh it in place.
        if stale(api, at).is_empty() {
            return Ok(false);
        }
        write_files(api, at)?;
        return Ok(true);
    }
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&target, at).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(&target, at).is_ok();
    if !linked {
        write_files(api, at)?;
    }
    Ok(true)
}

/// `to` relative to the directory `from` (both under one root).
fn relative(to: &Path, from: &Path) -> PathBuf {
    let to: Vec<_> = to.components().collect();
    let from: Vec<_> = from.components().collect();
    let common = to.iter().zip(&from).take_while(|(a, b)| a == b).count();
    let mut p = PathBuf::new();
    for _ in common..from.len() {
        p.push("..");
    }
    for c in &to[common..] {
        p.push(c);
    }
    p
}
