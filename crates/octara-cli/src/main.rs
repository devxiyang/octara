//! `octara`: the framework's API, usage and diagnostics, for agents and people.

use clap::{Parser, Subcommand};
use octara_cli::api;
use octara_cli::catalog::{Catalog, Entry};
use std::fmt::Write as _;
use std::path::Path;
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
    /// Explain a diagnostic code: what it means and why it happens.
    ///
    /// Takes a code (`OVERFLOW`) or any text containing one (a message, a panic). Without one,
    /// lists every code.
    Explain {
        code: Option<String>,
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Check the app: at light and dark, phone and desktop sizes, report layout issues,
    /// diagnostics, work that never settles and accessibility problems. Exit 0 when clean, 1 with
    /// findings, 2 when the app does not build or run.
    ///
    /// The app names its root view in Cargo.toml: `[package.metadata.octara] view = "my_app::app"`.
    Check {
        #[command(flatten)]
        app: AppArgs,
        /// Print the report as JSON (`octara.check/1`).
        #[arg(long)]
        json: bool,
    },
    /// Print the app's semantic tree after its first screen settles: test ids, roles, names,
    /// states, bounds, actions and source locations.
    Inspect {
        #[command(flatten)]
        app: AppArgs,
        #[command(flatten)]
        window: WindowArgs,
        /// Print the snapshot as JSON (`octara.inspect/1`).
        #[arg(long)]
        json: bool,
    },
    /// Drive the app with requests, one JSON line each (`{"op":"tap","id":"save"}`): from a
    /// script (stopping at the first failure), or interactively from stdin.
    Run {
        #[command(flatten)]
        app: AppArgs,
        #[command(flatten)]
        window: WindowArgs,
        /// A file of requests, one JSON line each (`//` comments allowed).
        #[arg(long)]
        script: Option<std::path::PathBuf>,
        /// Print the replies as JSON lines.
        #[arg(long)]
        json: bool,
    },
    /// The `octara` skill for coding agents: install it into a project, or export it.
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
}

/// Which app: the package naming `[package.metadata.octara] view`.
#[derive(clap::Args)]
struct AppArgs {
    /// The app's Cargo.toml (default: the current directory's workspace).
    #[arg(long)]
    manifest_path: Option<std::path::PathBuf>,
    /// The package, when several in the workspace name a view.
    #[arg(long, short)]
    package: Option<String>,
}

#[derive(clap::Args)]
struct WindowArgs {
    /// Window size in logical pixels, e.g. `390x844`.
    #[arg(long)]
    size: Option<String>,
    /// `light` or `dark`.
    #[arg(long)]
    theme: Option<String>,
}

impl WindowArgs {
    fn args(&self) -> Vec<String> {
        let mut a = Vec::new();
        if let Some(s) = &self.size {
            a.extend(["--size".to_string(), s.clone()]);
        }
        if let Some(t) = &self.theme {
            a.extend(["--theme".to_string(), t.clone()]);
        }
        a
    }
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
        Command::Check { app, json } => check(&mut out, &app, json),
        Command::Inspect { app, window, json } => inspect(&mut out, &app, &window, json),
        Command::Run { app, window, script, json } => run(&mut out, &app, &window, script.as_deref(), json),
        Command::Skill { action } => skill(&mut out, action),
    };
    // Written at once: a reader that stops early (`| head`) is not an error.
    let _ = std::io::Write::write_all(&mut std::io::stdout().lock(), out.as_bytes());
    code
}

/// The API of the octara the current project depends on, with the kernel's tables.
fn api_here() -> api::Api {
    api::for_project(Path::new(".")).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(2)
    })
}

