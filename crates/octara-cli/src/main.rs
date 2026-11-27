//! `octara`: the framework's API, usage and diagnostics, for agents and people.

use clap::{Parser, Subcommand};
use octara_cli::api;
use octara_cli::catalog::{Catalog, Entry};
use octara_cli::harness;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod audit;
#[cfg(unix)]
mod dev;
mod launch;
mod modules;
mod receipt;
mod release;
mod session;
mod web;

#[derive(Parser)]
#[command(name = "octara", version, about = "The Octara framework's API, usage and diagnostics.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Look up a function, method, type, token, spring or diagnostic code.
    ///
    /// Takes a full path (`octara::view::Styled::bg`), a path suffix (`Styled::bg`, `bg`), or a
    /// diagnostic code. Without a name, lists the prelude.
    Vocab {
        name: Option<String>,
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Explain a diagnostic code: what it means and why it happens.
    ///
    /// Takes a code (`STALE_TARGET`) or any text containing one (a message, a panic). Without one,
    /// lists every code.
    Explain {
        code: Option<String>,
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Check runtime diagnostics and settlement at light and dark, phone and desktop sizes.
    /// Exit 0 when clean, 1 with findings, or 2 when the app fails to build or run.
    ///
    /// The app names its startup function under `[package.metadata.octara]` in Cargo.toml
    /// with `entry = "my_app::startup"`.
    Check {
        #[command(flatten)]
        app: AppArgs,
        /// Print the report as JSON (`octara.check/2`).
        #[arg(long)]
        json: bool,
    },
    /// Observe the app's semantic tree, geometry and frame progress.
    /// Builds a headless harness by default. --connect reads an existing window.
    Inspect {
        #[command(flatten)]
        app: AppArgs,
        #[command(flatten)]
        window: WindowArgs,
        /// Inspect an existing window at its OCTARA_INSPECT Unix socket, without building a harness.
        #[arg(long, value_name = "SOCKET", conflicts_with_all = ["manifest_path", "package", "size", "theme"])]
        connect: Option<std::path::PathBuf>,
        /// Compare the connected window with a receipt from `octara launch`.
        #[arg(long, value_name = "FILE", requires = "connect")]
        receipt: Option<std::path::PathBuf>,
        /// Select a window using its id from a list_windows receipt.
        #[arg(long)]
        window_id: Option<u64>,
        /// Print the complete octara.inspect/2 receipt as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Audit the app's settled windows for text clipping, accessibility names and focus targets.
    /// Exit 1 for errors in the report or 2 if the audit cannot run.
    Audit {
        #[command(flatten)]
        app: AppArgs,
        #[command(flatten)]
        window: WindowArgs,
        /// Audit an existing window at its OCTARA_INSPECT Unix socket.
        #[arg(long, value_name = "SOCKET", conflicts_with_all = ["manifest_path", "package", "size", "theme"])]
        connect: Option<PathBuf>,
        /// Audit only this window id, as returned by list_windows.
        #[arg(long)]
        window_id: Option<u64>,
        /// Print the report as JSON (`octara.audit/1`).
        #[arg(long)]
        json: bool,
    },
    /// Drive the app with octara.inspect/2 requests, one per JSON line.
    /// Scripts stop at the first unsuccessful receipt. Stdin receives a reply after each request.
    Run {
        #[command(flatten)]
        app: AppArgs,
        #[command(flatten)]
        window: WindowArgs,
        /// Drive an existing window at its OCTARA_INSPECT Unix socket, without building a harness.
        #[arg(long, value_name = "SOCKET", conflicts_with_all = ["manifest_path", "package", "size", "theme"])]
        connect: Option<std::path::PathBuf>,
        /// Check the running build before sending script requests.
        #[arg(long, value_name = "RECEIPT", requires = "connect")]
        require_build: Option<std::path::PathBuf>,
        /// A file of requests, one JSON line each (`//` comments allowed).
        #[arg(long)]
        script: Option<std::path::PathBuf>,
        /// Print the replies as JSON lines.
        #[arg(long)]
        json: bool,
    },
    /// Build and run the app's declared entry with a native Inspector socket and build receipt.
    Launch {
        #[command(flatten)]
        app: AppArgs,
        /// An absolute, unused path for the local Unix socket.
        #[arg(long)]
        socket: PathBuf,
        /// Write a build receipt to an unused path.
        #[arg(long)]
        receipt: PathBuf,
        /// Require Cargo.lock to stay unchanged.
        #[arg(long)]
        locked: bool,
    },
    /// Watch Rust sources and restart the app with its persisted state after a successful rebuild.
    Dev {
        #[command(flatten)]
        app: AppArgs,
        /// Require Cargo.lock to stay unchanged.
        #[arg(long)]
        locked: bool,
    },
    /// Merge platform metadata declared by the app's Octara module dependencies.
    Modules {
        #[command(subcommand)]
        action: modules::Action,
    },
    /// Publish or select a complete version of a standalone Web app.
    Web {
        #[command(subcommand)]
        action: web::Action,
    },
    /// Prepare signed resource packages for an installed native application.
    Release {
        #[command(subcommand)]
        action: release::Action,
    },
    /// The `octara` skill for coding agents: install it into a project, or export it.
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
    #[command(hide = true)]
    ExportApi { path: PathBuf },
}

/// Which app to build from `[package.metadata.octara] entry`.
#[derive(clap::Args)]
struct AppArgs {
    /// The app's Cargo.toml (default: the current directory's workspace).
    #[arg(long)]
    manifest_path: Option<std::path::PathBuf>,
    /// The package, when several in the workspace name an Octara startup entry.
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
    fn options(&self) -> Result<serde_json::Value, String> {
        let size = self
            .size
            .as_deref()
            .map(|size| {
                let (width, height) = size.split_once('x').ok_or_else(|| format!("expected WIDTHxHEIGHT, got `{size}`"))?;
                let dimension = |text: &str| {
                    text.parse::<f32>().ok().filter(|n| n.is_finite() && *n > 0.0).ok_or_else(|| format!("invalid window size `{size}`"))
                };
                Ok::<_, String>(octara_model::geometry::Size::new(dimension(width)?, dimension(height)?))
            })
            .transpose()?;
        if let Some(theme) = &self.theme
            && !["light", "dark"].contains(&theme.as_str())
        {
            return Err(format!("unknown theme `{theme}`"));
        }
        Ok(serde_json::json!({ "size": size, "theme": self.theme }))
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
        Command::Inspect { app, window, connect, receipt, window_id, json } => {
            result(session::inspect(&mut out, &app, &window, connect.as_deref(), receipt.as_deref(), window_id, json))
        }
        Command::Audit { app, window, connect, window_id, json } => {
            result(audit::run(&mut out, &app, &window, connect.as_deref(), window_id, json))
        }
        Command::Run { app, window, connect, require_build, script, json } => {
            result(session::run(&mut out, &app, &window, connect.as_deref(), require_build.as_deref(), script.as_deref(), json))
        }
        Command::Launch { app, socket, receipt, locked } => result(launch::launch(&app, &socket, &receipt, locked)),
        Command::Dev { app, locked } => {
            #[cfg(unix)]
            let run = dev::run(&app, locked);
            #[cfg(not(unix))]
            let run = {
                let _ = (app, locked);
                Err("dev requires Unix domain sockets".into())
            };
            result(run)
        }
        Command::Modules { action } => result(action.run(&mut out).map(|()| ExitCode::SUCCESS)),
        Command::Web { action } => match action.run() {
            Ok(release) => {
                writeln!(out, "{release}").expect("a String can be written");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::from(2)
            }
        },
        Command::Release { action } => match action.run(&mut out) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::from(2)
            }
        },
        Command::Skill { action } => skill(&mut out, action),
        Command::ExportApi { path } => result(api::export_bundle(&path).map(|()| ExitCode::SUCCESS)),
    };
    // Written at once: a reader that stops early (`| head`) is not an error.
    let _ = std::io::Write::write_all(&mut std::io::stdout().lock(), out.as_bytes());
    code
}

