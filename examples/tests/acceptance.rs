use octara::diagnostics::codes;
use octara::prelude::*;
use octara::{A11yAction, BackPhase, CommitKind, EditorEvent, Input, InteractionEvent, PointerId, PointerKind, TextEdit};
use octara_examples::{album_back, apps, panel_scroll, showcase, window_options};
use octara_inspect::protocol::{Delivery, NodeSnapshot, TreeSnapshot};
use octara_inspect::{AgentAction, AgentCommand, AgentReceipt, AgentRequest, HeadlessDriver, TargetRef};
use std::time::Duration;

fn launch<V: IntoView>(root: impl FnOnce(&mut BuildCx<'_>) -> V + 'static) -> (HeadlessDriver, WindowId) {
    let mut driver = HeadlessDriver::default();
    let definition = AppDefinition::new()
        .with_module(octara_webview::mock())
        .window(WindowDefinition::new(window_options("Example", 800.0, 600.0), root));
    let window = driver.launch(definition).unwrap()[0];
    (driver, window)
}

fn observe(driver: &mut HeadlessDriver, window: WindowId) -> TreeSnapshot {
    let receipt = driver.execute(AgentRequest::new(Some(window), AgentCommand::Observe));
    assert!(receipt.ok && receipt.progress.settled, "{receipt:?}");
    receipt.snapshot.unwrap()
}

fn node<'a>(snapshot: &'a TreeSnapshot, id: &str) -> &'a NodeSnapshot {
    snapshot.nodes.iter().find(|node| node.test_id.as_deref() == Some(id)).unwrap_or_else(|| panic!("missing #{id}"))
}

fn act(driver: &mut HeadlessDriver, window: WindowId, target: TargetRef, action: AgentAction) -> AgentReceipt {
    driver.execute(AgentRequest::new(Some(window), AgentCommand::Act { target, action }))
}

fn accessible(driver: &mut HeadlessDriver, window: WindowId, id: &str, action: A11yAction) {
    let target = node(&observe(driver, window), id).target;
    let receipt = act(driver, window, target, AgentAction::Accessibility { action });
    assert!(receipt.ok && receipt.handled == Some(true), "{id}: {receipt:?}");
}

fn press(driver: &mut HeadlessDriver, window: WindowId, id: &str) {
    accessible(driver, window, id, A11yAction::Press);
}

fn input(driver: &mut HeadlessDriver, window: WindowId, id: &str, value: &str) {
    accessible(driver, window, id, A11yAction::Focus);
    let snapshot = observe(driver, window);
    let target = node(&snapshot, id).target;
    let editor = snapshot.editor.expect("focused editor");
    let event = EditorEvent::ProposeEdit {
        session: editor.session,
        expected_rev: editor.document_rev,
        edit: TextEdit::Commit { text: value.into(), selection: (value.len(), value.len()), kind: CommitKind::Other },
    };
    let receipt = act(driver, window, target, AgentAction::Editor { event });
    assert!(receipt.ok && receipt.handled == Some(true), "{id}: {receipt:?}");
}

#[test]
fn counter_restarts_with_persisted_state_and_fresh_targets() {
    let mut driver = HeadlessDriver::default();
    let window = driver.launch(apps::counter_app()).unwrap()[0];
    let initial = observe(&mut driver, window);
    let target = node(&initial, "inc").target;
    for _ in 0..3 {
        press(&mut driver, window, "inc");
    }
    let state = observe(&mut driver, window);
    assert_eq!(node(&state, "inc").target, target);
    assert_eq!(node(&state, "count").text.as_deref(), Some("Count: 3"));
    assert_eq!(node(&state, "double").text.as_deref(), Some("Double: 6"));

    let mut restarted = HeadlessDriver::default();
    let new_window = apps::counter_app().launch(restarted.app_mut(), Some(driver.app().export_persisted())).unwrap()[0];
    let current = observe(&mut restarted, new_window);
    assert_eq!(node(&current, "count").text.as_deref(), Some("Count: 3"));
    let stale = act(&mut restarted, new_window, target, AgentAction::Accessibility { action: A11yAction::Press });
    assert_eq!(stale.delivery, Delivery::Refused);
    assert_eq!(stale.code.as_deref(), Some(codes::STALE_TARGET.id));
    press(&mut restarted, new_window, "reset");
    assert_eq!(node(&observe(&mut restarted, new_window), "count").text.as_deref(), Some("Count: 0"));
}

