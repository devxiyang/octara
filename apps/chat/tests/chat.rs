//! Product UI tests using guarded Octara Inspect actions and native-equivalent editor input.

#[allow(dead_code)]
#[path = "../examples/support/controlled.rs"]
mod controlled;
#[path = "support/inspect.rs"]
mod inspect;
#[path = "support/session.rs"]
mod session;

use controlled::Controlled;
use octara::prelude::*;
use octara::{BackPhase, EditorEvent, HostCommand, HostReply, Input, InteractionEvent};
use octara_chat::engine::Purpose;
use octara_chat::model::{Attachment, Event, Thread};
use octara_chat::store::Store;
use octara_chat::{ModelEntry, Options, view};
use octara_inspect::AgentAction;
use session::Session;
use std::rc::Rc;
use std::time::Duration;

fn offline() -> Session {
    Session::new(Options::default(), view)
}

fn controlled(store: Option<Store>) -> (Session, Controlled) {
    let model = Controlled::default();
    let options = Options { models: vec![ModelEntry::new("controlled", Rc::new(model.clone()))], store, ..Options::default() };
    (Session::new(options, view), model)
}

fn send(ui: &Session, question: &str) {
    ui.command("chat.focus_composer");
    ui.text(question);
    ui.key(Key::Enter, Modifiers::empty());
}

#[test]
fn scripted_reply_streams_markdown_and_keeps_nested_declarations() {
    let ui = offline();
    assert!(ui.press("starter-learn"));
    assert!(ui.snapshot().editor.unwrap().value.unwrap().contains("simple terms"));
    ui.update(|chat, w| w.set(chat.draft, String::new()));
    send(&ui, "What is the weather?");
    assert!(ui.exists("stop"));
    for step in 1..=48 {
        ui.driver.borrow_mut().advance_to(Duration::from_millis(step * 30)).unwrap();
    }
    assert!(!ui.exists("stop"));
    let reasoning = "reasoning-3-0-trigger";
    assert!(ui.press(reasoning));
    let target = ui.node(reasoning).target;
    assert!(ui.exists("reasoning-text-3-0"));
    assert!(ui.press("tool-3-1-trigger"));
    assert_eq!(ui.node("tool-input-3-1").text.as_deref(), Some("{\"city\": \"Oslo\"}"));
    assert!(ui.node("tool-output-3-1").text.unwrap().contains("cloudy"));
    assert_eq!(ui.node(reasoning).target, target);
    assert!(ui.snapshot().nodes.iter().any(|n| n.text.as_deref().is_some_and(|t| t.contains("cloudy in Oslo"))));
    ui.clean();
}

#[test]
fn composer_commands_and_mentions_reach_the_offline_rich_answer() {
    let ui = offline();
    ui.command("chat.focus_composer");
    ui.text("/tra");
    ui.key(Key::Enter, Modifiers::empty());
    let draft = ui.update(|chat, w| w.get(chat.draft));
    assert!(draft.contains("exoplanet transit"));
    assert!(!ui.exists("msg-2"));

    ui.edit(|session, expected_rev| EditorEvent::ProposeSelection { session, expected_rev, selection: (draft.len(), draft.len()) });
    ui.text(" @Mi");
    ui.key(Key::Enter, Modifiers::empty());
    assert!(ui.update(|chat, w| w.get(chat.draft)).trim_end().ends_with("[@Mira](user://mira)"));
    ui.key(Key::Enter, Modifiers::empty());

    for step in 1..=102 {
        ui.driver.borrow_mut().advance_to(Duration::from_millis(step * 30)).unwrap();
    }
    assert!(!ui.exists("stop"));
    assert!(ui.node("transit-chart").label.unwrap().contains("Midpoint 99 percent"));
    assert!(ui.node("transit-rust").text.unwrap().contains("depth.sqrt()"));
    let snapshot = ui.snapshot();
    assert!(snapshot.nodes.iter().any(|node| node.label.as_deref() == Some("A planet crossing the face of a star")));
    assert!(snapshot.nodes.iter().any(|node| node.text.as_deref() == Some("Analysis notes")));
    let driver = ui.driver.borrow();
    let tree = driver.app().runtime().window(ui.window).unwrap().tree();
    assert!(tree.inline_media_sources().any(|(_, _, slot)| matches!(slot.state, ImageState::Ready { .. })));
}

