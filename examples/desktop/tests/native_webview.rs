//! Run with OCTARA_TEST_NATIVE_WINDOW=1 from a macOS desktop session.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() -> Result<(), octara_host_native::ShellError> {
    if std::env::var_os("OCTARA_TEST_NATIVE_WINDOW").is_some_and(|enabled| enabled == "1") {
        macos::run()?;
        println!("Verified AppKit WebKit placement, airspace snapshot and restoration, vibrancy, underlay routing, and bridge");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::{MainThreadMarker, rc::Retained};
    use objc2_app_kit::{NSApplication, NSView, NSVisualEffectView};
    use objc2_foundation::NSPoint;
    use objc2_web_kit::WKWebView;
    use octara::model::capture::FrameReceipt;
    use octara::model::paint::Item;
    use octara::module::ModuleError;
    use octara::prelude::*;
    use octara::runtime::WindowId;
    use octara_host_native::{ShellError, ShellHook, run_app_with_hook};
    use octara_webview::{RpcMethod, SchemeResponse, WebViewCommand, WebViewEvent, WebViewProps};
    use serde_json::json;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    const TITLE: &str = "Octara WebKit bridge test";
    const BRIDGE_URL: &str = "https://bridge.invalid/start";
    const FIRST: &str = "data:text/html,%3Ctitle%3EFirst%3C/title%3E";
    const SECOND: &str = "data:text/html,%3Ctitle%3ESecond%3C/title%3E";
    const SCHEME_URL: &str = "octara-fixture://smoke/page";
    const HTML: &str = r#"<!doctype html>
<title>Bridge fixture</title>
<p>Before find marker after</p>
<script>
window.octara.handle("double", async number => number * 2);
window.octara.call("greet", "WebKit")
    .then(greeting => window.octara.postMessage({example: [true, 1, null, "text"], greeting}))
    .catch(error => window.octara.postMessage({error: String(error)}));
</script>"#;

    struct Greet;

    impl RpcMethod for Greet {
        const NAME: &'static str = "greet";
        type Input = String;
        type Output = String;
    }

    struct Double;

    impl RpcMethod for Double {
        const NAME: &'static str = "double";
        type Input = u32;
        type Output = u32;
    }

    pub(super) fn run() -> Result<(), ShellError> {
        let received = Rc::new(RefCell::new(Vec::new()));
        let source = Rc::new(Cell::new(None));
        let moved = Rc::new(Cell::new(None));
        let cover = Rc::new(Cell::new(None));
        let overlay_loaded = Rc::new(Cell::new(false));
        let options = WindowOptions {
            presentation: WindowPresentation {
                title: TITLE.into(),
                material: WindowMaterial::Vibrancy(VibrancyMaterial::Sidebar),
                ..Default::default()
            },
            ..Default::default()
        };
        let app = AppDefinition::new().with_module(octara_webview::module()).window(WindowDefinition::new(options, {
            let received = received.clone();
            let source = source.clone();
            let moved = moved.clone();
            let cover = cover.clone();
            let overlay_loaded = overlay_loaded.clone();
            move |build| {
                let props = build.signal(with_fixture_scheme(
                    WebViewProps::html(HTML)
                        .base_url(BRIDGE_URL)
                        .trust_origin("https://bridge.invalid")
                        .on_rpc::<Greet>(|name| Ok(format!("Hello, {name}"))),
                ));
                source.set(Some(props));
                let placement = build.signal(false);
                moved.set(Some(placement));
                let occluded = build.signal(false);
                cover.set(Some(occluded));
                el().position(Position::Relative)
                    .size(rel(1.0))
                    .child(
                        octara_webview::view(move |read: &mut ReadCx<'_>| read.get(props))
                            .native_slot(NativeSlot::Underlay)
                            .test_id("bridge")
                            .position(Position::Absolute)
                            .style_with(move |read, style| style.left(if read.get(placement) { 220 } else { 20 }))
                            .top(20)
                            .w(260)
                            .h(180)
                            .on_typed_native::<WebViewEvent>(move |_, _, event| received.borrow_mut().push(event.clone())),
                    )
                    .child(el().position(Position::Absolute).left(30).top(30).size(54).bg(rgb(0x3264a8)))
                    .child(
                        octara_webview::view(WebViewProps::html("<!doctype html><title>Airspace</title><body style='background:#d62239'>"))
                            .native_slot(NativeSlot::Overlay)
                            .test_id("overlay-webkit")
                            .position(Position::Absolute)
                            .left(20)
                            .top(235)
                            .w(160)
                            .h(90)
                            .on_typed_native::<WebViewEvent>(move |_, _, event| {
                                if matches!(event, WebViewEvent::Loaded { .. }) {
                                    overlay_loaded.set(true);
                                }
                            }),
                    )
                    .portal_part(
                        move |read| read.get(occluded),
                        |_| el().position(Position::Absolute).left(100).top(250).w(46).h(36).bg(rgb(0x3264a8)),
                    )
            }
        }));
        run_app_with_hook(
            app,
            Bridge {
                started: Instant::now(),
                received,
                events: Vec::new(),
                source,
                moved,
                cover,
                overlay_loaded,
                overlay_clip: None,
                step: Step::Load,
            },
        )
    }

    fn with_fixture_scheme(props: WebViewProps) -> WebViewProps {
        props.with_scheme("octara-fixture", |request| {
            if request.url != SCHEME_URL || request.method != "GET" {
                return Err(ModuleError::InvalidArgs(format!("unexpected scheme request {request:?}")));
            }
            Ok(SchemeResponse {
                content_type: "text/html".into(),
                body: b"<!doctype html><title>Custom scheme</title><p>Served by the Octara fixture</p>".to_vec(),
            })
        })
    }

    #[derive(Debug)]
    enum Step {
        Load,
        Move(u64),
        Evaluate,
        Find,
        Clear,
        First,
        DisplayOnly,
        Second,
        Back,
        Forward,
        Reload(usize),
        CustomScheme,
        AirspaceCovered,
        AirspaceReopened,
        AirspaceRestored,
    }

    struct Bridge {
        started: Instant,
        received: Rc<RefCell<Vec<WebViewEvent>>>,
        events: Vec<WebViewEvent>,
        source: Rc<Cell<Option<Signal<WebViewProps>>>>,
        moved: Rc<Cell<Option<Signal<bool>>>>,
        cover: Rc<Cell<Option<Signal<bool>>>>,
        overlay_loaded: Rc<Cell<bool>>,
        overlay_clip: Option<Retained<NSView>>,
        step: Step,
    }

    impl ShellHook for Bridge {
        fn tick(&mut self, app: &mut App, presented: &dyn Fn(WindowId) -> Option<FrameReceipt>) -> Result<Option<Duration>, ShellError> {
            let Some((&window, _)) = app.runtime().windows().next() else { return Ok(None) };
            let frame = presented(window);
            self.events.append(&mut self.received.borrow_mut());
            let events = &self.events;
            if self.started.elapsed() > Duration::from_secs(15) {
                return Err(ShellError(format!("WebKit bridge stalled at {:?}. Events: {events:?}", self.step)));
            }
            let load_count = || events.iter().filter(|event| matches!(event, WebViewEvent::Loaded { .. })).count();
            let message = events.iter().find_map(|event| match event {
                WebViewEvent::PostMessage(payload) => Some(payload),
                _ => None,
            });
            let result = |id: &str| {
                events.iter().rev().find_map(|event| match event {
                    WebViewEvent::JavaScript { request_id, result } if request_id.as_deref() == Some(id) => Some(result),
                    _ => None,
                })
            };
            let double = events.iter().find_map(|event| match event {
                WebViewEvent::Rpc(reply) if reply.request_id == "double" => Some(reply),
                _ => None,
            });
            let navigation = events.iter().rev().find_map(|event| match event {
                WebViewEvent::Navigation(state) => Some(state),
                _ => None,
            });
            let loaded_url = events.iter().rev().find_map(|event| match event {
                WebViewEvent::Loaded { url } => Some(url),
                _ => None,
            });
            let settled = |title: &str| {
                navigation.filter(|state| !state.loading && state.title.as_deref() == Some(title) && loaded_url == Some(&state.url))
            };
            match self.step {
                Step::Load
                    if frame.as_ref().is_some_and(|frame| frame.gpu_complete)
                        && message.is_some()
                        && settled("Bridge fixture").is_some() =>
                {
                    assert_eq!(message, Some(&json!({"example": [true, 1, null, "text"], "greeting": "Hello, WebKit"})));
                    assert!(events.iter().any(|event| matches!(event, WebViewEvent::Navigation(state) if state.loading)));
                    let navigation = settled("Bridge fixture").expect("a settled navigation event");
                    assert_eq!(navigation.url.as_deref(), Some(BRIDGE_URL));
                    assert!(!navigation.can_go_back);
                    assert!(!navigation.can_go_forward);

                    assert_underlay_routing(false);
                    let moved = self.moved.get().expect("mounted placement signal");
                    app.update(|write| write.set(moved, true)).map_err(|error| ShellError(error.to_string()))?;
                    self.step = Step::Move(frame.expect("presented frame").stamp.native_rev);
                }
                Step::Move(previous_revision) => {
                    let moved = frame.is_some_and(|frame| frame.stamp.native_rev > previous_revision);
                    assert_underlay_routing(moved);
                    if !moved {
                        return Ok(Some(Duration::from_millis(8)));
                    }
                    evaluate(app, window, "data", "({ok: true, data: [1, null, 'value']})")?;
                    evaluate(app, window, "error", "throw new Error('expected evaluation failure')")?;
                    let queued = app
                        .update_window(window, |write| octara_webview::send_rpc::<Double>(write, "bridge", "double", 21))
                        .map_err(|error| ShellError(error.to_string()))?
                        .map_err(|error| ShellError(error.to_string()))?;
                    assert!(queued, "mounted WebKit view accepts RPC");
                    self.step = Step::Evaluate;
                }
                Step::Evaluate if result("data").is_some() && result("error").is_some() && double.is_some() => {
                    assert_eq!(result("data"), Some(&Ok(json!({"ok": true, "data": [1, null, "value"]}))));
                    assert!(matches!(result("error"), Some(Err(ModuleError::Failed(_)))));
                    assert_eq!(double.expect("a completed RPC").decode::<Double>(), Ok(42));
                    send(app, window, WebViewCommand::Find { query: "find marker".into(), case_sensitive: true, backwards: true })?;
                    self.step = Step::Find;
                }
                Step::Find => {
                    if matches!(result("find"), Some(Ok(value)) if value == "find marker") {
                        send(app, window, WebViewCommand::ClearFind)?;
                        self.step = Step::Clear;
                    } else {
                        evaluate(app, window, "find", "window.getSelection().toString()")?;
                    }
                }
                Step::Clear => {
                    if matches!(result("clear"), Some(Ok(value)) if value == "") {
                        self.navigate(app, FIRST)?;
                        self.step = Step::First;
                    } else {
                        evaluate(app, window, "clear", "window.getSelection().toString()")?;
                    }
                }
                Step::First if settled("First").is_some() => {
                    evaluate(app, window, "display-only", "document.title")?;
                    self.step = Step::DisplayOnly;
                }
                Step::DisplayOnly if result("display-only").is_some() => {
                    assert_eq!(result("display-only"), Some(&Err(ModuleError::PermissionDenied)));
                    self.navigate(app, SECOND)?;
                    self.step = Step::Second;
                }
                Step::Second if settled("Second").is_some_and(|state| state.can_go_back) => {
                    send(app, window, WebViewCommand::GoBack)?;
                    self.step = Step::Back;
                }
                Step::Back if settled("First").is_some_and(|state| state.can_go_forward) => {
                    send(app, window, WebViewCommand::GoForward)?;
                    self.step = Step::Forward;
                }
                Step::Forward if settled("Second").is_some_and(|state| state.can_go_back && !state.can_go_forward) => {
                    send(app, window, WebViewCommand::Reload)?;
                    self.step = Step::Reload(load_count());
                }
                Step::Reload(loads) if load_count() > loads => {
                    assert!(navigation.is_some_and(|state| state.url.as_deref() == Some(SECOND)));
                    self.navigate(app, SCHEME_URL)?;
                    self.step = Step::CustomScheme;
                }
                Step::CustomScheme if settled("Custom scheme").is_some() && self.overlay_loaded.get() => {
                    assert_eq!(loaded_url.and_then(|url| url.as_deref()), Some(SCHEME_URL));
                    let clip = webkit_clip(true);
                    assert!(!clip.isHidden());
                    self.overlay_clip = Some(clip);
                    let cover = self.cover.get().expect("mounted airspace cover");
                    app.update(|w| w.set(cover, true)).map_err(|error| ShellError(error.to_string()))?;
                    self.step = Step::AirspaceCovered;
                }
                Step::AirspaceCovered if matches!(snapshot_pixel(app, window, frame), Some(Some(_))) => {
                    let [r, g, b, a] = snapshot_pixel(app, window, frame).flatten().unwrap();
                    assert!(r > 150 && g < 100 && b < 130 && a == 255, "frozen WebKit pixels were {r}, {g}, {b}, {a}");
                    let clip = webkit_clip(true);
                    assert!(clip.isHidden());
                    assert!(std::ptr::eq(&*clip, &**self.overlay_clip.as_ref().unwrap()));
                    assert!(!webkit_clip(false).isHidden());
                    let cover = self.cover.get().expect("mounted airspace cover");
                    app.update(|w| w.set(cover, false)).map_err(|error| ShellError(error.to_string()))?;
                    assert!(projected_snapshot_pixel(app, window).is_none());
                    // Publish the re-opened overlay before the shell can present the close.
                    app.update(|w| w.set(cover, true)).map_err(|error| ShellError(error.to_string()))?;
                    assert_eq!(projected_snapshot_pixel(app, window), Some([r, g, b, a]));
                    self.step = Step::AirspaceReopened;
                }
                Step::AirspaceReopened if matches!(snapshot_pixel(app, window, frame), Some(Some(_))) => {
                    assert!(webkit_clip(true).isHidden());
                    let cover = self.cover.get().expect("mounted airspace cover");
                    app.update(|w| w.set(cover, false)).map_err(|error| ShellError(error.to_string()))?;
                    self.step = Step::AirspaceRestored;
                }
                Step::AirspaceRestored if matches!(snapshot_pixel(app, window, frame), Some(None)) => {
                    let clip = webkit_clip(true);
                    assert!(!clip.isHidden());
                    assert!(std::ptr::eq(&*clip, &**self.overlay_clip.as_ref().unwrap()));
                    app.close_window(window).map_err(|error| ShellError(error.to_string()))?;
                    return Ok(None);
                }
                _ => {}
            }
            Ok(Some(Duration::from_millis(40)))
        }
    }

    // Outer None means the latest projection has not completed presentation yet.
    fn snapshot_pixel(app: &App, window: WindowId, frame: Option<FrameReceipt>) -> Option<Option<[u8; 4]>> {
        let commit = app.runtime().window(window)?.last_commit()?;
        let frame = frame.filter(|frame| frame.gpu_complete)?;
        (commit.frame == Some(frame.stamp.native_rev)).then(|| projected_snapshot_pixel(app, window))
    }

    fn projected_snapshot_pixel(app: &App, window: WindowId) -> Option<[u8; 4]> {
        let runtime = app.runtime().window(window)?;
        let &[id] = runtime.tree().test_ids("overlay-webkit") else { return None };
        runtime.last_commit()?.draw.as_ref()?.placed_items().find_map(|item| {
            let Item::Image(image) = item.item else { return None };
            if image.source != id {
                return None;
            }
            let sampled = image.image.frame();
            let center = (sampled.height as usize / 2) * sampled.width as usize + sampled.width as usize / 2;
            sampled.rgba.as_chunks::<4>().0.get(center).copied()
        })
    }

    impl Bridge {
        fn navigate(&self, app: &mut App, url: &str) -> Result<(), ShellError> {
            let props = self.source.get().expect("mounted source");
            app.update(|write| write.set(props, with_fixture_scheme(WebViewProps::url(url)))).map_err(|error| ShellError(error.to_string()))
        }
    }

    fn send(app: &mut App, window: WindowId, command: WebViewCommand) -> Result<(), ShellError> {
        let queued = app.update_window(window, |write| octara_webview::send(write, "bridge", command));
        assert!(queued.map_err(|error| ShellError(error.to_string()))?, "mounted WebKit view accepts commands");
        Ok(())
    }

    fn evaluate(app: &mut App, window: WindowId, request_id: &str, script: &str) -> Result<(), ShellError> {
        send(app, window, WebViewCommand::Evaluate { request_id: Some(request_id.into()), script: script.into() })
    }

    fn assert_underlay_routing(moved: bool) {
        let app = NSApplication::sharedApplication(MainThreadMarker::new().expect("main thread"));
        let window = app.windows().iter().find(|window| window.title().to_string() == TITLE).expect("test window");
        assert!(!window.isOpaque(), "Metal must allow the underlay to show through");
        let content = window.contentView().expect("test window content view");
        let subviews = content.subviews();
        let effect = subviews.iter().find_map(|view| view.downcast::<NSVisualEffectView>().ok()).expect("window vibrancy");
        let clip = webkit_clip(false);
        assert!(effect.layer().expect("vibrancy layer").zPosition() < clip.layer().expect("WebKit clip layer").zPosition());
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
        assert_ne!(hits_webkit(140.0, 55.0), moved, "left hit region must match the presented frame");
        assert_eq!(hits_webkit(350.0, 55.0), moved, "right hit region must match the presented frame");
        assert!(!hits_webkit(40.0, 40.0), "a point covered by Octara belongs to Octara");
    }

    fn webkit_clip(overlay: bool) -> Retained<NSView> {
        let app = NSApplication::sharedApplication(MainThreadMarker::new().expect("main thread"));
        let window = app.windows().iter().find(|window| window.title().to_string() == TITLE).expect("test window");
        let content = window.contentView().expect("test window content view");
        content
            .subviews()
            .iter()
            .find(|view| {
                view.layer().is_some_and(|layer| (layer.zPosition() == 2.0) == overlay)
                    && view.subviews().iter().any(|child| child.downcast_ref::<WKWebView>().is_some())
            })
            .expect("WebKit clip view")
    }
}
