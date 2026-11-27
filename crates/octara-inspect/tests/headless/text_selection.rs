use octara::{
    App,
    prelude::*,
    runtime::{HostCommand, Input},
    tree::editor::EditorSession,
};
use octara_inspect::HeadlessDriver;
use octara_model::{
    ask::{ServiceOutcome, ServiceRequest},
    input::{Button, EditorEvent, HostReply, InteractionEvent, Key, KeyEvent, Modifiers, PointerId, PointerKind},
    text::TextFragment,
    theme::ColorToken,
};
use octara_pipeline::{NativeEngine, NativeEngineConfig, NativePublication};
use std::time::Duration;

fn options() -> WindowOptions {
    WindowOptions { viewport: WindowViewport { size: Size::new(260.0, 180.0), scale: 1.0, safe_area: None }, ..Default::default() }
}

fn native_app() -> (App, NativePublication) {
    let publication = NativePublication::default();
    let app = App::new({
        let publication = publication.clone();
        move |_, _| {
            Box::new(NativeEngine::with_publication(
                NativeEngineConfig { check_paint_cache: true, check_a11y_cache: true, ..Default::default() },
                publication.clone(),
            ))
        }
    });
    (app, publication)
}

fn id(app: &App, window: WindowId, name: &str) -> octara_model::identity::NodeId {
    app.runtime().window(window).unwrap().tree().test_ids(name)[0]
}

fn key(app: &mut App, window: WindowId, key: Key, modifiers: Modifiers) {
    let event = InteractionEvent::Key(KeyEvent::new(key, modifiers));
    assert!(app.dispatch(window, Input::Interaction(event)).unwrap().handled);
}

fn down(app: &mut App, window: WindowId, pos: Point, mods: Modifiers) {
    let target = app.runtime().window(window).unwrap().hit_test(pos).expect("pointer target");
    let event = InteractionEvent::PointerDown { id: PointerId(1), kind: PointerKind::Mouse, target, pos, button: Button::Primary, mods };
    assert!(app.dispatch(window, Input::Interaction(event)).unwrap().delivered);
}

fn up(app: &mut App, window: WindowId, pos: Point) {
    let target = app.runtime().window(window).unwrap().hit_test(pos);
    let event = InteractionEvent::PointerUp { id: PointerId(1), target, pos, mods: Modifiers::empty() };
    assert!(app.dispatch(window, Input::Interaction(event)).unwrap().delivered);
}

fn click(app: &mut App, window: WindowId, pos: Point, mods: Modifiers) {
    down(app, window, pos, mods);
    up(app, window, pos);
}

fn selection(app: &App, window: WindowId, node: octara_model::identity::NodeId) -> (usize, usize) {
    app.runtime().window(window).unwrap().tree().editor(node).unwrap().selection()
}

#[test]
fn headless_editor_fallback_keeps_unicode_selections_and_clipboard_edits() {
    let mut driver = HeadlessDriver::default();
    let app = driver.app_mut();
    let window = app
        .open_window(options(), |cx| {
            let value = cx.signal("é🚀 word 文".to_owned());
            input().w(220).h(30).value(value).on_input(move |w, text| w.set(value, text.into())).test_id("field")
        })
        .unwrap();
    let field = id(app, window, "field");
    click(app, window, Point::new(2.0, 10.0), Modifiers::empty());
    key(app, window, Key::Right, Modifiers::empty());
    assert_eq!(selection(app, window, field), (2, 2));
    key(app, window, Key::Right, Modifiers::SHIFT);
    assert_eq!(selection(app, window, field), (2, 6));
    key(app, window, Key::Char('c'), Modifiers::META);
    assert!(app.runtime_mut().take_host_commands().iter().any(|command| matches!(
        command,
        HostCommand::Service { request: ServiceRequest::WriteClipboardText { text }, .. } if text == "🚀"
    )));
    key(app, window, Key::End, Modifiers::empty());
    key(app, window, Key::Left, Modifiers::CTRL);
    assert_eq!(selection(app, window, field), (12, 12));
    key(app, window, Key::Backspace, Modifiers::ALT);
    let value = |app: &App| app.runtime().window(window).unwrap().tree().editor(field).unwrap().value().to_owned();
    assert_eq!(value(app), "é🚀 文");
    key(app, window, Key::Home, Modifiers::empty());
    key(app, window, Key::Delete, Modifiers::empty());
    assert_eq!(value(app), "🚀 文");
    key(app, window, Key::Char('a'), Modifiers::CTRL);
    key(app, window, Key::Char('x'), Modifiers::CTRL);
    let cut = app.runtime_mut().take_host_commands().into_iter().find_map(|command| match command {
        HostCommand::Service { id, request: ServiceRequest::WriteClipboardText { text }, .. } if text == "🚀 文" => Some(id),
        _ => None,
    });
    app.dispatch(window, Input::Host(HostReply::ServiceReply { id: cut.unwrap(), outcome: ServiceOutcome::ClipboardWritten })).unwrap();
    assert_eq!(value(app), "");
    key(app, window, Key::Char('v'), Modifiers::CTRL);
    let paste = app.runtime_mut().take_host_commands().into_iter().find_map(|command| match command {
        HostCommand::Service { id, request: ServiceRequest::ReadClipboardText, .. } => Some(id),
        _ => None,
    });
    app.dispatch(
        window,
        Input::Host(HostReply::ServiceReply { id: paste.unwrap(), outcome: ServiceOutcome::ClipboardText("中\r\n文".into()) }),
    )
    .unwrap();
    assert_eq!(value(app), "中文");
    assert_eq!(selection(app, window, field), (6, 6));
}