#[test]
fn todos_keep_their_keys_across_edits_and_filters() {
    let (mut driver, window) = launch(|b| apps::todo(b, 3));
    let initial = observe(&mut driver, window);
    let stable = node(&initial, "todo-2").target;
    input(&mut driver, window, "new-todo", "  Buy milk  ");
    press(&mut driver, window, "add");
    assert_eq!(node(&observe(&mut driver, window), "title-4").text.as_deref(), Some("Buy milk"));
    press(&mut driver, window, "toggle-1");
    press(&mut driver, window, "filter-active");
    let filtered = observe(&mut driver, window);
    assert!(!filtered.nodes.iter().any(|node| node.test_id.as_deref() == Some("todo-1")));
    assert_eq!(node(&filtered, "todo-2").target, stable);
    press(&mut driver, window, "clear-done");
    press(&mut driver, window, "filter-all");
    let done = observe(&mut driver, window);
    assert_eq!(node(&done, "left").text.as_deref(), Some("3 left"));
    assert!(!done.nodes.iter().any(|node| node.test_id.as_deref() == Some("clear-done")));
}

#[test]
fn chat_streams_on_the_virtual_clock() {
    let (mut driver, window) = launch(|b| apps::chat(b, 0));
    input(&mut driver, window, "composer", "Hello");
    press(&mut driver, window, "send");
    let partial = observe(&mut driver, window);
    assert_eq!(node(&partial, "msg-body-1").text.as_deref(), Some("Hello"));
    assert_eq!(node(&partial, "status").text.as_deref(), Some("streaming"));
    let reply = node(&partial, "msg-body-2").target;
    driver.advance_to(Duration::from_millis(80)).unwrap();
    let first = observe(&mut driver, window);
    assert_eq!(node(&first, "msg-body-2").text.as_deref(), Some("Got "));
    driver.advance_to(Duration::from_millis(240)).unwrap();
    let done = observe(&mut driver, window);
    assert_eq!(node(&done, "status").text.as_deref(), Some("idle"));
    assert_eq!(node(&done, "msg-body-2").target, reply);
    assert_eq!(node(&done, "msg-body-2").text.as_deref(), Some("Got your message."));
}

#[test]
fn markdown_showcase_replays_with_a_reveal_and_caret_command() {
    let (mut driver, window) = launch(|b| showcase::page(b, "markdown").unwrap());
    let initial = observe(&mut driver, window);
    assert!(node(&initial, "eclipse-chart").label.as_deref().is_some_and(|label| label.contains("Totality 16")));
    let image = initial.nodes.iter().find(|node| node.label.as_deref() == Some("Red Moon against a dark sky")).unwrap();
    let tree = driver.app().runtime().window(window).unwrap().tree();
    let open_image =
        std::iter::successors(Some(image), |node| node.parent.and_then(|parent| initial.nodes.iter().find(|node| node.target == parent)))
            .find(|node| node.target.node().is_some_and(|node| tree.handler(node, HandlerKind::Press).is_some()))
            .expect("the Markdown image has an activation target");
    let receipt = act(&mut driver, window, open_image.target, AgentAction::Accessibility { action: A11yAction::Press });
    assert!(receipt.ok && receipt.handled == Some(true), "{receipt:?}");
    assert!(observe(&mut driver, window).modal.is_some());
    press(&mut driver, window, "eclipse-lightbox-close");

    input(&mut driver, window, "eclipse-composer", "/rep");
    let active = observe(&mut driver, window);
    assert!(active.nodes.iter().any(|node| node.text.as_deref() == Some("/replay · Replay the answer")));
    let editor = active.editor.unwrap();
    let event = EditorEvent::KeyIntercept {
        session: editor.session,
        expected_rev: editor.document_rev,
        key: KeyEvent::new(Key::Enter, Modifiers::empty()),
    };
    let receipt = act(&mut driver, window, active.focus.unwrap(), AgentAction::Editor { event });
    assert!(receipt.ok && receipt.handled == Some(true), "{receipt:?}");
    assert!(!observe(&mut driver, window).nodes.iter().any(|node| node.test_id.as_deref() == Some("eclipse-chart")));

    let mut revealed = false;
    for step in 1..=160 {
        driver.advance_to(Duration::from_millis(step * 25)).unwrap();
        if !revealed {
            let tree = driver.app().runtime().window(window).unwrap().tree();
            revealed = tree.text_reveals().any(|(_, reveal)| !reveal.finished && reveal.revealed_bytes > 0);
        }
    }
    assert!(revealed);
    assert!(node(&observe(&mut driver, window), "eclipse-chart").label.as_deref().is_some_and(|label| label.contains("Totality 16")));
}