fn catalog() -> Catalog {
    Catalog::build(&api_here())
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
            let _ = writeln!(s, "  members ({}): {}", e.members.len(), e.members.join(", "));
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
                let _ = writeln!(out, "{:<10} {:<20} {}", e.kind, e.name(), octara_cli::catalog::first_sentence(&e.summary));
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

/// Finds and builds the app's harness; prints the error and gives exit 2 on failure.
fn harness(app: &AppArgs) -> Result<(octara_cli::harness::Target, std::path::PathBuf), ExitCode> {
    use octara_cli::harness;
    let built = harness::find(app.manifest_path.as_deref(), app.package.as_deref()).and_then(|t| harness::build(&t).map(|exe| (t, exe)));
    match built {
        Ok(built) => Ok(built),
        Err(e) => {
            eprintln!("error: {e}");
            Err(ExitCode::from(2))
        }
    }
}

fn check(out: &mut String, app: &AppArgs, json: bool) -> ExitCode {
    let (target, exe) = match harness(app) {
        Ok(e) => e,
        Err(code) => return code,
    };
    let args = vec!["check".to_string(), "--options".into(), target.options.to_string()];
    let (ok, stdout, stderr) = match octara_cli::harness::run(&exe, &args, None) {
        Ok((ok, stdout, stderr)) => (ok, octara_cli::harness::relative_sources(&target, &stdout), stderr),
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let report: serde_json::Value = match serde_json::from_str(&stdout) {
        Ok(v) if ok => v,
        _ => {
            eprintln!("error: the app's check did not finish:\n{stderr}");
            return ExitCode::from(2);
        }
    };
    if json {
        *out += &stdout;
    } else {
        *out += &check_text(&report);
    }
    if report["clean"] == true { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// A check report as text, one block per finding, ending `check: clean` or `check: N problems`.
fn check_text(r: &serde_json::Value) -> String {
    let mut s = String::new();
    let str_of = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
    let findings = r["findings"].as_array().cloned().unwrap_or_default();
    for f in &findings {
        let cells: Vec<String> = f["cells"].as_array().into_iter().flatten().map(str_of).collect();
        let id = f["id"].as_str().map(|i| format!(" #{i}")).unwrap_or_default();
        let _ = writeln!(s, "{} {}{id} — {}", str_of(&f["severity"]), str_of(&f["code"]), cells.join(", "));
        let _ = writeln!(s, "  {}", str_of(&f["message"]));
        let cause = str_of(&f["cause"]);
        if !cause.is_empty() {
            let _ = writeln!(s, "  cause: {cause}");
        }
        if let Some(src) = f["source"].as_str() {
            let _ = writeln!(s, "  at {src}");
        }
    }
    for a in r["accepted"].as_array().into_iter().flatten() {
        let id = a["id"].as_str().map(|i| format!(" #{i}")).unwrap_or_default();
        let _ = writeln!(s, "accepted {}{id}: {}", str_of(&a["code"]), str_of(&a["why"]));
    }
    let problems = findings.iter().filter(|f| f["severity"] != "info").count();
    let cells: Vec<String> = r["cells"].as_array().into_iter().flatten().map(str_of).collect();
    if problems == 0 {
        let _ = writeln!(s, "check: clean ({})", cells.join(", "));
    } else {
        let _ = writeln!(
            s,
            "check: {problems} problem{} ({}); `octara explain <CODE>` for any code",
            if problems == 1 { "" } else { "s" },
            cells.join(", ")
        );
    }
    s
}

fn inspect(out: &mut String, app: &AppArgs, window: &WindowArgs, json: bool) -> ExitCode {
    let (target, exe) = match harness(app) {
        Ok(e) => e,
        Err(code) => return code,
    };
    let mut args = vec!["inspect".to_string()];
    args.extend(window.args());
    let result = octara_cli::harness::run(&exe, &args, None).map(|(ok, o, e)| (ok, octara_cli::harness::relative_sources(&target, &o), e));
    match result {
        Ok((true, stdout, _)) if json => *out += &stdout,
        Ok((true, stdout, _)) => match serde_json::from_str::<serde_json::Value>(&stdout) {
            Ok(snap) => *out += &snapshot_text(&snap),
            Err(e) => {
                eprintln!("error: unreadable snapshot: {e}");
                return ExitCode::from(2);
            }
        },
        Ok((false, _, stderr)) | Err(stderr) => {
            eprintln!("error: {stderr}");
            return ExitCode::from(2);
        }
    }
    ExitCode::SUCCESS
}

/// A snapshot as an indented tree: `role #id "name" value=.. [states] {actions} x,y w×h`.
fn snapshot_text(snap: &serde_json::Value) -> String {
    fn node(n: &serde_json::Value, depth: usize, s: &mut String) {
        let _ = write!(s, "{:indent$}{}", "", n["role"].as_str().unwrap_or("?"), indent = depth * 2);
        if let Some(id) = n["id"].as_str() {
            let _ = write!(s, " #{id}");
        }
        if let Some(name) = n["name"].as_str() {
            let _ = write!(s, " {name:?}");
        }
        if let Some(v) = n["value"].as_str() {
            let _ = write!(s, " value={v:?}");
        }
        if let Some(c) = n["checked"].as_bool() {
            let _ = write!(s, " checked={c}");
        }
        let list = |k: &str| n[k].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(" ")).unwrap_or_default();
        let (states, actions) = (list("states"), list("actions"));
        if !states.is_empty() {
            let _ = write!(s, " [{states}]");
        }
        if !actions.is_empty() {
            let _ = write!(s, " {{{actions}}}");
        }
        let b: Vec<f64> = n["bounds"].as_array().into_iter().flatten().filter_map(|v| v.as_f64()).collect();
        if b.len() == 4 {
            let _ = write!(s, " {},{} {}×{}", b[0], b[1], b[2], b[3]);
        }
        s.push('\n');
        for c in n["children"].as_array().into_iter().flatten() {
            node(c, depth + 1, s);
        }
    }
    let mut s = String::new();
    let v = &snap["viewport"];
    let _ = writeln!(
        s,
        "{} {}×{} {} theme, focus: {}, pending: {}",
        snap["schema"].as_str().unwrap_or_default(),
        v[0],
        v[1],
        snap["theme"].as_str().unwrap_or_default(),
        snap["focus"].as_str().unwrap_or("none"),
        snap["pending"]
    );
    node(&snap["root"], 0, &mut s);
    s
}

fn run(out: &mut String, app: &AppArgs, window: &WindowArgs, script: Option<&std::path::Path>, json: bool) -> ExitCode {
    let (target, exe) = match harness(app) {
        Ok(e) => e,
        Err(code) => return code,
    };
    let mut args = vec!["run".to_string()];
    args.extend(window.args());
    let Some(script) = script else {
        // Interactive: requests from our stdin, replies straight to our stdout.
        let status = std::process::Command::new(&exe).args(&args).status();
        return match status {
            Ok(s) if s.success() => ExitCode::SUCCESS,
            Ok(_) => ExitCode::FAILURE,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(2)
            }
        };
    };
    let text = match std::fs::read_to_string(script) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {}: {e}", script.display());
            return ExitCode::from(2);
        }
    };
    args.push("--stop".into());
    let (ok, stdout, stderr) = match octara_cli::harness::run(&exe, &args, Some(&text)) {
        Ok((ok, stdout, stderr)) => (ok, octara_cli::harness::relative_sources(&target, &stdout), stderr),
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    if !ok {
        eprintln!("error: the app stopped:\n{stderr}");
        return ExitCode::from(2);
    }
    let replies: Vec<serde_json::Value> = stdout.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let failed = replies.iter().any(|r| r["ok"] == false);
    if json {
        *out += &stdout;
    } else {
        for r in &replies {
            let line = &r["line"];
            if r["ok"] == true {
                let _ = writeln!(out, "line {line}: ok");
            } else {
                let _ = writeln!(out, "line {line}: {} — {}", r["code"].as_str().unwrap_or("?"), r["message"].as_str().unwrap_or_default());
            }
            if let Some(snap) = r.get("snapshot") {
                *out += &snapshot_text(snap);
            }
            for i in r["issues"].as_array().into_iter().flatten() {
                let _ = writeln!(
                    out,
                    "  {} {}: {}",
                    i["severity"].as_str().unwrap_or_default(),
                    i["code"].as_str().unwrap_or_default(),
                    i["message"].as_str().unwrap_or_default()
                );
            }
        }
        let _ = writeln!(out, "run: {}", if failed { "failed" } else { "ok" });
    }
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

fn skill(out: &mut String, action: SkillAction) -> ExitCode {
    use octara_cli::skill;
    let result = match action {
        SkillAction::Export { dir } => skill::export(&dir).map(|root| vec![format!("wrote {}", root.display())]),
        SkillAction::Install { dir, global, agents, no_pointer, check } => {
            let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(std::path::PathBuf::from) else {
                eprintln!("error: no home directory (HOME)");
                return ExitCode::FAILURE;
            };
            let api = match api::for_project(&dir) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            if check {
                let root = if global { home } else { dir }.join(".agents/skills").join(skill::NAME);
                let stale = skill::stale(Some(&api), &root);
                if stale.is_empty() {
                    let _ = writeln!(out, "{} is current", root.display());
                    return ExitCode::SUCCESS;
                }
                let _ = writeln!(out, "{} is out of date ({}); run `octara skill install`", root.display(), stale.join(", "));
                return ExitCode::FAILURE;
            }
            let opts = skill::Install { project: dir, home, global, agents: (!agents.is_empty()).then_some(agents), pointer: !no_pointer };
            skill::install(&api, &opts)
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