#[test]
fn editor_pointer_uses_settled_composition_and_wrapped_geometry() {
    let (mut app, publication) = native_app();
    let window = app
        .open_window(options(), |cx| {
            let value = cx.signal("élan two\nline two\n第三行\nlast".to_owned());
            textarea().w(120).rows(2, 2).value(value).on_input(move |w, text| w.set(value, text.into())).test_id("field")
        })
        .unwrap();
    let field = id(&app, window, "field");
    click(&mut app, window, Point::new(1.0, 8.0), Modifiers::empty());
    app.advance_to(Duration::from_secs(1)).unwrap();
    let point = |line: usize, fragment: usize| {
        let frame = publication.latest().unwrap();
        let geometry = frame.editor_geometry.as_ref().unwrap();
        let TextFragment { rect, .. } = geometry.lines[line].fragments[fragment];
        Point::new(geometry.origin.x + rect.x + rect.w * 0.25, geometry.origin.y + rect.y + rect.h * 0.5)
    };
    let first = point(0, 1);
    click(&mut app, window, first, Modifiers::empty());
    click(&mut app, window, first, Modifiers::empty());
    assert_eq!(selection(&app, window, field), (0, 5));

    down(&mut app, window, first, Modifiers::empty());
    assert_eq!(selection(&app, window, field), (0, 9));
    up(&mut app, window, point(1, 1));
    assert_eq!(selection(&app, window, field), (0, 18));
    key(&mut app, window, Key::Down, Modifiers::SHIFT);
    assert_eq!(selection(&app, window, field), (0, 28));
    assert!(publication.latest().unwrap().editor_geometry.as_ref().unwrap().origin.y < 0.0);
    let third = point(2, 0);
    app.advance_to(Duration::from_secs(2)).unwrap();
    click(&mut app, window, third, Modifiers::empty());
    assert_eq!(selection(&app, window, field), (19, 19));

    let session = EditorSession::active(app.runtime().window(window).unwrap().tree()).unwrap();
    let composing = EditorEvent::Composition {
        session: session.id(),
        expected_rev: session.editor().document_rev(),
        text: "あ".into(),
        selection: Some((3, 3)),
        replacement: None,
    };
    app.dispatch(window, Input::Editor(composing)).unwrap();
    let third = point(2, 1);
    app.advance_to(Duration::from_secs(3)).unwrap();
    click(&mut app, window, third, Modifiers::empty());
    let editor = app.runtime().window(window).unwrap().tree().editor(field).unwrap();
    assert!(editor.composition().is_none());
    assert_eq!(editor.value(), "élan two\nline two\nあ第三行\nlast");
    assert_eq!(editor.selection(), (22, 22));
}