#[test]
fn virtual_chat_materializes_only_rows_near_the_viewport() {
    let (mut driver, window) = launch(|b| apps::chat_virtual(b, 10_000));
    let snapshot = observe(&mut driver, window);
    assert_eq!(node(&snapshot, "header").text.as_deref(), Some("10000 messages"));
    let resident = snapshot.nodes.iter().filter_map(|node| node.test_id.as_ref()).filter(|id| id.starts_with("msg-body-")).count();
    assert!((1..100).contains(&resident), "{resident} mounted message rows");
    driver.app_mut().update_window(window, |w| w.reveal_row("messages", 5_000)).unwrap();
    assert_eq!(node(&observe(&mut driver, window), "msg-body-5000").text.as_deref(), Some("message 5000"));
}

#[test]
fn settings_preserve_model_values_when_conditional_fields_remount() {
    let (mut driver, window) = launch(apps::settings);
    press(&mut driver, window, "notifications");
    let initial = observe(&mut driver, window);
    let sound = node(&initial, "sound").target;
    press(&mut driver, window, "sound");
    input(&mut driver, window, "volume", "70");
    press(&mut driver, window, "notifications");
    press(&mut driver, window, "notifications");
    let current = observe(&mut driver, window);
    assert_ne!(node(&current, "sound").target, sound);
    assert_eq!(node(&current, "summary").text.as_deref(), Some("notifications=on sound=off volume=70 theme=system telemetry=off"));
}

#[test]
fn backing_out_of_an_album_retires_the_detail_and_restores_access() {
    let (mut driver, window) = launch(album_back::view);
    press(&mut driver, window, "album-photo-1");
    let shown = observe(&mut driver, window);
    let back = node(&shown, album_back::BACK).target;
    assert_eq!(shown.modal, Some(node(&shown, album_back::DETAIL).target));

    for phase in [BackPhase::Started, BackPhase::Completed] {
        driver.dispatch(window, Input::Interaction(InteractionEvent::BackGesture { id: 17, phase })).unwrap();
    }
    let closed = observe(&mut driver, window);
    assert!(closed.modal.is_none());
    assert_eq!(node(&closed, album_back::COUNTERS).text.as_deref(), Some("Back 1 · Released 1"));
    let stale = act(&mut driver, window, back, AgentAction::Accessibility { action: A11yAction::Press });
    assert_eq!(stale.code.as_deref(), Some(codes::STALE_TARGET.id));
}

#[test]
fn one_touch_delta_resizes_the_panel_and_scrolls_its_list() {
    let (mut driver, window) = launch(panel_scroll::view);
    let before = observe(&mut driver, window);
    let first = node(&before, "pan-place-1");
    let hit = first.geometry.unwrap().rect;
    let list = node(&before, panel_scroll::LIST).geometry.unwrap().rect;
    let at = Point::new(hit.x + 12.0, list.y + hit.y + 20.0);
    let pointer = PointerId(1);
    assert!(
        act(
            &mut driver,
            window,
            first.target,
            AgentAction::PointerDown {
                id: pointer,
                pointer: PointerKind::Touch,
                pos: at,
                button: Button::Primary,
                modifiers: Modifiers::empty(),
            }
        )
        .ok
    );
    assert!(
        act(
            &mut driver,
            window,
            first.target,
            AgentAction::PointerMove { id: pointer, pos: Point::new(at.x, at.y - 40.0), modifiers: Modifiers::empty() }
        )
        .ok
    );
    let moved = observe(&mut driver, window);
    let viewport = node(&moved, panel_scroll::LIST);
    assert!(viewport.geometry.unwrap().rect.h > list.h);
    assert!(viewport.scroll_offset.unwrap().y > 0.0);
    assert!(act(&mut driver, window, first.target, AgentAction::PointerCancel { id: pointer }).ok);
    assert_eq!(node(&observe(&mut driver, window), "pan-activations").text.as_deref(), Some("Activations 0"));
}

