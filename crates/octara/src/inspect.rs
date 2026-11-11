//! The Inspector protocol: what tools and agents read from a running app and how they drive it.
//!
//! - [`Snapshot`]: the semantic tree (`octara.inspect/1`) — roles, names, values, states,
//!   presented bounds, actions, commands and source locations, addressed by test id.
//! - [`Request`] / [`Reply`]: one JSON line each way (`{"op":"tap","id":"save"}` →
//!   `{"ok":true}`); a failure names its registered code. A script is requests, one per line
//!   ([`run_script`]), stopping at the first failure.
//! - [`check`]: the app at each theme and size, judged: layout issues, diagnostics, pending work
//!   and accessibility.
//!
//! The protocol lives in the framework, not the CLI, so it always matches the `octara` the app
//! was built with. Within a schema version fields are only added.

use serde::{Deserialize, Serialize};

/// The snapshot schema; bumped when a field changes meaning or goes away.
pub const SCHEMA: &str = "octara.inspect/1";

/// The app as tools see it at one moment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema: String,
    /// The `octara` version the app was built with.
    pub octara: String,
    /// Window size in logical pixels.
    pub viewport: [f32; 2],
    pub theme: String,
    pub breakpoint: String,
    /// The app clock, in milliseconds since start.
    pub time_ms: u64,
    /// The focused element's test id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    /// Tasks, subscriptions and background jobs still pending.
    pub pending: usize,
    pub root: Node,
}

/// One element of the semantic tree. Absent fields are left out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// The test id (`.id(..)`), which every operation addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The accessibility role, in snake case (`button`, `label`, `text_input`).
    pub role: String,
    /// What assistive technology announces: the label, else a text's own text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded: Option<bool>,
    /// `hovered`, `focused`, `focus_visible`, `pressed`, `selected`, `disabled`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<String>,
    /// Where it is drawn now (`[x, y, width, height]`, logical pixels, to 0.1), mid-animation
    /// included.
    pub bounds: [f32; 4],
    /// What can be done to it: `press`, `toggle`, `double_press`, `secondary_press`,
    /// `long_press`, `drag`, `input`, `submit`, `focus`, `scroll`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
    /// Commands it handles (`on_command`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
    /// Where it was created (`file:line:column`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

impl Node {
    /// This node and every node below it, depth first.
    pub fn walk(&self) -> Vec<&Node> {
        let mut out = vec![self];
        for c in &self.children {
            out.extend(c.walk());
        }
        out
    }
}

impl Snapshot {
    /// The node with test id `id`.
    pub fn find(&self, id: &str) -> Option<&Node> {
        self.root.walk().into_iter().find(|n| n.id.as_deref() == Some(id))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a snapshot serializes")
    }
}

/// `GenericContainer` → `generic_container`.
pub(crate) fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// A length to 0.1, so snapshots stay stable across float noise.
pub(crate) fn tenth(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

// ---- requests: the interactive protocol and scripts (JSON lines) ----

/// One request, one JSON line: `{"op":"tap","id":"save"}`. Scripts are these, one per line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// The semantic tree and app state ([`Snapshot`]).
    Inspect,
    /// Layout issues and diagnostics now.
    Issues,
    /// A primary click or tap on the element.
    Tap {
        id: String,
    },
    DoubleTap {
        id: String,
    },
    LongPress {
        id: String,
    },
    /// A right click / context press.
    SecondaryTap {
        id: String,
    },
    /// Focuses the element (as Tab would).
    Focus {
        id: String,
    },
    /// The pointer enters (`true`) or leaves the element.
    Hover {
        id: String,
        on: bool,
    },
    /// Presses a chord as a keyboard would: `"mod+s"`, `"tab"`, `"enter"`, `"a"`.
    Key {
        chord: String,
    },
    /// Types text into the focused field, or into `id` after focusing it.
    Type {
        #[serde(default)]
        id: Option<String>,
        text: String,
    },
    /// Runs a command (`"save"`, `"edit.undo"`) as its chord or a menu would.
    Invoke {
        command: String,
    },
    /// Moves the app clock forward.
    Advance {
        ms: u64,
    },
    /// Runs timers, tasks and motion until nothing is left (at most `limit_ms`, default 10 s).
    Settle {
        #[serde(default)]
        limit_ms: Option<u64>,
    },
    /// Resizes the window (logical pixels).
    Size {
        width: f32,
        height: f32,
    },
    /// `"light"` or `"dark"`.
    Theme {
        name: String,
    },
    ReducedMotion {
        on: bool,
    },
    /// Fails unless every given property holds for the element `id` (or, with only `clean`, for
    /// the app).
    Expect(Expect),
}

