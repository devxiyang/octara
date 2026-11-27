//! Run with OCTARA_TEST_NATIVE_WINDOW=1 from a macOS desktop session.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() -> Result<(), octara_host_native::ShellError> {
    if std::env::var_os("OCTARA_TEST_NATIVE_WINDOW").is_some() {
        macos::run()?;
        println!("AppKit WebKit Underlay, JS bridge, search, and navigation verified");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSPoint;
    use objc2_web_kit::WKWebView;
    use octara::model::capture::FrameReceipt;
    use octara::prelude::*;
    use octara::runtime::WindowId;
    use octara_host_native::{ShellError, ShellHook, run_app_with_hook};
    use serde_json::{Value, json};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    const TITLE: &str = "Octara WebKit bridge test";
    const FIRST: &str = "data:text/html,%3Ctitle%3EFirst%3C/title%3E";
    const SECOND: &str = "data:text/html,%3Ctitle%3ESecond%3C/title%3E";
    const HTML: &str = r#"<!doctype html>
<title>Bridge fixture</title>
<p>Before find marker after</p>
<script>window.webkit.messageHandlers.octara.postMessage({example: [true, 1, null, "text"]})</script>"#;

    pub(super) fn run() -> Result<(), ShellError> {
        let received = Rc::new(RefCell::new(Vec::new()));
        let source = Rc::new(Cell::new(None));
        let app = AppDefinition::new().window(WindowDefinition::new(WindowOptions { title: TITLE.into(), ..Default::default() }, {
            let received = received.clone();
            let source = source.clone();
            move |build| {
                let props = build.signal(json!({"html": HTML, "base_url": "https://bridge.invalid/start"}));
                source.set(Some(props));
                el().position(Position::Relative)
                    .size(rel(1.0))
                    .child(
                        native_view("web")
                            .native_slot(NativeSlot::Underlay)
                            .test_id("bridge")
                            .position(Position::Absolute)
                            .left(20)
                            .top(20)
                            .w(260)
                            .h(180)
                            .native_props(move |read: &mut ReadCx<'_>| read.get(props))
                            .on_native(move |_, event| received.borrow_mut().push(event.clone())),
                    )
                    .child(el().position(Position::Absolute).left(30).top(30).size(54).bg(rgb(0x3264a8)))
            }
        }));
        run_app_with_hook(app, Bridge { started: Instant::now(), received, source, step: Step::Load })
    }

    #[derive(Debug)]
    enum Step {
        Load,
        Evaluate,
        Find,
        Clear,
        First,
        Second,
        Back,
        Forward,
        Reload(usize),
    }

    struct Bridge {
        started: Instant,
        received: Rc<RefCell<Vec<NativeEvent>>>,
        source: Rc<Cell<Option<Signal<Value>>>>,
        step: Step,
    }

    impl ShellHook for Bridge {
        fn tick(&mut self, app: &mut App, presented: &dyn Fn(WindowId) -> Option<FrameReceipt>) -> Result<Option<Duration>, ShellError> {
            let Some((&window, _)) = app.runtime().windows().next() else { return Ok(None) };
            let ready = presented(window).is_some_and(|frame| frame.gpu_complete);
            let events = self.received.borrow();
            if self.started.elapsed() > Duration::from_secs(15) {
                return Err(ShellError(format!("WebKit bridge stalled at {:?}. Events: {events:?}", self.step)));
            }
            let event = |name: &str| events.iter().find(|event| event.name == name).map(|event| &event.data);
            let result = |id: &str, key: &str| {
                events.iter().rev().find(|event| event.name == "js_result" && event.data["request_id"] == id).map(|event| &event.data[key])
            };
            let navigation = events.iter().rev().find(|event| event.name == "navigation").map(|event| &event.data);
            match self.step {
                Step::Load
                    if ready
                        && event("load").is_some()
                        && event("post_message").is_some()
                        && navigation.is_some_and(|state| state["title"] == "Bridge fixture" && state["loading"] == false) =>
                {
                    assert_eq!(event("post_message"), Some(&json!({"example": [true, 1, null, "text"]})));
                    assert!(events.iter().any(|event| event.name == "title" && event.data["title"] == "Bridge fixture"));
                    assert!(events.iter().any(|event| event.name == "navigation" && event.data["loading"] == true));
                    let navigation = navigation.expect("a loading-state event");
                    assert_eq!(navigation["url"], "https://bridge.invalid/start");
                    assert_eq!(navigation["title"], "Bridge fixture");
                    assert_eq!(navigation["loading"], false);
                    assert_eq!(navigation["can_go_back"], false);
                    assert_eq!(navigation["can_go_forward"], false);

                    assert_underlay_routing();
                    evaluate(app, window, "data", "({ok: true, data: [1, null, 'value']})")?;
                    evaluate(app, window, "error", "throw new Error('expected evaluation failure')")?;
                    self.step = Step::Evaluate;
                }
                Step::Evaluate if result("data", "value").is_some() && result("error", "error").is_some() => {
                    assert_eq!(result("data", "value"), Some(&json!({"ok": true, "data": [1, null, "value"]})));
                    assert!(result("error", "error").is_some_and(Value::is_string));
                    send(app, window, "find", json!({"query": "find marker", "case_sensitive": true, "backwards": true}))?;
                    self.step = Step::Find;
                }
                Step::Find => {
                    if result("find", "value").is_some_and(|value| value == "find marker") {
                        send(app, window, "clear_find", Value::Null)?;
                        self.step = Step::Clear;
                    } else {
                        evaluate(app, window, "find", "window.getSelection().toString()")?;
                    }
                }
                Step::Clear => {
                    if result("clear", "value").is_some_and(|value| value == "") {
                        self.navigate(app, FIRST)?;
                        self.step = Step::First;
                    } else {
                        evaluate(app, window, "clear", "window.getSelection().toString()")?;
                    }
                }
                Step::First if navigation.is_some_and(|state| state["title"] == "First" && state["loading"] == false) => {
                    self.navigate(app, SECOND)?;
                    self.step = Step::Second;
                }
                Step::Second
                    if navigation
                        .is_some_and(|state| state["can_go_back"] == true && state["title"] == "Second" && state["loading"] == false) =>
                {
                    send(app, window, "go_back", Value::Null)?;
                    self.step = Step::Back;
                }
                Step::Back
                    if navigation
                        .is_some_and(|state| state["can_go_forward"] == true && state["title"] == "First" && state["loading"] == false) =>
                {
                    send(app, window, "go_forward", Value::Null)?;
                    self.step = Step::Forward;
                }
                Step::Forward
                    if navigation.is_some_and(|state| {
                        state["can_go_back"] == true
                            && state["can_go_forward"] == false
                            && state["title"] == "Second"
                            && state["loading"] == false
                    }) =>
                {
                    let loads = events.iter().filter(|event| event.name == "load").count();
                    send(app, window, "reload", Value::Null)?;
                    self.step = Step::Reload(loads);
                }
                Step::Reload(loads) if events.iter().filter(|event| event.name == "load").count() > loads => {
                    assert!(navigation.is_some_and(|state| state["url"] == SECOND));
                    app.close_window(window).map_err(|error| ShellError(error.to_string()))?;
                    return Ok(None);
                }
                _ => {}
            }
            Ok(Some(Duration::from_millis(40)))
        }
    }

    impl Bridge {
        fn navigate(&self, app: &mut App, url: &str) -> Result<(), ShellError> {
            let props = self.source.get().expect("mounted source");
            app.update(|write| write.set(props, json!({ "url": url }))).map_err(|error| ShellError(error.to_string()))
        }
    }

    fn send(app: &mut App, window: WindowId, name: &str, data: Value) -> Result<(), ShellError> {
        let queued = app.update_window(window, |write| write.send_native_command("bridge", name, data));
        assert!(queued.map_err(|error| ShellError(error.to_string()))?, "mounted WebKit view accepts {name}");
        Ok(())
    }

    fn evaluate(app: &mut App, window: WindowId, request_id: &str, script: &str) -> Result<(), ShellError> {
        send(app, window, "evaluate_js", json!({ "request_id": request_id, "script": script }))
    }

    fn assert_underlay_routing() {
        let app = NSApplication::sharedApplication(MainThreadMarker::new().expect("main thread"));
        let window = app.windows().iter().find(|window| window.title().to_string() == TITLE).expect("test window");
        assert!(!window.isOpaque(), "Metal must allow the underlay to show through");
        let content = window.contentView().expect("test window content view");
        let hits_webkit = |x, y| {
            let y = if content.isFlipped() { y } else { content.bounds().size.height - y };
            // SAFETY: Traverse the attached views on AppKit's main thread.
            let parent = unsafe { content.superview() };
            let point = content.convertPoint_toView(NSPoint::new(x, y), parent.as_deref());
            let mut hit = content.hitTest(point);
            while let Some(view) = hit {
                if view.downcast_ref::<WKWebView>().is_some() {
                    return true;
                }
                // SAFETY: The original hit and its ancestors are retained on the main thread.
                hit = unsafe { view.superview() };
            }
            false
        };
        assert!(hits_webkit(140.0, 55.0), "an uncovered point belongs to WebKit");
        assert!(!hits_webkit(40.0, 40.0), "a point covered by Octara belongs to Octara");
    }
}