#[test]
fn showcase_form_keeps_controls_during_validation_and_reorder() {
    let (mut driver, window) = launch(|b| showcase::page(b, "forms").unwrap());
    let before = observe(&mut driver, window);
    let original = node(&before, "profile-name").target;
    let balanced = node(&before, "reply-style-option-balanced").target;
    input(&mut driver, window, "profile-name", "admin");
    let invalid = observe(&mut driver, window);
    assert_eq!(node(&invalid, "profile-name").target, original);
    assert!(node(&invalid, "profile-name-error").text.as_deref().unwrap().contains("不可用"));
    press(&mut driver, window, "forms-reorder");
    let reordered = observe(&mut driver, window);
    let radio = node(&reordered, "reply-style-option-balanced");
    assert_eq!(radio.target, balanced);
    assert_eq!(radio.label.as_deref(), Some("均衡 · 推荐"));
    input(&mut driver, window, "profile-name", " 小明 ");
    press(&mut driver, window, "forms-save");
    assert_eq!(node(&observe(&mut driver, window), "forms-status").text.as_deref(), Some("已保存：小明 · balanced"));
}

#[test]
fn showcase_dialog_retains_its_draft_and_routes_enter_to_suggestions_first() {
    let (mut driver, window) = launch(|b| showcase::page(b, "interaction").unwrap());
    press(&mut driver, window, "interaction-open");
    input(&mut driver, window, "suggest-editor", "/");
    let draft = node(&observe(&mut driver, window), "suggest-editor").target;
    let enter = Input::Interaction(InteractionEvent::Key(KeyEvent::new(Key::Enter, Modifiers::empty())));
    assert!(driver.dispatch(window, enter.clone()).unwrap().handled);
    let picked = observe(&mut driver, window);
    assert_eq!(node(&picked, "interaction-chosen").text.as_deref(), Some("已选择：review"));
    assert_eq!(node(&picked, "interaction-submissions").text.as_deref(), Some("提交次数：0"));

    press(&mut driver, window, "editor-tabs-tab-preview");
    let preview = observe(&mut driver, window);
    assert_eq!(node(&preview, "editor-preview-text").text.as_deref(), Some("预览：请审阅当前改动。"));
    press(&mut driver, window, "editor-tabs-tab-draft");
    assert_eq!(node(&observe(&mut driver, window), "suggest-editor").target, draft);
    accessible(&mut driver, window, "suggest-editor", A11yAction::Focus);
    assert!(driver.dispatch(window, enter).unwrap().handled);
    assert_eq!(node(&observe(&mut driver, window), "interaction-submissions").text.as_deref(), Some("提交次数：1"));
}

#[test]
fn editor_rejects_a_proposal_for_an_old_document_revision() {
    let (mut driver, window) = launch(|b| apps::undo_document(b, "A"));
    accessible(&mut driver, window, "editor", A11yAction::Focus);
    let view = observe(&mut driver, window);
    let before = view.editor.unwrap();
    let target = node(&observe(&mut driver, window), "editor").target;
    let action = AgentAction::Editor {
        event: EditorEvent::Commit { session: before.session, expected_rev: before.document_rev, text: "你好".into() },
    };
    let receipt = act(&mut driver, window, target, action.clone());
    assert!(receipt.ok, "{receipt:?}");
    let after = observe(&mut driver, window);
    assert!(after.editor.unwrap().value.unwrap().contains("你好"));
    let stale = act(&mut driver, window, target, action);
    assert_eq!(stale.code.as_deref(), Some(codes::STALE_TARGET.id));
}

#[test]
fn all_showcase_cards_publish_a_settled_window() {
    for &(name, _) in showcase::CARDS {
        let (mut driver, window) = launch(move |b| showcase::page(b, name).unwrap());
        let receipt = driver.execute(AgentRequest::new(Some(window), AgentCommand::Observe));
        assert!(receipt.ok && receipt.progress.settled && receipt.diagnostics.is_empty(), "{name}: {receipt:?}");
    }
}