/// What an `expect` checks; every given field must hold.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Whether the element exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
    /// The element's text (a text element's own, else its name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// A text field's value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    /// A state the element is in (`focused`, `disabled`, ..; see [`Node::states`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// No layout issues (Warning or worse) and no diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clean: Option<bool>,
}

/// The answer to one request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub ok: bool,
    /// The registered diagnostic code of a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// What an `expect` found instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Box<Snapshot>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issues: Option<Vec<Issue>>,
}

/// A layout issue or diagnostic, as tools see it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    pub code: String,
    /// `info`, `warning` or `error`.
    pub severity: String,
    pub message: String,
    /// The element's test id, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Where the element was created, or where the diagnostic was reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The fix for this case, when it is more specific than the code's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Reply {
    pub fn ok() -> Reply {
        Reply { ok: true, code: None, message: None, actual: None, snapshot: None, issues: None }
    }

    pub fn fail(code: &'static Code, message: impl Into<String>) -> Reply {
        Reply { ok: false, code: Some(code.id.into()), message: Some(message.into()), ..Reply::ok() }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a reply serializes")
    }
}

use crate::app::App;
use crate::diagnostics::{self as codes, Code, Severity};

pub(crate) fn severity_name(s: Severity) -> &'static str {
    match s {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

/// Every layout issue and diagnostic of `app` now.
pub fn issues(app: &App) -> Vec<Issue> {
    let mut out: Vec<Issue> = app
        .layout_issues()
        .into_iter()
        .map(|i| Issue {
            code: i.code.id.into(),
            severity: severity_name(i.code.severity).into(),
            message: i.message,
            id: i.test_id,
            source: i.at,
            hint: Some(i.hint.into()),
        })
        .collect();
    out.extend(app.diagnostics().into_iter().map(|d| Issue {
        code: d.code.id.into(),
        severity: severity_name(d.code.severity).into(),
        message: d.message,
        id: None,
        source: d.location,
        hint: None,
    }));
    out
}

/// Runs one request against `app`.
pub fn handle(app: &mut App, req: Request) -> Reply {
    use std::time::Duration;
    let exists = |app: &App, id: &str| app.kernel().find(id).is_some();
    let target = |app: &App, id: &str| -> Result<(), Reply> {
        if exists(app, id) { Ok(()) } else { Err(Reply::fail(&codes::NO_SUCH_ELEMENT, format!("no element has test id `{id}`"))) }
    };
    let step = |r: Result<(), String>| match r {
        Ok(()) => Reply::ok(),
        Err(e) => Reply::fail(&codes::STEP_FAILED, e),
    };
    macro_rules! on {
        ($id:expr, $e:expr) => {{
            if let Err(r) = target(app, $id) {
                return r;
            }
            step($e)
        }};
    }
    match req {
        Request::Inspect => Reply { snapshot: Some(Box::new(app.inspect())), ..Reply::ok() },
        Request::Issues => Reply { issues: Some(issues(app)), ..Reply::ok() },
        Request::Tap { id } => on!(&id, app.tap(&id)),
        Request::DoubleTap { id } => on!(&id, app.double_tap(&id)),
        Request::LongPress { id } => on!(&id, app.long_press(&id)),
        Request::SecondaryTap { id } => on!(&id, app.secondary_tap(&id)),
        Request::Focus { id } => on!(&id, app.dispatch(crate::app::Event::Focus(id.clone()))),
        Request::Hover { id, on } => on!(&id, app.dispatch(crate::app::Event::Hover(id.clone(), on))),
        Request::Key { chord } => step(app.press_key(&chord)),
        Request::Type { id, text } => {
            if let Some(id) = id {
                if let Err(r) = target(app, &id) {
                    return r;
                }
                if let Err(e) = app.dispatch(crate::app::Event::Focus(id.clone())) {
                    return Reply::fail(&codes::STEP_FAILED, e);
                }
            }
            if app.text_field().is_none() {
                return Reply::fail(&codes::STEP_FAILED, "no text field has focus");
            }
            app.type_text(&text);
            Reply::ok()
        }
        Request::Invoke { command } => step(app.invoke(&command)),
        Request::Advance { ms } => {
            app.advance(Duration::from_millis(ms));
            Reply::ok()
        }
        Request::Settle { limit_ms } => match app.run_until_idle(Duration::from_millis(limit_ms.unwrap_or(10_000))) {
            Ok(()) => Reply::ok(),
            Err(pending) => Reply::fail(&codes::STEP_FAILED, format!("still running at the limit: {pending:?}")),
        },
        Request::Size { width, height } => {
            app.resize(width, height);
            Reply::ok()
        }
        Request::Theme { name } => match name.as_str() {
            "light" => {
                app.set_theme(crate::kernel::Theme::light());
                Reply::ok()
            }
            "dark" => {
                app.set_theme(crate::kernel::Theme::dark());
                Reply::ok()
            }
            other => Reply::fail(&codes::BAD_REQUEST, format!("unknown theme `{other}`: `light` or `dark`")),
        },
        Request::ReducedMotion { on } => {
            app.set_reduced_motion(on);
            Reply::ok()
        }
        Request::Expect(e) => expect(app, &e),
    }
}

fn expect(app: &App, e: &Expect) -> Reply {
    let mismatch = |what: &str, want: serde_json::Value, got: serde_json::Value| Reply {
        actual: Some(got.clone()),
        ..Reply::fail(&codes::EXPECTATION_FAILED, format!("expected {what} {want}, found {got}"))
    };
    if e.clean == Some(true) {
        let found: Vec<Issue> = issues(app).into_iter().filter(|i| i.severity != "info").collect();
        if !found.is_empty() {
            let codes: Vec<&str> = found.iter().map(|i| i.code.as_str()).collect();
            return Reply { issues: Some(found.clone()), ..mismatch("no issues, ", "[]".into(), codes.into()) };
        }
    }
    let Some(id) = &e.id else {
        return if e.clean.is_some() { Reply::ok() } else { Reply::fail(&codes::BAD_REQUEST, "`expect` needs an `id` (or only `clean`)") };
    };
    let snap = app.inspect();
    let node = snap.find(id);
    if let Some(want) = e.exists
        && want != node.is_some()
    {
        return mismatch(&format!("`{id}` to exist:"), want.into(), node.is_some().into());
    }
    let wants_node = e.text.is_some() || e.value.is_some() || e.checked.is_some() || e.state.is_some();
    let Some(node) = node else {
        return if wants_node { Reply::fail(&codes::NO_SUCH_ELEMENT, format!("no element has test id `{id}`")) } else { Reply::ok() };
    };
    if let Some(want) = &e.text {
        let got = app.kernel().text_of(id).filter(|t| !t.is_empty()).or_else(|| node.name.clone()).unwrap_or_default();
        if &got != want {
            return mismatch(&format!("`{id}` text"), want.as_str().into(), got.into());
        }
    }
    if let Some(want) = &e.value
        && node.value.as_ref() != Some(want)
    {
        return mismatch(&format!("`{id}` value"), want.as_str().into(), node.value.clone().into());
    }
    if let Some(want) = e.checked
        && node.checked != Some(want)
    {
        return mismatch(&format!("`{id}` checked"), want.into(), node.checked.into());
    }
    if let Some(want) = &e.state
        && !node.states.contains(want)
    {
        return mismatch(&format!("`{id}` in state"), want.as_str().into(), node.states.clone().into());
    }
    Reply::ok()
}

/// Reads one request line and runs it.
pub fn handle_line(app: &mut App, line: &str) -> Reply {
    match serde_json::from_str::<Request>(line) {
        Ok(req) => handle(app, req),
        Err(e) => Reply::fail(&codes::BAD_REQUEST, format!("{e}: {line}")),
    }
}

/// Runs a script (JSON lines; blank lines and `//` comments skipped), stopping at the first
/// failure. Returns each step's line number and reply.
pub fn run_script(app: &mut App, script: &str) -> Vec<(usize, Reply)> {
    let mut out = Vec::new();
    for (i, line) in script.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let reply = handle_line(app, line);
        let failed = !reply.ok;
        out.push((i + 1, reply));
        if failed {
            break;
        }
    }
    out
}

