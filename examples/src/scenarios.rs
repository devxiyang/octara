//! Shared scenarios for the example apps: acceptance tests, hot-reload scenarios and benchmark
//! steps, written against `octara::testing::Host`. Specs: `examples/README.md`.

pub use octara::app::Window;
pub use octara::app::{DEFAULT_VIEWPORT, Event, Work};
pub use octara::diagnostics::{Diagnostic, Severity};
pub use octara::kernel::{Kernel, State, Theme, Token};

/// Builds the four spec apps.
pub trait Factory {
    const NAME: &'static str;
    fn counter() -> Window;
    fn todo_with(n: usize) -> Window;
    fn chat_with(n: usize) -> Window;
    fn settings() -> Window;
    fn todo() -> Window {
        Self::todo_with(0)
    }
    fn chat() -> Window {
        Self::chat_with(0)
    }
}

pub type Check = Result<(), String>;

macro_rules! ensure {
    ($cond:expr, $($fmt:tt)+) => {
        if !$cond {
            return Err(format!($($fmt)+));
        }
    };
}

macro_rules! ensure_eq {
    ($left:expr, $right:expr) => {{
        let (l, r) = (&$left, &$right);
        if l != r {
            return Err(format!("{} == {}: left {:?}, right {:?} (line {})", stringify!($left), stringify!($right), l, r, line!()));
        }
    }};
}

pub fn press(h: &mut Window, id: &str) -> Check {
    h.dispatch(Event::Press(id.into()))
}
pub fn input(h: &mut Window, id: &str, text: &str) -> Check {
    h.dispatch(Event::Input(id.into(), text.into()))
}
pub fn toggle(h: &mut Window, id: &str) -> Check {
    h.dispatch(Event::Toggle(id.into()))
}
pub fn external(h: &mut Window, name: &str, payload: &str) -> Check {
    h.dispatch(Event::Send(name.into(), payload.into()))
}

fn text(h: &Window, id: &str) -> Option<String> {
    h.kernel().text_of(id)
}

fn selected(h: &Window, id: &str) -> Option<bool> {
    h.kernel().state_of(id).map(|s| s.contains(State::SELECTED))
}

fn text_color(h: &Window, id: &str) -> Option<octara::kernel::Color> {
    h.kernel().visual_of(id).map(|v| v.text)
}

/// Fails with the tree dump attached, to make failures self-explanatory.
pub fn dump(h: &Window) -> String {
    h.kernel().tree_dump()
}