#[test]
fn release_workspace_filters_and_sorts_the_table_and_retains_the_review_draft() {
    let (mut driver, window) = launch(|b| showcase::page(b, "workspace").unwrap());
    let initial = observe(&mut driver, window);
    let tree = initial.nodes.iter().find(|item| item.role == Some(Role::TreeItem) && item.label.as_deref() == Some("Web")).unwrap();
    let receipt = act(&mut driver, window, tree.target, AgentAction::Accessibility { action: A11yAction::Press });
    assert!(receipt.ok && receipt.handled == Some(true), "{receipt:?}");
    let filtered = observe(&mut driver, window);
    assert_eq!(node(&filtered, "workspace-filter").text.as_deref(), Some("Web releases"));
    assert!(!filtered.nodes.iter().any(|row| row.test_id.as_deref() == Some("release-studio") && row.shown));
    let header =
        filtered.nodes.iter().find(|item| item.role == Some(Role::ColumnHeader) && item.label.as_deref() == Some("Release")).unwrap();
    let sort = filtered.nodes.iter().find(|item| item.role == Some(Role::Button) && item.parent == Some(header.target)).unwrap();
    let portal = node(&filtered, "release-portal").target;
    let receipt = act(&mut driver, window, sort.target, AgentAction::Accessibility { action: A11yAction::Press });
    assert!(receipt.ok && receipt.handled == Some(true), "{receipt:?}");
    let sorted = observe(&mut driver, window);
    let row = node(&sorted, "release-portal");
    assert_eq!(row.target, portal);
    let row_y =
        |cell: &NodeSnapshot| sorted.nodes.iter().find(|parent| Some(parent.target) == cell.parent).unwrap().geometry.unwrap().rect.y;
    assert!(row_y(node(&sorted, "release-billing")) < row_y(row));

    press(&mut driver, window, "workspace-commands");
    input(&mut driver, window, "workspace-palette-search", "selected");
    assert!(
        driver.dispatch(window, Input::Interaction(InteractionEvent::Key(KeyEvent::new(Key::Enter, Modifiers::empty())))).unwrap().handled
    );
    assert_eq!(node(&observe(&mut driver, window), "review-release-name").text.as_deref(), Some("Portal 3.2"));
    input(&mut driver, window, "review-draft", "Check the migration notes");
    let draft = node(&observe(&mut driver, window), "review-draft").target;
    for phase in [BackPhase::Started, BackPhase::Completed] {
        driver.dispatch(window, Input::Interaction(InteractionEvent::BackGesture { id: 21, phase })).unwrap();
    }
    assert!(node(&observe(&mut driver, window), "workspace-releases").shown);
    press(&mut driver, window, "workspace-forward");
    let reopened = observe(&mut driver, window);
    assert_eq!(node(&reopened, "review-draft").target, draft);
    assert_eq!(node(&reopened, "review-draft").value.as_deref(), Some("Check the migration notes"));
}