// ---- check: the app at each theme and size, judged ----

/// The check report schema.
pub const CHECK_SCHEMA: &str = "octara.check/1";

/// One window size to check at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub name: String,
    pub width: f32,
    pub height: f32,
    /// Input is touch here: targets must be large enough for a finger.
    #[serde(default)]
    pub touch: bool,
}

/// A finding the app accepts, with the reason (`[package.metadata.octara] accept`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accept {
    pub code: String,
    /// A test id, or a prefix ending in `*` (`row-*`); absent for findings without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Why it is fine. Required.
    pub why: String,
}

/// What `check` runs; every field may be left out (`[package.metadata.octara]` gives them).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CheckOptions {
    pub sizes: Vec<Size>,
    pub themes: Vec<String>,
    pub accept: Vec<Accept>,
}

impl Default for CheckOptions {
    /// A phone and a desktop window, light and dark.
    fn default() -> Self {
        CheckOptions {
            sizes: vec![
                Size { name: "phone".into(), width: 390.0, height: 844.0, touch: true },
                Size { name: "desktop".into(), width: 1280.0, height: 800.0, touch: false },
            ],
            themes: vec!["light".into(), "dark".into()],
            accept: Vec::new(),
        }
    }
}

/// One problem, merged across the cells (theme / size) it appears in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub code: String,
    pub severity: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The fix for this case; the code's general fix is `fix`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    pub fix: String,
    pub doc: String,
    /// Where it appears: `light/phone`, `dark/desktop`, ...
    pub cells: Vec<String>,
    /// Accepted findings: why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckReport {
    pub schema: String,
    pub octara: String,
    /// No finding of Warning or worse remains (accepted ones aside).
    pub clean: bool,
    pub cells: Vec<String>,
    /// Sorted by severity, then code.
    pub findings: Vec<Finding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted: Vec<Finding>,
}

