use crate::{AppArgs, WindowArgs, json_to, receipt};
use octara_cli::{harness, live};
use octara_inspect::{AgentCommand, AgentReceipt, AgentRequest, protocol::WindowRef};
use octara_model::diagnostics::codes;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, ExitCode, Stdio};

pub(crate) enum Session {
    Live(live::Client),
    Headless(Headless),
}

impl Session {
    pub(crate) fn open(app: &AppArgs, options: &WindowArgs, connect: Option<&Path>) -> Result<Self, String> {
        if let Some(path) = connect {
            return live::Client::connect(path).map(Self::Live);
        }
        let options = options.options()?;
        let exe = harness::build(&app.target()?)?;
        let mut child = Command::new(&exe)
            .args(["run", &options.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|error| format!("{}: {error}", exe.display()))?;
        let output = BufReader::new(child.stdout.take().expect("piped stdout"));
        Ok(Self::Headless(Headless { child, output }))
    }

    fn request(&mut self, line: &str) -> Result<AgentReceipt, String> {
        match self {
            Session::Live(client) => client.request(line),
            Session::Headless(host) => {
                live::write_request(host.child.stdin.as_mut().expect("piped stdin"), line)?;
                live::read_reply(&mut host.output)
            }
        }
    }

    fn observe(&mut self, window: Option<u64>) -> Result<AgentReceipt, String> {
        self.command(window.map(WindowRef), AgentCommand::Observe)
    }

    pub(crate) fn command(&mut self, window: Option<WindowRef>, command: AgentCommand) -> Result<AgentReceipt, String> {
        let request = AgentRequest { window, ..AgentRequest::new(None, command) };
        let line = serde_json::to_string(&request).map_err(|error| error.to_string())?;
        self.request(&line)
    }
}

pub(crate) struct Headless {
    child: Child,
    output: BufReader<ChildStdout>,
}

impl Drop for Headless {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.wait();
    }
}

pub(crate) fn inspect(
    out: &mut String,
    app: &AppArgs,
    window: &WindowArgs,
    connect: Option<&Path>,
    receipt: Option<&Path>,
    window_id: Option<u64>,
    json: bool,
) -> Result<ExitCode, String> {
    let expected = receipt.map(receipt::Receipt::read).transpose()?;
    let mut session = Session::open(app, window, connect)?;
    let mut observed = session.observe(window_id)?;
    if let Some(expected) = expected
        && let Some(refusal) = expected.mismatch(&observed)
    {
        observed = refusal;
    }
    if json {
        json_to(out, &observed);
    } else {
        if let Some(build) = &observed.build {
            let _ = writeln!(out, "build {} (pid {})", build.digest, build.pid);
        }
        observation_text(out, &observed);
    }
    Ok(if observed.ok { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

pub(crate) fn run(
    out: &mut String,
    app: &AppArgs,
    window: &WindowArgs,
    connect: Option<&Path>,
    require_build: Option<&Path>,
    script: Option<&Path>,
    json: bool,
) -> Result<ExitCode, String> {
    let expected = require_build.map(receipt::Receipt::read).transpose()?;
    let interactive = script.is_none();
    let input: Box<dyn BufRead> = match script {
        Some(path) => Box::new(BufReader::new(File::open(path).map_err(|error| format!("{}: {error}", path.display()))?)),
        None => Box::new(io::stdin().lock()),
    };
    let mut session = Session::open(app, window, connect)?;
    if expected.is_some() || connect.is_none() {
        let observed = session.command(None, AgentCommand::ListWindows)?;
        if let Some(refusal) = expected.and_then(|expected| expected.mismatch(&observed)) {
            if interactive || json {
                writeln!(out, "{}", serde_json::to_string(&refusal).expect("a refusal serializes")).expect("writing to a String");
            } else {
                observation_text(out, &refusal);
            }
            return Ok(ExitCode::FAILURE);
        }
    }

    let mut stdout = io::stdout().lock();
    let mut failed = false;
    for (line, request) in input.lines().enumerate() {
        let request = request.map_err(|error| format!("reading request on line {}: {error}", line + 1))?;
        let request = request.trim();
        if request.is_empty() || request.starts_with("//") {
            continue;
        }
        let reply = session.request(request)?;
        if interactive || json {
            let reply_json = serde_json::to_string(&reply).map_err(|error| error.to_string())?;
            if interactive {
                if writeln!(stdout, "{reply_json}").and_then(|()| stdout.flush()).is_err() {
                    break;
                }
            } else {
                writeln!(out, "{reply_json}").expect("writing to a String");
            }
        } else {
            writeln!(out, "line {}: {}", line + 1, if reply.ok { "ok" } else { "failed" }).expect("writing to a String");
            observation_text(out, &reply);
        }
        failed = !reply.ok;
        if failed && !interactive {
            break;
        }
    }
    if !interactive && !json {
        writeln!(out, "run: {}", if failed { "failed" } else { "ok" }).expect("writing to a String");
    }
    Ok(if failed && !interactive { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

fn observation_text(out: &mut String, receipt: &AgentReceipt) {
    if let Some(code) = &receipt.code {
        let _ = writeln!(out, "{code}: {}", receipt.message.as_deref().unwrap_or_default());
    }
    if let Some(snapshot) = &receipt.snapshot {
        let size = snapshot.viewport.size;
        let progress = receipt.progress;
        let _ = writeln!(
            out,
            "{} {}×{} observation {} tree {} settled={} committed={:?} presented={:?}",
            receipt.protocol,
            size.width,
            size.height,
            progress.observation_rev,
            progress.tree_rev,
            progress.settled,
            progress.committed_frame,
            progress.presented_frame,
        );
        let mut depths = HashMap::new();
        for node in &snapshot.nodes {
            let depth = node.parent.and_then(|target| depths.get(&target)).map_or(0, |parent| parent + 1);
            depths.insert(node.target, depth);
            let role = node.role.map_or_else(|| "node".into(), |role| format!("{role:?}"));
            let _ = write!(out, "{:indent$}{role}", "", indent = depth * 2);
            if let Some(id) = &node.test_id {
                let _ = write!(out, " #{id}");
            }
            if let Some(label) = node.label.as_ref().or(node.text.as_ref()) {
                let _ = write!(out, " {label:?}");
            }
            if let Some(value) = &node.value {
                let _ = write!(out, " value={value:?}");
            }
            if let Some(geometry) = node.geometry {
                let rect = geometry.rect;
                let _ = write!(out, " {},{} {}×{}", rect.x, rect.y, rect.w, rect.h);
            }
            if !node.shown {
                *out += " [hidden]";
            } else if node.interactive {
                *out += " [interactive]";
            }
            if snapshot.focus == Some(node.target) {
                *out += " [focused]";
            }
            out.push('\n');
        }
    }
    if let Some(windows) = &receipt.windows {
        for window in windows {
            let _ = writeln!(out, "window {} {:?}", window.id.0, window.title);
        }
    }
    for diagnostic in &receipt.diagnostics {
        let _ = writeln!(out, "  {}: {}", diagnostic.code, diagnostic.message);
    }
    if receipt.code.as_deref() == Some(codes::BUILD_MISMATCH.id) {
        let _ = writeln!(out, "No action was sent.");
    }
}
