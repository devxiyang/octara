// Compiled in the generated harness against the app's framework version.

use octara::{
    diagnostics::{Diagnostic, Severity, codes},
    prelude::{AppDefinition, Size, Theme, WindowOptions},
};
use octara_inspect::{AgentCommand, AgentRequest, HeadlessDriver, transport::InspectTransport};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, BufRead, Write},
    process::ExitCode,
};

pub fn run(startup: fn() -> AppDefinition) -> ExitCode {
    match execute(startup) {
        Ok(clean) => {
            if clean {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

fn execute(startup: fn() -> AppDefinition) -> Result<bool, Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    let options = args.next().ok_or("missing harness options")?;
    match command.as_deref() {
        Some("run") => {
            let options: WindowOverrides = serde_json::from_str(&options)?;
            let mut driver = HeadlessDriver::default();
            startup().launch_with_window_options(driver.app_mut(), None, |window| options.apply(window))?;
            let mut exchange = |request| driver.execute(request);
            let stdin = io::stdin().lock();
            let mut stdout = io::stdout().lock();
            for line in stdin.lines() {
                let reply = exchange.exchange(line?.as_bytes());
                stdout.write_all(reply.as_bytes())?;
                stdout.flush()?;
            }
            Ok(true)
        }
        Some("check") => {
            let options: CheckOptions = serde_json::from_str(&options)?;
            let report = check(startup, options)?;
            println!("{}", serde_json::to_string(&report)?);
            Ok(report.clean)
        }
        _ => Err("unknown harness command".into()),
    }
}

#[derive(Default, Deserialize)]
struct WindowOverrides {
    size: Option<Size>,
    theme: Option<ThemeName>,
    touch: Option<bool>,
}

impl WindowOverrides {
    fn apply(&self, window: &mut WindowOptions) {
        if let Some(size) = self.size {
            window.viewport.size = size;
        }
        if let Some(theme) = self.theme {
            window.theme = match theme {
                ThemeName::Light => Theme::light(),
                ThemeName::Dark => Theme::dark(),
            };
        }
        if let Some(touch) = self.touch {
            window.touch = touch;
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ThemeName {
    Light,
    Dark,
}

impl ThemeName {
    fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

#[derive(Deserialize)]
struct CheckSize {
    name: String,
    width: f32,
    height: f32,
    #[serde(default)]
    touch: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Acceptance {
    code: String,
    why: String,
}

#[derive(Default, Deserialize)]
struct CheckOptions {
    themes: Option<Vec<ThemeName>>,
    sizes: Option<Vec<CheckSize>>,
    #[serde(default)]
    accept: Vec<Acceptance>,
}

#[derive(Serialize)]
struct CheckReport {
    schema: &'static str,
    clean: bool,
    cells: Vec<String>,
    findings: Vec<Finding>,
    accepted: Vec<Finding>,
}

#[derive(Serialize)]
struct Finding {
    code: &'static str,
    severity: Severity,
    message: String,
    source: Option<String>,
    cells: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    why: Option<String>,
}

fn check(startup: fn() -> AppDefinition, options: CheckOptions) -> Result<CheckReport, Box<dyn std::error::Error>> {
    let themes = options.themes.unwrap_or_else(|| vec![ThemeName::Light, ThemeName::Dark]);
    let sizes = options.sizes.unwrap_or_else(|| {
        vec![
            CheckSize { name: "phone".into(), width: 390.0, height: 844.0, touch: true },
            CheckSize { name: "desktop".into(), width: 1280.0, height: 800.0, touch: false },
        ]
    });
    if themes.is_empty()
        || sizes.is_empty()
        || sizes.iter().any(|size| {
            size.name.trim().is_empty() || !size.width.is_finite() || size.width <= 0.0 || !size.height.is_finite() || size.height <= 0.0
        })
    {
        return Err("check requires themes and named sizes with positive finite dimensions".into());
    }
    if options.accept.iter().any(|rule| rule.why.trim().is_empty() || codes::lookup(&rule.code).is_none()) {
        return Err("check acceptance needs a registered diagnostic code and a reason".into());
    }
    let mut report = CheckReport { schema: "octara.check/2", clean: true, cells: Vec::new(), findings: Vec::new(), accepted: Vec::new() };
    for theme in themes {
        for size in &sizes {
            let cell = format!("{}/{}", theme.name(), size.name);
            let mut driver = HeadlessDriver::default();
            let overrides = WindowOverrides { size: Some(Size::new(size.width, size.height)), theme: Some(theme), touch: Some(size.touch) };
            startup().launch_with_window_options(driver.app_mut(), None, |window| overrides.apply(window))?;
            let windows: Vec<_> = driver.app().runtime().windows().map(|(&window, _)| window).collect();
            if windows.is_empty() {
                return Err(format!("{cell}: the app opened no windows").into());
            }
            for window in windows {
                let receipt = driver.execute(AgentRequest::new(Some(window), AgentCommand::Observe));
                for diagnostic in receipt.diagnostics {
                    record(&mut report, &cell, diagnostic, &options.accept);
                }
                if !receipt.progress.settled {
                    record(
                        &mut report,
                        &cell,
                        Diagnostic::new(&codes::NOT_SETTLED, receipt.message.unwrap_or_else(|| "window did not settle".into())),
                        &options.accept,
                    );
                }
            }
            report.cells.push(cell);
        }
    }
    report.clean = report.findings.iter().all(|finding| finding.severity == Severity::Info);
    Ok(report)
}

fn record(report: &mut CheckReport, cell: &str, diagnostic: Diagnostic, rules: &[Acceptance]) {
    let rule = rules.iter().find(|rule| rule.code == diagnostic.code.id);
    let findings = if rule.is_some() { &mut report.accepted } else { &mut report.findings };
    if let Some(finding) = findings.iter_mut().find(|finding| {
        finding.code == diagnostic.code.id && finding.message == diagnostic.message && finding.source == diagnostic.location
    }) {
        if !finding.cells.iter().any(|name| name == cell) {
            finding.cells.push(cell.into());
        }
    } else {
        findings.push(Finding {
            code: diagnostic.code.id,
            severity: diagnostic.severity(),
            message: diagnostic.message,
            source: diagnostic.location,
            cells: vec![cell.into()],
            why: rule.map(|rule| rule.why.clone()),
        });
    }
}