impl CheckReport {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a report serializes")
    }
}

/// How long `check` lets the app settle, in app time.
pub const SETTLE_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Mounts `view` fresh at each theme and size, lets it settle, and judges it: layout issues,
/// diagnostics, work that does not settle, and accessibility (names, contrast, and touch target
/// sizes where input is touch).
pub fn check(view: fn() -> crate::view::View, opts: &CheckOptions) -> CheckReport {
    let mut findings: Vec<Finding> = Vec::new();
    let mut cells = Vec::new();
    for theme in &opts.themes {
        for size in &opts.sizes {
            let cell = format!("{theme}/{}", size.name);
            cells.push(cell.clone());
            let mut app = App::new(view);
            app.set_theme(if theme == "dark" { crate::kernel::Theme::dark() } else { crate::kernel::Theme::light() });
            app.resize(size.width, size.height);
            let mut found = issues(&app);
            if let Err(pending) = app.run_until_idle(SETTLE_LIMIT) {
                found.push(Issue {
                    code: codes::NOT_SETTLED.id.into(),
                    severity: severity_name(codes::NOT_SETTLED.severity).into(),
                    message: format!("{} still running: {pending:?}", pending.len()),
                    id: None,
                    source: None,
                    hint: None,
                });
            }
            // After settling, what the app shows at rest.
            found = issues(&app).into_iter().chain(found.into_iter().filter(|i| i.code == codes::NOT_SETTLED.id)).collect();
            found.extend(app.kernel().accessibility_issues(size.touch).into_iter().map(|i| Issue {
                code: i.code.id.into(),
                severity: severity_name(i.code.severity).into(),
                message: i.message,
                id: i.test_id,
                source: i.at,
                hint: Some(i.hint.into()),
            }));
            for i in found {
                let same = |f: &&mut Finding| f.code == i.code && f.id == i.id && f.source == i.source;
                if let Some(f) = findings.iter_mut().find(same) {
                    if !f.cells.contains(&cell) {
                        f.cells.push(cell.clone());
                    }
                    continue;
                }
                let code = codes::code(&i.code);
                findings.push(Finding {
                    fix: code.map(|c| c.fix.to_string()).unwrap_or_default(),
                    doc: code.map(|c| c.doc.to_string()).unwrap_or_default(),
                    code: i.code,
                    severity: i.severity,
                    message: i.message,
                    id: i.id,
                    source: i.source,
                    hint: i.hint,
                    cells: vec![cell.clone()],
                    why: None,
                });
            }
        }
    }
    let rank = |s: &str| match s {
        "error" => 0,
        "warning" => 1,
        _ => 2,
    };
    findings.sort_by(|a, b| rank(&a.severity).cmp(&rank(&b.severity)).then(a.code.cmp(&b.code)));
    let (mut accepted, mut open) = (Vec::new(), Vec::new());
    for mut f in findings {
        let hit = opts.accept.iter().find(|a| {
            a.code == f.code
                && match (&a.id, &f.id) {
                    (None, _) => true,
                    (Some(p), Some(id)) => p.strip_suffix('*').map_or(p == id, |pre| id.starts_with(pre)),
                    (Some(_), None) => false,
                }
        });
        match hit {
            Some(a) => {
                f.why = Some(a.why.clone());
                accepted.push(f);
            }
            None => open.push(f),
        }
    }
    CheckReport {
        schema: CHECK_SCHEMA.into(),
        octara: env!("CARGO_PKG_VERSION").into(),
        clean: open.iter().all(|f| f.severity == "info"),
        cells,
        findings: open,
        accepted,
    }
}