fn no_errors(h: &Window) -> Check {
    let errors: Vec<_> = h.app().diagnostics().into_iter().filter(|d| d.severity() == Severity::Error).collect();
    ensure!(errors.is_empty(), "unexpected error diagnostics: {errors:?}");
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Acceptance
// ---------------------------------------------------------------------------------------------

pub fn counter_acceptance(h: &mut Window) -> Check {
    ensure_eq!(text(h, "count"), Some("Count: 0".into()));
    ensure_eq!(text(h, "double"), Some("Double: 0".into()));
    for _ in 0..3 {
        press(h, "inc")?;
    }
    ensure_eq!(text(h, "count"), Some("Count: 3".into()));
    ensure_eq!(text(h, "double"), Some("Double: 6".into()));
    press(h, "reset")?;
    ensure_eq!(text(h, "count"), Some("Count: 0".into()));
    ensure_eq!(text(h, "double"), Some("Double: 0".into()));
    no_errors(h)
}

pub fn todo_acceptance(h: &mut Window) -> Check {
    let k = |h: &Window| h.kernel().child_ids("todos");
    ensure_eq!(text(h, "left"), Some("0 left".into()));
    ensure!(h.kernel().exists("todos"), "group#todos missing");
    ensure!(k(h).is_empty(), "todos should start empty");

    input(h, "new-todo", "Buy milk")?;
    ensure_eq!(h.kernel().value_of("new-todo"), Some("Buy milk".into()));
    press(h, "add")?;
    ensure_eq!(h.kernel().value_of("new-todo"), Some(String::new()));
    ensure_eq!(text(h, "title-1"), Some("Buy milk".into()));
    ensure_eq!(text(h, "left"), Some("1 left".into()));

    input(h, "new-todo", "   ")?;
    press(h, "add")?;
    ensure_eq!(k(h), vec!["todo-1".to_string()]);

    input(h, "new-todo", "  Walk dog ")?;
    press(h, "add")?;
    ensure_eq!(text(h, "title-2"), Some("Walk dog".into()));
    ensure_eq!(text(h, "left"), Some("2 left".into()));
    ensure!(!h.kernel().exists("clear-done"), "clear-done should be absent");

    toggle(h, "toggle-1")?;
    ensure_eq!(h.kernel().checked_of("toggle-1"), Some(true));
    ensure_eq!(text_color(h, "title-1"), Some(Theme::light().get(Token::TextMuted)));
    ensure_eq!(text_color(h, "title-2"), Some(Theme::light().get(Token::Text)));
    ensure_eq!(text(h, "left"), Some("1 left".into()));
    ensure!(h.kernel().exists("clear-done"), "clear-done should appear");

    ensure_eq!(selected(h, "filter-all"), Some(true));
    press(h, "filter-active")?;
    ensure_eq!(k(h), vec!["todo-2".to_string()]);
    ensure_eq!(selected(h, "filter-active"), Some(true));
    ensure_eq!(selected(h, "filter-all"), Some(false));
    press(h, "filter-done")?;
    ensure_eq!(k(h), vec!["todo-1".to_string()]);
    press(h, "filter-all")?;
    ensure_eq!(k(h), vec!["todo-1".to_string(), "todo-2".to_string()]);

    toggle(h, "toggle-1")?;
    ensure_eq!(text_color(h, "title-1"), Some(Theme::light().get(Token::Text)));
    ensure!(!h.kernel().exists("clear-done"), "clear-done should disappear");
    toggle(h, "toggle-1")?;

    press(h, "remove-2")?;
    ensure!(!h.kernel().exists("todo-2"), "todo-2 should be removed");
    ensure_eq!(text(h, "left"), Some("0 left".into()));
    press(h, "clear-done")?;
    ensure!(k(h).is_empty(), "clear-done should remove todo-1");
    ensure!(!h.kernel().exists("clear-done"), "clear-done should be absent again");

    input(h, "new-todo", "x")?;
    press(h, "add")?;
    ensure_eq!(k(h), vec!["todo-3".to_string()]);
    no_errors(h)
}

pub fn chat_acceptance(h: &mut Window) -> Check {
    ensure_eq!(text(h, "header"), Some("0 messages".into()));
    ensure_eq!(text(h, "status"), Some("idle".into()));

    external(h, "user", "hi")?;
    ensure_eq!(text(h, "msg-role-1"), Some("user".into()));
    ensure_eq!(text(h, "msg-body-1"), Some("hi".into()));
    ensure_eq!(text(h, "msg-role-2"), Some("assistant".into()));
    ensure_eq!(text(h, "msg-body-2"), Some(String::new()));
    ensure_eq!(text(h, "header"), Some("2 messages".into()));
    ensure_eq!(text(h, "status"), Some("streaming".into()));

    external(h, "token", "Hel")?;
    external(h, "token", "lo")?;
    ensure_eq!(text(h, "msg-body-2"), Some("Hello".into()));
    ensure_eq!(text(h, "msg-body-1"), Some("hi".into()));
    external(h, "done", "")?;
    ensure_eq!(text(h, "status"), Some("idle".into()));

    input(h, "composer", "next")?;
    press(h, "send")?;
    ensure_eq!(h.kernel().value_of("composer"), Some(String::new()));
    ensure_eq!(text(h, "msg-body-3"), Some("next".into()));
    ensure_eq!(text(h, "msg-role-4"), Some("assistant".into()));
    ensure_eq!(text(h, "status"), Some("streaming".into()));
    ensure_eq!(
        h.kernel().child_ids("messages"),
        vec!["msg-1", "msg-2", "msg-3", "msg-4"].into_iter().map(String::from).collect::<Vec<_>>()
    );
    press(h, "send")?; // empty composer: no-op
    ensure_eq!(text(h, "header"), Some("4 messages".into()));
    no_errors(h)
}

fn summary(n: &str, s: &str, v: &str, t: &str, tel: &str) -> Option<String> {
    Some(format!("notifications={n} sound={s} volume={v} theme={t} telemetry={tel}"))
}

pub fn settings_acceptance(h: &mut Window) -> Check {
    ensure_eq!(text(h, "summary"), summary("off", "on", "50", "system", "off"));
    ensure!(!h.kernel().exists("notification-options"), "options should start hidden");
    ensure_eq!(selected(h, "theme-system"), Some(true));
    ensure!(!h.kernel().exists("account-body"), "account starts closed");

    toggle(h, "notifications")?;
    ensure!(h.kernel().exists("notification-options"), "options should show");
    ensure_eq!(h.kernel().checked_of("sound"), Some(true));
    ensure_eq!(h.kernel().value_of("volume"), Some("50".into()));
    input(h, "volume", "70")?;
    toggle(h, "sound")?;
    ensure_eq!(text(h, "summary"), summary("on", "off", "70", "system", "off"));

    press(h, "account-toggle")?;
    ensure_eq!(text(h, "account-name"), Some("Signed in as dev".into()));

    toggle(h, "notifications")?;
    ensure!(!h.kernel().exists("notification-options"), "options should hide");
    ensure!(h.kernel().exists("account-body"), "account must stay open across a sibling change");
    ensure!(!h.kernel().exists("advanced-body"), "advanced must stay closed across a sibling change");
    toggle(h, "notifications")?;
    ensure_eq!(h.kernel().checked_of("sound"), Some(false));
    ensure_eq!(h.kernel().value_of("volume"), Some("70".into()));
    ensure!(h.kernel().exists("account-body"), "account must stay open");

    press(h, "advanced-toggle")?;
    toggle(h, "telemetry")?;
    ensure_eq!(h.kernel().checked_of("telemetry"), Some(true));
    press(h, "theme-dark")?;
    ensure_eq!(selected(h, "theme-dark"), Some(true));
    ensure_eq!(selected(h, "theme-system"), Some(false));
    ensure_eq!(text(h, "summary"), summary("on", "off", "70", "dark", "on"));

    press(h, "account-toggle")?;
    ensure!(!h.kernel().exists("account-body"), "account should close");
    ensure!(h.kernel().exists("advanced-body"), "advanced stays open");
    no_errors(h)
}

// ---------------------------------------------------------------------------------------------
// Hot reload: interact, reload, compare the semantic dump, keep interacting.
// ---------------------------------------------------------------------------------------------

fn reload_keeps_tree(h: &mut Window) -> Check {
    let before = h.kernel().tree_dump();
    h.app().hot_reload();
    let after = h.kernel().tree_dump();
    ensure!(before == after, "tree changed across hot reload:\n--- before\n{before}--- after\n{after}");
    no_errors(h)
}

pub fn counter_hot_reload(h: &mut Window) -> Check {
    press(h, "inc")?;
    press(h, "inc")?;
    reload_keeps_tree(h)?;
    press(h, "inc")?;
    ensure_eq!(text(h, "count"), Some("Count: 3".into()));
    Ok(())
}

pub fn todo_hot_reload(h: &mut Window) -> Check {
    for t in ["a", "b", "c"] {
        input(h, "new-todo", t)?;
        press(h, "add")?;
    }
    toggle(h, "toggle-2")?;
    press(h, "filter-active")?;
    input(h, "new-todo", "draft")?;
    reload_keeps_tree(h)?;
    toggle(h, "toggle-1")?;
    ensure_eq!(h.kernel().child_ids("todos"), vec!["todo-3".to_string()]);
    press(h, "add")?;
    ensure_eq!(text(h, "title-4"), Some("draft".into()));
    Ok(())
}

pub fn chat_hot_reload(h: &mut Window) -> Check {
    external(h, "user", "hi")?;
    external(h, "token", "par")?;
    input(h, "composer", "typing")?;
    reload_keeps_tree(h)?;
    external(h, "token", "tial")?;
    ensure_eq!(text(h, "msg-body-2"), Some("partial".into()));
    ensure_eq!(h.kernel().value_of("composer"), Some("typing".into()));
    Ok(())
}

pub fn settings_hot_reload(h: &mut Window) -> Check {
    toggle(h, "notifications")?;
    input(h, "volume", "30")?;
    press(h, "account-toggle")?;
    press(h, "theme-light")?;
    reload_keeps_tree(h)?;
    ensure!(h.kernel().exists("account-body"), "disclosure state lost");
    toggle(h, "notifications")?;
    ensure!(h.kernel().exists("account-body"), "disclosure state lost after sibling change");
    ensure_eq!(text(h, "summary"), summary("off", "on", "30", "light", "off"));
    Ok(())
}

/// Runs every acceptance and hot-reload scenario, returning `(name, result)` pairs.
pub fn run_all<F: Factory>() -> Vec<(&'static str, Check)> {
    vec![
        ("counter", counter_acceptance(&mut F::counter())),
        ("todo", todo_acceptance(&mut F::todo())),
        ("chat", chat_acceptance(&mut F::chat())),
        ("settings", settings_acceptance(&mut F::settings())),
        ("counter/hot-reload", counter_hot_reload(&mut F::counter())),
        ("todo/hot-reload", todo_hot_reload(&mut F::todo())),
        ("chat/hot-reload", chat_hot_reload(&mut F::chat())),
        ("settings/hot-reload", settings_hot_reload(&mut F::settings())),
        ("todo_with(1000) shape", todo_with_shape(&mut F::todo_with(1000))),
        ("chat_with(1000) shape", chat_with_shape(&mut F::chat_with(1000))),
    ]
}

pub fn todo_with_shape(h: &mut Window) -> Check {
    ensure_eq!(h.kernel().child_ids("todos").len(), 1000);
    ensure_eq!(text(h, "title-500"), Some("Item 500".into()));
    ensure_eq!(text(h, "left"), Some("1000 left".into()));
    Ok(())
}

fn chat_with_shape(h: &mut Window) -> Check {
    ensure_eq!(h.kernel().child_ids("messages").len(), 1000);
    ensure_eq!(text(h, "msg-role-999"), Some("user".into()));
    ensure_eq!(text(h, "msg-body-1000"), Some("message 1000".into()));
    ensure_eq!(text(h, "header"), Some("1000 messages".into()));
    ensure_eq!(text(h, "status"), Some("idle".into()));
    Ok(())
}

/// Asserts every scenario passes; for use from each prototype's test suite.
pub fn assert_all<F: Factory>() {
    let failures: Vec<_> = run_all::<F>().into_iter().filter_map(|(name, r)| r.err().map(|e| format!("[{name}] {e}"))).collect();
    assert!(failures.is_empty(), "{} failed:\n{}", F::NAME, failures.join("\n"));
}

// ---------------------------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------------------------

/// One measured update: build with `setup`, run `prepare` unmeasured, then `op` `repeat` times.
pub struct BenchStep {
    pub name: &'static str,
    pub app: BenchApp,
    pub prepare: fn(&mut Window) -> Check,
    pub op: fn(&mut Window, usize) -> Check,
    pub repeat: usize,
}

#[derive(Clone, Copy)]
pub enum BenchApp {
    Counter,
    Todo(usize),
    Chat(usize),
    Settings,
}

impl BenchApp {
    pub fn build<F: Factory>(self) -> Window {
        match self {
            BenchApp::Counter => F::counter(),
            BenchApp::Todo(n) => F::todo_with(n),
            BenchApp::Chat(n) => F::chat_with(n),
            BenchApp::Settings => F::settings(),
        }
    }
}

fn ok(_: &mut Window) -> Check {
    Ok(())
}

pub fn bench_steps() -> Vec<BenchStep> {
    vec![
        BenchStep { name: "counter: inc", app: BenchApp::Counter, prepare: ok, op: |h, _| press(h, "inc"), repeat: 100 },
        BenchStep {
            name: "todo(1000): toggle #500",
            app: BenchApp::Todo(1000),
            prepare: ok,
            op: |h, _| toggle(h, "toggle-500"),
            repeat: 20,
        },
        BenchStep {
            name: "todo(1000): add one",
            app: BenchApp::Todo(1000),
            prepare: ok,
            op: |h, _| {
                input(h, "new-todo", "new")?;
                press(h, "add")
            },
            repeat: 20,
        },
        BenchStep {
            name: "todo(1000): type one char in draft",
            app: BenchApp::Todo(1000),
            prepare: ok,
            op: |h, i| input(h, "new-todo", &"x".repeat(i + 1)),
            repeat: 20,
        },
        BenchStep {
            name: "todo(1000): filter all→active",
            app: BenchApp::Todo(1000),
            prepare: |h| toggle(h, "toggle-2"),
            op: |h, i| press(h, if i % 2 == 0 { "filter-active" } else { "filter-all" }),
            repeat: 20,
        },
        BenchStep {
            name: "chat(1000): append token",
            app: BenchApp::Chat(1000),
            prepare: |h| external(h, "user", "go"),
            op: |h, _| external(h, "token", "tok "),
            repeat: 100,
        },
        BenchStep {
            name: "settings: toggle notifications",
            app: BenchApp::Settings,
            prepare: ok,
            op: |h, _| toggle(h, "notifications"),
            repeat: 20,
        },
        BenchStep {
            name: "settings: toggle account",
            app: BenchApp::Settings,
            prepare: ok,
            op: |h, _| press(h, "account-toggle"),
            repeat: 20,
        },
    ]
}
