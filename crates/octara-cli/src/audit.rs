use crate::{AppArgs, WindowArgs, json_to, session::Session};
use octara_inspect::{AgentCommand, AgentReceipt, AuditFinding, protocol::WindowRef};
use octara_model::diagnostics::Severity;
use serde::Serialize;
use std::{fmt::Write as _, path::Path, process::ExitCode};

#[derive(Serialize)]
struct Report {
    schema: &'static str,
    clean: bool,
    windows: Vec<WindowAudit>,
}

#[derive(Serialize)]
struct WindowAudit {
    id: WindowRef,
    title: String,
    findings: Vec<AuditFinding>,
}

pub(crate) fn run(
    out: &mut String,
    app: &AppArgs,
    options: &WindowArgs,
    connect: Option<&Path>,
    window_id: Option<u64>,
    json: bool,
) -> Result<ExitCode, String> {
    let mut session = Session::open(app, options, connect)?;
    let windows = accepted(session.command(None, AgentCommand::ListWindows)?)?.windows.ok_or("the host did not return a window list")?;
    let mut report = Report { schema: "octara.audit/1", clean: true, windows: Vec::new() };
    for window in windows.into_iter().filter(|window| window_id.is_none_or(|id| window.id.0 == id)) {
        let receipt = accepted(session.command(Some(window.id), AgentCommand::Audit)?)?;
        if receipt.window != Some(window.id) {
            return Err(format!("audit receipt identifies a different window than {}", window.id.0));
        }
        if !receipt.progress.settled {
            return Err(format!("window {} did not settle", window.id.0));
        }
        let findings = receipt.findings.ok_or("the host did not return audit findings")?;
        report.clean &= findings.iter().all(|finding| finding.severity != Severity::Error);
        report.windows.push(WindowAudit { id: window.id, title: window.title, findings });
    }
    if report.windows.is_empty() {
        return Err(window_id.map_or_else(|| "the app opened no windows".into(), |id| format!("no window has id {id}")));
    }
    if json {
        json_to(out, &report);
    } else {
        render_text(out, &report);
    }
    Ok(if report.clean { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

fn accepted(receipt: AgentReceipt) -> Result<AgentReceipt, String> {
    if receipt.ok {
        return Ok(receipt);
    }
    Err(format!(
        "{}: {}",
        receipt.code.as_deref().unwrap_or("audit request failed"),
        receipt.message.as_deref().unwrap_or("the host returned no details")
    ))
}

fn render_text(out: &mut String, report: &Report) {
    for window in &report.windows {
        let _ = writeln!(out, "window {} {:?}", window.id.0, window.title);
        for finding in &window.findings {
            if let Some(source) = &finding.provenance {
                let _ = write!(out, "  {source}: ");
            } else {
                let _ = write!(out, "  {:?}: ", finding.node.target);
            }
            let severity = match finding.severity {
                Severity::Info => "info",
                Severity::Warning => "warning",
                Severity::Error => "error",
            };
            let _ = writeln!(out, "{severity} {}: {}", finding.code, finding.message);
        }
    }
    let findings: usize = report.windows.iter().map(|window| window.findings.len()).sum();
    let noun = if findings == 1 { "finding" } else { "findings" };
    let _ = writeln!(out, "audit: {findings} {noun}");
}