// ---- the harness entry point ----

/// The `main` of the harness `octara check` / `inspect` / `run` build around an app: the
/// app's root view is all it needs. Arguments:
///
/// - `check [--options <json>]`: prints a [`CheckReport`] (JSON);
/// - `inspect [--size <w>x<h>] [--theme light|dark]`: settles the first screen and prints a
///   [`Snapshot`] (JSON);
/// - `run [--size ..] [--theme ..] [--stop]`: reads requests from stdin, one JSON line each, and
///   prints one reply line each (`{"line":n,"ok":..}`); `--stop` ends at the first failure.
///
/// Exits 2 on bad arguments.
pub fn main(view: fn() -> crate::view::View) {
    use std::io::{BufRead, Write};
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let fail = |msg: String| -> ! {
        eprintln!("octara harness: {msg}");
        std::process::exit(2)
    };
    let setup = |app: &mut App| {
        if let Some(size) = flag("--size") {
            let parsed = size.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
            let Some((w, h)) = parsed else { fail(format!("--size takes <width>x<height>, not `{size}`")) };
            app.resize(w, h);
        }
        match flag("--theme").as_deref() {
            None | Some("light") => {}
            Some("dark") => app.set_theme(crate::kernel::Theme::dark()),
            Some(other) => fail(format!("unknown theme `{other}`")),
        }
    };
    match args.first().map(String::as_str) {
        Some("check") => {
            let opts = match flag("--options") {
                Some(json) => serde_json::from_str(&json).unwrap_or_else(|e| fail(format!("--options: {e}"))),
                None => CheckOptions::default(),
            };
            println!("{}", check(view, &opts).to_json());
        }
        Some("inspect") => {
            let mut app = App::new(view);
            setup(&mut app);
            let _ = app.run_until_idle(SETTLE_LIMIT);
            println!("{}", serde_json::to_string_pretty(&app.inspect()).expect("serializes"));
        }
        Some("run") => {
            let mut app = App::new(view);
            setup(&mut app);
            let stop = args.iter().any(|a| a == "--stop");
            let stdout = std::io::stdout();
            for (i, line) in std::io::stdin().lock().lines().enumerate() {
                let Ok(line) = line else { break };
                let line = line.trim();
                if line.is_empty() || line.starts_with("//") {
                    continue;
                }
                let reply = handle_line(&mut app, line);
                let mut value = serde_json::to_value(&reply).expect("serializes");
                value.as_object_mut().expect("an object").insert("line".into(), (i + 1).into());
                let mut out = stdout.lock();
                let _ = writeln!(out, "{value}");
                let _ = out.flush();
                if stop && !reply.ok {
                    break;
                }
            }
        }
        _ => fail("usage: check [--options <json>] | inspect [--size WxH] [--theme T] | run [--size WxH] [--theme T] [--stop]".into()),
    }
}