#[test]
fn release_workspace_reorders_keyed_checks_and_announces_menu_actions() {
    let (mut driver, window) = launch(|b| showcase::page(b, "workspace").unwrap());
    let initial = observe(&mut driver, window);
    let smoke = node(&initial, "queue-smoke").target;
    let notes = node(&initial, "queue-notes").target;
    assert_eq!(node(&initial, "queue-notify").checked, Some(Checked::Checked));
    press(&mut driver, window, "queue-notify");
    assert_eq!(node(&observe(&mut driver, window), "queue-notify").checked, Some(Checked::Unchecked));

    accessible(&mut driver, window, "queue-smoke", A11yAction::Focus);
    for key in [Key::Space, Key::Down, Key::Enter] {
        driver.dispatch(window, Input::Interaction(InteractionEvent::Key(KeyEvent::new(key, Modifiers::empty())))).unwrap();
    }
    let reordered = observe(&mut driver, window);
    assert_eq!(reordered.focus, Some(smoke));
    assert_eq!(node(&reordered, "queue-notes").target, notes);
    assert!(node(&reordered, "queue-notes").geometry.unwrap().rect.y < node(&reordered, "queue-smoke").geometry.unwrap().rect.y);
    assert!(!reordered.nodes.iter().any(|node| node.text.as_deref().is_some_and(|text| text.starts_with("Moved "))));

    accessible(&mut driver, window, "queue-actions", A11yAction::ShowContextMenu);
    press(&mut driver, window, "queue-actions-action-queue.reset");
    let reset = observe(&mut driver, window);
    assert_eq!(node(&reset, "queue-smoke").target, smoke);
    assert!(node(&reset, "queue-smoke").geometry.unwrap().rect.y < node(&reset, "queue-notes").geometry.unwrap().rect.y);
    assert!(reset.nodes.iter().any(|node| node.text.as_deref() == Some("Review order reset") && node.shown));
    driver.advance_to(Duration::from_secs(6)).unwrap();
    assert!(!observe(&mut driver, window).nodes.iter().any(|node| node.text.as_deref() == Some("Review order reset") && node.shown));

    press(&mut driver, window, "queue-notify");
    let ready = observe(&mut driver, window);
    let point_in_row = |id, fraction| {
        let row = node(&ready, id);
        let rect = row.geometry.unwrap().rect;
        let mut at = Point::new(rect.right() - 24.0, rect.y + rect.h * fraction);
        let mut parent = row.parent;
        while let Some(target) = parent {
            let node = ready.nodes.iter().find(|node| node.target == target).unwrap();
            let rect = node.geometry.unwrap().rect;
            let scroll = node.scroll_offset.unwrap_or_default();
            at = Point::new(at.x + rect.x - scroll.x, at.y + rect.y - scroll.y);
            parent = node.parent;
        }
        at
    };
    let start = point_in_row("queue-smoke", 0.5);
    let after_last = point_in_row("queue-sign-off", 0.9);
    let id = PointerId(11);
    let modifiers = Modifiers::empty();
    for action in [
        AgentAction::PointerDown { id, pointer: PointerKind::Mouse, pos: start, button: Button::Primary, modifiers },
        AgentAction::PointerMove { id, pos: after_last, modifiers },
        AgentAction::PointerUp { id, pos: after_last, modifiers },
    ] {
        let receipt = act(&mut driver, window, smoke, action);
        assert!(receipt.ok, "{receipt:?}");
    }
    let dropped = observe(&mut driver, window);
    assert!(node(&dropped, "queue-sign-off").geometry.unwrap().rect.y < node(&dropped, "queue-smoke").geometry.unwrap().rect.y);
    assert!(dropped.nodes.iter().any(|node| node.text.as_deref() == Some("Moved \"Run smoke tests\" to position 3") && node.shown));
}

#[test]
fn note_window_actions_affect_only_the_opened_window() {
    let (mut driver, window) = launch(apps::windows);
    driver.stub_module_call::<octara_picker::PickFiles>(|request| {
        assert!(request.multiple);
        Ok(vec![octara_picker::PickedFile {
            path: None,
            name: "note.txt".into(),
            media_type: "text/plain".into(),
            bytes: Some(b"notes".to_vec()),
        }])
    });
    press(&mut driver, window, "open");
    assert_eq!(node(&observe(&mut driver, window), "picked").text.as_deref(), Some("note.txt"));
    press(&mut driver, window, "new");
    let note = driver.app().runtime().windows().find(|&(&id, _)| id != window).map(|(&id, _)| id).expect("opened note");
    input(&mut driver, note, "body", "A note");
    assert_eq!(node(&observe(&mut driver, note), "body").value.as_deref(), Some("A note"));
    press(&mut driver, note, "close");
    assert!(driver.app().runtime().window(note).is_none());
    assert_eq!(node(&observe(&mut driver, window), "opened").text.as_deref(), Some("1 notes opened"));
}

#[test]
fn renaming_keeps_the_editor_session_during_directory_reorder() {
    let (mut driver, window) = launch(octara_examples::rename::view);
    press(&mut driver, window, "rename-start");
    input(&mut driver, window, "rename-editor", "你好.txt");
    let before = observe(&mut driver, window);
    let target = node(&before, "rename-editor").target;
    let session = before.editor.unwrap().session;

    let receipt = act(&mut driver, window, before.target, AgentAction::MenuCommand { name: "rename.reorder".into() });
    assert!(receipt.ok, "{receipt:?}");
    let reordered = observe(&mut driver, window);
    assert_eq!(node(&reordered, "rename-editor").target, target);
    let editor = reordered.editor.unwrap();
    assert_eq!(editor.session, session);
    assert_eq!(editor.value.as_deref(), Some("你好.txt"));
}