#[test]
fn composer_preserves_identity_and_blocks_invalid_generation_across_input_routes() {
    let (ui, model) = controlled(None);
    ui.command("chat.focus_composer");
    let composer = ui.node("composer").target;
    ui.text("   ");
    assert!(!ui.press("send"));
    ui.key(Key::Enter, Modifiers::empty());
    assert_eq!(model.count(Purpose::Reply), 0);
    ui.update(|chat, w| {
        chat.attach(w, &["note.txt".into()]);
        let generation = octara_chat::settings::Generation { max_completion_tokens: Some(0), ..Default::default() };
        w.set(chat.generation, generation);
    });
    assert!(!ui.press("send"));
    ui.key(Key::Enter, Modifiers::empty());
    assert_eq!(model.count(Purpose::Reply), 0);
    ui.update(|chat, w| w.set(chat.generation, Default::default()));
    assert!(ui.press("send"));
    assert_eq!(model.count(Purpose::Reply), 1);
    send(&ui, "Queued with Enter");
    assert_eq!(ui.update(|chat, w| w.len(chat.queue)), 1);
    assert_eq!(ui.node("composer").target, composer);
    assert!(ui.press("stop"));
    assert!(model.cancelled(model.index(Purpose::Reply, 0).unwrap()));
    ui.clean();
}

#[test]
fn duplicate_attachment_names_have_independent_remove_actions() {
    let (ui, _) = controlled(None);
    ui.driver.borrow_mut().stub_module_call::<octara_picker::PickFiles>(|request| {
        assert_eq!(request.title.as_deref(), Some("Attach"));
        assert!(request.multiple);
        Ok(["a/note.txt", "b/note.txt"]
            .map(|path| octara_picker::PickedFile {
                path: Some(path.into()),
                name: "note.txt".into(),
                media_type: "text/plain".into(),
                bytes: None,
            })
            .into())
    });
    assert!(ui.press("attach"));
    assert!(ui.press("remove-file-1"));
    assert!(!ui.exists("file-1"));
    assert!(ui.exists("file-2"));
    ui.clean();
}

#[test]
fn dropping_files_on_the_composer_uses_the_same_removable_attachments_as_the_picker() {
    let (ui, _) = controlled(None);
    ui.command("chat.focus_composer");
    ui.text("Keep typing");

    let snapshot = ui.snapshot();
    let composer = snapshot.nodes.iter().find(|node| node.test_id.as_deref() == Some("composer")).unwrap();
    let area = ui.node("composer-area").target;
    let editor = snapshot.editor.unwrap().session;
    let rect = composer.geometry.unwrap().rect;
    let mut at = point(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    let mut parent = composer.parent;
    while let Some(node) = parent.and_then(|target| snapshot.nodes.iter().find(|node| node.target == target)) {
        if let Some(geometry) = node.geometry {
            at.x += geometry.rect.x;
            at.y += geometry.rect.y;
        }
        parent = node.parent;
    }

    let path = std::path::PathBuf::from("/local/note.txt");
    let mut driver = ui.driver.borrow_mut();
    let window = driver.app().runtime().window(ui.window).unwrap();
    assert_eq!(window.hit_test(at).unwrap().hit, composer.target);
    assert_eq!(window.file_drop_target(at), Some(area));
    let reply =
        HostReply::FilesDropped { target: area, files: vec![DroppedFile::Path(path.clone()), DroppedFile::Name("note.txt".into())] };
    let receipt = driver.dispatch(ui.window, Input::Host(reply)).unwrap();
    assert!(receipt.delivered && receipt.handled);
    drop(driver);

    let files = ui
        .update(|chat, w| w.keys(chat.files).iter().map(|id| w.with_row(chat.files, id, |file| file.unwrap().clone())).collect::<Vec<_>>());
    assert_eq!(files, [Attachment { name: "note.txt".into(), path: Some(path) }, Attachment { name: "note.txt".into(), path: None }]);
    assert_eq!(ui.node("composer").target, composer.target);
    let editor_after = ui.snapshot().editor.unwrap();
    assert_eq!(editor_after.session, editor);
    assert_eq!(editor_after.value.as_deref(), Some("Keep typing"));
    assert!(ui.press("remove-file-1"));
    assert!(ui.exists("file-2"));
    assert_eq!(ui.update(|chat, w| w.len(chat.files)), 1);
    ui.clean();
}

#[test]
fn rename_and_delete_conversation_through_nested_panels() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path());
    let (ui, model) = controlled(Some(store.clone()));
    send(&ui, "My question");
    let answer = model.index(Purpose::Reply, 0).unwrap();
    model.emit(answer, Event::Text("Answer.".into()));
    model.emit(answer, Event::Done);
    ui.settle();
    assert!(ui.press("thread-menu-1"));
    assert!(ui.press("thread-rename-1"));
    assert_eq!(ui.snapshot().focus, Some(ui.node("rename-1").target));
    ui.text("New title");
    ui.key(Key::Enter, Modifiers::empty());
    assert!(ui.node("title").text.unwrap().contains("New title"));
    assert!(ui.press("thread-menu-1"));
    assert!(ui.press("thread-delete-1"));
    assert_eq!(ui.node("delete-thread-1").role, Some(Role::AlertDialog));
    assert!(ui.press("confirm-delete-1"));
    assert!(!ui.exists("thread-1"));
    store.flush();
    assert!(store.load().is_empty());
    ui.clean();
}