/// The API of the Octara build the current project uses.
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
            let _ = writeln!(out, "{}  ({})", e.path, e.facts["severity"]);
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

impl AppArgs {
    fn target(&self) -> Result<harness::Target, String> {
        harness::find(self.manifest_path.as_deref(), self.package.as_deref())
    }
}

fn result(result: Result<ExitCode, String>) -> ExitCode {
    result.unwrap_or_else(|error| {
        eprintln!("error: {error}");
        ExitCode::from(2)
    })
}

fn check(out: &mut String, app: &AppArgs, json: bool) -> ExitCode {
    result((|| {
        let target = app.target()?;
        let exe = harness::build(&target)?;
        let output = std::process::Command::new(&exe)
            .args(["check", &target.options.to_string()])
            .output()
            .map_err(|error| format!("{}: {error}", exe.display()))?;
        if !matches!(output.status.code(), Some(0 | 1)) {
            return Err(format!("the app's check did not finish:\n{}", String::from_utf8_lossy(&output.stderr)));
        }
        let report: serde_json::Value =
            serde_json::from_slice(&output.stdout).map_err(|error| format!("unreadable check report: {error}"))?;
        if json {
            json_to(out, &report);
        } else {
            *out += &check_text(&report);
        }
        Ok(if output.status.success() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
    })())
}

/// A check report as text, one block per finding, ending `check: clean` or `check: N problems`.
fn check_text(r: &serde_json::Value) -> String {
    let mut s = String::new();
    let str_of = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
    let findings = r["findings"].as_array().map_or(&[][..], Vec::as_slice);
    for f in findings {
        let cells: Vec<String> = f["cells"].as_array().into_iter().flatten().map(str_of).collect();
        let _ = writeln!(s, "{} {} — {}", str_of(&f["severity"]), str_of(&f["code"]), cells.join(", "));
        let _ = writeln!(s, "  {}", str_of(&f["message"]));
        if let Some(src) = f["source"].as_str() {
            let _ = writeln!(s, "  at {src}");
        }
    }
    for a in r["accepted"].as_array().into_iter().flatten() {
        let _ = writeln!(s, "accepted {}: {}", str_of(&a["code"]), str_of(&a["why"]));
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
