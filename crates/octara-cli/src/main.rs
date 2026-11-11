//! `octara`: the framework's API, usage and diagnostics, for agents and people.

use clap::{Parser, Subcommand};
use octara_cli::api;
use octara_cli::catalog::{Catalog, Entry, compress};
use std::fmt::Write as _;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "octara", version, about = "The Octara framework's API, usage and diagnostics.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Look up how to use something: a function, method, type, token, spring, command or code.
    ///
    /// Takes a full path (`octara::view::Styled::bg`), a path suffix (`Styled::bg`, `bg`), a
    /// diagnostic code or a built-in command (`focus.next`). Without a name, lists the prelude.
    Vocab {
        name: Option<String>,
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Explain a diagnostic code: what it means, why it happens, how to fix it.
    ///
    /// Takes a code (`OVERFLOW`) or any text containing one (a message, a panic). Without one,
    /// lists every code.
    Explain {
        code: Option<String>,
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Print the compressed index of every name an app can use (the skill's `references/index.md`).
    Index,
    /// The `octara` skill for coding agents: install it into a project, or export it.
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Regenerate `api.json` from the source (needs a nightly toolchain for rustdoc's JSON).
    Api {
        /// Only check that `api.json` matches the source; exit 1 if it is out of date.
        #[arg(long)]
        check: bool,
    },
}

#[derive(Subcommand)]
enum SkillAction {
    /// Install the skill: `.agents/skills/octara/` (read by Codex, Cursor, Copilot, Gemini CLI,
    /// OpenCode, Amp, Windsurf, Zed, Junie, Goose), links for Claude Code and Kiro, and a pointer
    /// block in the project's `AGENTS.md`. Run again to update.
    Install {
        /// The project directory.
        #[arg(long, default_value = ".")]
        dir: std::path::PathBuf,
        /// Install into the home directory (`~/.agents/skills`) for every project instead.
        #[arg(long)]
        global: bool,
        /// Link for these agents only (claude, kiro, amp); by default for those found installed.
        #[arg(long = "agent", value_name = "AGENT")]
        agents: Vec<String>,
        /// Leave `AGENTS.md` alone.
        #[arg(long)]
        no_pointer: bool,
        /// Only check that the installed skill is this version's; exit 1 if not.
        #[arg(long)]
        check: bool,
    },
    /// Write the skill into `<dir>/octara/`.
    Export { dir: std::path::PathBuf },
}

fn main() -> ExitCode {
    let mut out = String::new();
    let code = match Cli::parse().command {
        Command::Vocab { name, json } => vocab(&mut out, name.as_deref(), json),
        Command::Explain { code, json } => explain(&mut out, code.as_deref(), json),
        Command::Index => {
            out += &catalog().index();
            ExitCode::SUCCESS
        }
        Command::Skill { action } => skill(&mut out, action),
        Command::Api { check } => regenerate(check),
    };
    // Written at once: a reader that stops early (`| head`) is not an error.
    let _ = std::io::Write::write_all(&mut std::io::stdout().lock(), out.as_bytes());
    code
}

/// The first sentence of a summary.
fn sentence(s: &str) -> &str {
    s.find(". ").map(|i| &s[..=i]).unwrap_or(s)
}

fn catalog() -> Catalog {
    Catalog::build(&api::load())
}

fn json_to(out: &mut String, v: &impl serde::Serialize) {
    *out += &serde_json::to_string_pretty(v).expect("serializes");
    *out += "\n";
}

/// One entry as text: path and kind, signature, summary, facts, docs, related, members.
fn render(e: &Entry, full: bool) -> String {
    let mut s = String::new();
    let _ = write!(s, "{}  ({})", e.path, e.kind);
    if e.prelude {
        s += "  [prelude]";
    }
    s += "\n";
    if !e.within.is_empty() {
        let _ = writeln!(s, "  in {}", e.within);
    }
    if !e.sig.is_empty() {
        let _ = writeln!(s, "  {}", e.sig);
    }
    if !e.summary.is_empty() {
        let _ = writeln!(s, "  {}", e.summary);
    }
    for (k, v) in &e.facts {
        let _ = writeln!(s, "  {k}: {v}");
    }
    if full {
        let rest = e.docs.split_once("\n\n").map(|(_, r)| r.trim()).unwrap_or("");
        if !rest.is_empty() {
            s += "\n";
            for l in rest.lines() {
                let _ = writeln!(s, "  {l}");
            }
            s += "\n";
        }
        if !e.members.is_empty() {
            let _ = writeln!(s, "  members ({}): {}", e.members.len(), compress(&e.members).join(", "));
        }
        if !e.related.is_empty() {
            let _ = writeln!(s, "  see: {}", e.related.join(", "));
        }
        if !e.file.is_empty() {
            let _ = writeln!(s, "  defined in {}", e.file);
        }
    }
    s
}

fn vocab(out: &mut String, name: Option<&str>, json: bool) -> ExitCode {
    let cat = catalog();
    let Some(name) = name else {
        let prelude: Vec<&Entry> = cat.prelude().collect();
        if json {
            json_to(out, &prelude);
        } else {
            *out += "The prelude (`use octara::prelude::*`); `octara vocab <name>` for any of them:\n\n";
            for e in prelude {
                let _ = writeln!(out, "{:<10} {:<20} {}", e.kind, e.name(), sentence(&e.summary));
            }
        }
        return ExitCode::SUCCESS;
    };
    let found = cat.lookup(name);
    if json {
        json_to(out, &found);
    } else if found.matches.is_empty() {
        let _ = writeln!(out, "nothing is named `{name}`");
        if !found.similar.is_empty() {
            let _ = writeln!(out, "similar: {}", found.similar.join(", "));
        }
    } else {
        let full = found.matches.len() <= 3;
        for e in &found.matches {
            let _ = writeln!(out, "{}", render(e, full));
        }
        if !found.similar.is_empty() {
            let _ = writeln!(out, "also: {}", found.similar.join(", "));
        }
    }
    if found.matches.is_empty() { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

fn explain(out: &mut String, code: Option<&str>, json: bool) -> ExitCode {
    let cat = catalog();
    let Some(code) = code else {
        let codes: Vec<&Entry> = cat.entries.iter().filter(|e| e.kind == "diagnostic").collect();
        if json {
            json_to(out, &codes);
        } else {
            for e in codes {
                let _ = writeln!(out, "{:<28} {:<8} {}", e.path, e.facts["severity"], e.summary);
            }
        }
        return ExitCode::SUCCESS;
    };
    match cat.code_in(code) {
        Some(e) if json => json_to(out, e),
        Some(e) => {
            let _ = writeln!(out, "{}  ({}, {})", e.path, e.facts["severity"], e.facts["area"]);
            let _ = writeln!(out, "  {}", e.summary);
            let _ = writeln!(out, "  cause: {}", e.facts["cause"]);
            let _ = writeln!(out, "  fix: {}", e.facts["fix"]);
            let _ = writeln!(out, "  docs: {}", e.facts["doc"]);
            if !e.related.is_empty() {
                let _ = writeln!(out, "  see: {}", e.related.join(", "));
            }
        }
        None => {
            eprintln!("no registered code in `{code}`; `octara explain` lists them");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

fn skill(out: &mut String, action: SkillAction) -> ExitCode {
    use octara_cli::skill;
    let cat = catalog();
    let result = match action {
        SkillAction::Export { dir } => skill::export(&cat, &dir).map(|root| vec![format!("wrote {}", root.display())]),
        SkillAction::Install { dir, global, agents, no_pointer, check } => {
            let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(std::path::PathBuf::from) else {
                eprintln!("error: no home directory (HOME)");
                return ExitCode::FAILURE;
            };
            if check {
                let root = if global { home } else { dir }.join(".agents/skills").join(skill::NAME);
                let stale = skill::stale(&cat, &root);
                if stale.is_empty() {
                    let _ = writeln!(out, "{} is current", root.display());
                    return ExitCode::SUCCESS;
                }
                let _ = writeln!(out, "{} is out of date ({}); run `octara skill install`", root.display(), stale.join(", "));
                return ExitCode::FAILURE;
            }
            let opts = skill::Install { project: dir, home, global, agents: (!agents.is_empty()).then_some(agents), pointer: !no_pointer };
            skill::install(&cat, &opts)
        }
    };
    match result {
        Ok(report) if report.is_empty() => *out += "already installed and current\n",
        Ok(report) => {
            for l in report {
                let _ = writeln!(out, "{l}");
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

fn regenerate(check: bool) -> ExitCode {
    let fresh = match api::generate() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let text = api::to_json(&fresh);
    let path = api::checked_in_path();
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    if current == text {
        eprintln!("api.json is up to date ({} items)", fresh.items().count());
        return ExitCode::SUCCESS;
    }
    if check {
        if let Ok(old) = serde_json::from_str::<api::Api>(&current) {
            let (added, removed, changed) = api::diff(&old, &fresh);
            eprintln!("api.json is out of date: {} added, {} removed, {} changed", added.len(), removed.len(), changed.len());
        } else {
            eprintln!("api.json is out of date");
        }
        eprintln!("run `cargo run -p octara-cli -- api` to regenerate it");
        return ExitCode::FAILURE;
    }
    if let Err(e) = std::fs::write(&path, &text) {
        eprintln!("error: {}: {e}", path.display());
        return ExitCode::FAILURE;
    }
    eprintln!("wrote {} ({} items)", path.display(), fresh.items().count());
    ExitCode::SUCCESS
}