#[test]
fn double_click_drag_uses_words_and_single_click_uses_horizontal_scroll() {
    let (mut app, publication) = native_app();
    let content = "élan two followed by a long horizontal run 尾字";
    let window = app
        .open_window(options(), |cx| {
            let value = cx.signal(content.to_owned());
            let empty = cx.signal(String::new());
            col()
                .child(input().w(95).h(30).value(value).test_id("field"))
                .child(input().w(95).h(30).secure(true).value(value).test_id("secure"))
                .child(input().w(95).h(30).placeholder("Placeholder").value(empty).test_id("empty"))
        })
        .unwrap();
    let [field, secure, empty] = ["field", "secure", "empty"].map(|name| id(&app, window, name));
    let point = |byte| {
        let frame = publication.latest().unwrap();
        let geometry = frame.editor_geometry.as_ref().unwrap();
        let fragment = geometry.lines[0].fragments.iter().find(|fragment| fragment.range.start == byte).unwrap();
        let rect = fragment.rect;
        Point::new(geometry.origin.x + rect.x + rect.w * 0.25, geometry.origin.y + rect.y + rect.h * 0.5)
    };
    click(&mut app, window, Point::new(2.0, 10.0), Modifiers::empty());
    app.advance_to(Duration::from_secs(1)).unwrap();
    let first = point(2);
    let second = point(6);
    click(&mut app, window, second, Modifiers::empty());
    down(&mut app, window, second, Modifiers::empty());
    assert_eq!(selection(&app, window, field), (6, 9));
    up(&mut app, window, first);
    assert_eq!(selection(&app, window, field), (9, 0));
    key(&mut app, window, Key::End, Modifiers::empty());
    let tail = content.len() - "尾字".len();
    let at = point(tail);
    assert!(publication.latest().unwrap().editor_geometry.as_ref().unwrap().origin.x < 0.0);
    assert!(at.x > 0.0 && at.x < 95.0);
    click(&mut app, window, at, Modifiers::empty());
    assert_eq!(selection(&app, window, field), (tail, tail));

    let secure_at = Point::new(4.0, 40.0);
    click(&mut app, window, secure_at, Modifiers::empty());
    click(&mut app, window, secure_at, Modifiers::empty());
    assert_eq!(selection(&app, window, secure), (0, content.len()));
    click(&mut app, window, Point::new(60.0, 70.0), Modifiers::empty());
    assert_eq!(selection(&app, window, empty), (0, 0));
}

#[test]
fn selectable_document_keeps_tree_order_and_pauses_selection_rendering_when_inactive() {
    let (mut app, publication) = native_app();
    let window = app
        .open_window(options(), |cx| {
            let value = cx.signal("editor".to_owned());
            col()
                .child(
                    col()
                        .selectable(true)
                        .test_id("region")
                        .child(text("first").h(30).test_id("one"))
                        .child(text("second").h(30).test_id("two")),
                )
                .child(input().w(140).h(30).value(value).test_id("input"))
                .child(col().selectable(true).child(text("other").focusable().test_id("other")))
        })
        .unwrap();
    let [region, one, two, editor] = ["region", "one", "two", "input"].map(|name| id(&app, window, name));
    let before = Point::new(2.0, 9.0);
    let after = Point::new(90.0, 39.0);
    down(&mut app, window, after, Modifiers::empty());
    up(&mut app, window, before);
    let selected = |app: &App| app.runtime().window(window).unwrap().tree().doc_selection(region);
    assert_eq!(selected(&app), Some([(two, 6), (one, 0)]));
    click(&mut app, window, Point::new(80.0, 9.0), Modifiers::SHIFT);
    assert_eq!(selected(&app), Some([(two, 6), (one, 5)]));
    key(&mut app, window, Key::Char('a'), Modifiers::CTRL);
    assert_eq!(selected(&app), Some([(one, 0), (two, 6)]));
    let other = id(&app, window, "other");
    let target = app.runtime().window(window).unwrap().tree().guard(other);
    app.dispatch(window, Input::Host(HostReply::FocusChanged { target: Some(target), visible: true })).unwrap();
    key(&mut app, window, Key::Char('c'), Modifiers::META);
    assert!(app.runtime_mut().take_host_commands().iter().any(|command| matches!(
        command,
        HostCommand::Service { request: ServiceRequest::WriteClipboardText { text }, .. } if text == "first\nsecond"
    )));

    let target = app.runtime().window(window).unwrap().tree().guard(editor);
    app.dispatch(window, Input::Host(HostReply::FocusChanged { target: Some(target), visible: true })).unwrap();
    key(&mut app, window, Key::Char('a'), Modifiers::META);
    let tree = app.runtime().window(window).unwrap().tree();
    let normal = tree.theme().get(ColorToken::Selection);
    let dim = ColorToken::TextSubtle.alpha(0.28).resolve(tree.theme()).0;
    let assert_color = |expected| {
        let frame = publication.latest().unwrap();
        for node in [one, two, editor] {
            assert!(frame.draw.quads().any(|quad| quad.source == node && quad.fill == Some(expected)));
        }
    };
    assert_color(normal);
    assert!(publication.latest().unwrap().editor_caret.is_some());
    app.dispatch(window, Input::Interaction(InteractionEvent::WindowActive(false))).unwrap();
    assert_color(dim);
    let inactive = publication.latest().unwrap();
    assert!(inactive.editor_caret.is_none());
    app.advance_to(Duration::from_secs(2)).unwrap();
    assert_eq!(publication.latest().unwrap().number, inactive.number);
    app.dispatch(window, Input::Interaction(InteractionEvent::WindowActive(true))).unwrap();
    assert_color(normal);
    assert!(publication.latest().unwrap().editor_caret.is_some());
}