#[test]
fn conversation_context_menu_is_available_from_the_focused_row() {
    let (ui, _) = controlled(None);
    ui.update(|chat, w| {
        w.push_row(chat.threads, 1, Thread { id: 1, title: "A conversation".into(), updated: 1_800_000_000 });
    });
    ui.update(|_, w| w.focus("thread-1"));
    let thread = ui.node("thread-1").target;
    ui.key(Key::F(10), Modifiers::SHIFT);
    assert_eq!(ui.node("thread-menu-1-popover").role, Some(Role::Menu));
    assert_eq!(ui.snapshot().focus, Some(ui.node("thread-rename-1").target));
    ui.key(Key::Down, Modifiers::empty());
    assert_eq!(ui.snapshot().focus, Some(ui.node("thread-delete-1").target));
    ui.key(Key::Escape, Modifiers::empty());
    assert!(!ui.exists("thread-menu-1-popover"));
    assert_eq!(ui.snapshot().focus, Some(thread));

    ui.act(thread, AgentAction::Accessibility { action: A11yAction::ShowContextMenu });
    ui.key(Key::Enter, Modifiers::empty());
    assert_eq!(ui.snapshot().focus, Some(ui.node("rename-1").target));
    assert!(!ui.exists("thread-menu-1-popover"));
    ui.clean();
}

#[test]
fn clipboard_feedback_follows_the_host_reply_and_expires() {
    let (ui, _) = controlled(None);
    send(&ui, "Copy this message");
    let toast_text = |expected: &str| ui.snapshot().nodes.iter().any(|node| node.text.as_deref() == Some(expected));
    for (outcome, expected) in [
        (ServiceOutcome::ClipboardWritten, "Copied to clipboard"),
        (ServiceOutcome::Error(ServiceError::Denied), "Couldn’t copy to clipboard"),
    ] {
        assert!(ui.press("copy-2"));
        let service = ui
            .driver
            .borrow_mut()
            .take_host_commands()
            .into_iter()
            .find_map(|command| match command {
                HostCommand::Service { id, request: ServiceRequest::WriteClipboardText { text }, .. } => {
                    assert_eq!(text, "Copy this message");
                    Some(id)
                }
                _ => None,
            })
            .unwrap();
        assert!(!toast_text(expected));
        ui.host(HostReply::ServiceReply { id: service, outcome });
        assert!(toast_text(expected));
        let expires = ui.now() + Duration::from_secs(6);
        ui.driver.borrow_mut().advance_to(expires).unwrap();
        assert!(!toast_text(expected));
    }
    ui.clean();
}

#[test]
fn settings_validate_and_save_only_on_confirmation() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path());
    let (ui, _) = controlled(Some(store.clone()));
    assert!(ui.press("settings-open"));
    ui.update(|_, w| w.focus("settings-temperature"));
    ui.text("3");
    assert!(!ui.press("settings-save"));
    assert!(ui.node("settings-error").text.unwrap().contains("0"));
    assert!(ui.press("settings-cancel"));
    assert!(store.load_preferences().is_none());
    assert!(ui.press("settings-open"));
    ui.update(|_, w| w.focus("settings-temperature"));
    ui.text("0.7");
    assert!(ui.press("settings-save"));
    assert!(!ui.exists("settings"));
    assert_eq!(store.load_preferences().unwrap().generation.temperature, Some(0.7));
    ui.clean();
}

#[test]
fn declared_menu_shortcuts_work_with_window_and_editor_focus() {
    let (ui, model) = controlled(None);
    let menus = ui.driver.borrow_mut().take_host_commands().into_iter().find_map(|command| match command {
        HostCommand::Menus(menus) => Some(menus),
        _ => None,
    });
    let menus = menus.unwrap();
    assert!(menus.iter().flat_map(|menu| &menu.items).any(|item| matches!(
        item,
        MenuItem::Action { id, shortcut: Some(shortcut), .. } if id == "chat.focus_composer" && shortcut == "mod+l"
    )));

    let primary = if cfg!(target_os = "macos") { Modifiers::META } else { Modifiers::CTRL };
    ui.key(Key::Char('l'), primary);
    assert_eq!(ui.snapshot().focus, Some(ui.node("composer").target));
    ui.text("Shortcut test");
    ui.key(Key::Enter, Modifiers::empty());
    let answer = model.index(Purpose::Reply, 0).unwrap();
    ui.key(Key::Char('.'), primary);
    assert!(model.cancelled(answer));
    ui.key(Key::Char('n'), primary);
    assert_eq!(ui.node("title").text.as_deref(), Some("New chat"));
    ui.clean();
}

#[test]
fn navigation_reveals_virtual_endpoints_and_becomes_modal_on_small_windows() {
    let (ui, _) = controlled(None);
    ui.update(|chat, w| {
        for id in 1..=64 {
            w.push_row(chat.threads, id, Thread { id, title: format!("Conversation {id}"), updated: 1_800_000_000 - (64 - id) * 86_400 });
        }
        chat.open(w, 63);
    });
    ui.update(|_, w| w.focus("thread-63"));
    assert!(!ui.exists("thread-1"));
    ui.key(Key::End, Modifiers::empty());
    assert_eq!(ui.snapshot().focus, Some(ui.node("thread-1").target));
    ui.key(Key::Home, Modifiers::empty());
    assert_eq!(ui.snapshot().focus, Some(ui.node("thread-64").target));
    ui.key(Key::Home, Modifiers::empty());
    ui.key(Key::Down, Modifiers::empty());
    assert_eq!(ui.snapshot().focus, Some(ui.node("thread-63").target));
    ui.key(Key::Up, Modifiers::empty());
    assert_eq!(ui.snapshot().focus, Some(ui.node("thread-64").target));
    assert_eq!(ui.node("title").text.as_deref(), Some("Conversation 63"));
    ui.resize(390.0, 844.0);
    assert!(!ui.exists("sidebar"));
    let composer = ui.node("composer").target;
    ui.command("chat.focus_composer");
    ui.text("A draft from compact mode");
    assert!(ui.press("conversations"));
    assert_eq!(ui.snapshot().modal, Some(ui.node("chat-listing").target));
    ui.key(Key::Escape, Modifiers::empty());
    assert!(!ui.exists("chat-listing"));
    assert!(ui.press("conversations"));
    for phase in [BackPhase::Started, BackPhase::Progress(0.5), BackPhase::Completed] {
        let receipt =
            ui.driver.borrow_mut().dispatch(ui.window, Input::Interaction(InteractionEvent::BackGesture { id: 1, phase })).unwrap();
        assert!(receipt.handled, "{phase:?}");
    }
    assert!(!ui.exists("chat-listing"));
    assert_eq!(ui.node("composer").target, composer);
    assert!(ui.press("conversations"));
    assert!(ui.press("thread-64"));
    assert!(!ui.exists("chat-listing"));
    assert_eq!(ui.snapshot().focus, Some(composer));
    assert_eq!(ui.snapshot().editor.unwrap().value.as_deref(), Some("A draft from compact mode"));
    ui.clean();
}
