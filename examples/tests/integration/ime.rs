//! Input method composition and the caret, headless.

use octara::app::InputEvent;
use octara::inspect::{self, Delivery};
use octara::platform::HeadlessWindow;
use octara::prelude::*;
use octara::serde_json::json;
use octara_examples::{apps, rename};
use octara_primitives::text::TextEdit;

#[test]
fn composition_is_shown_underlined_and_committed_text_replaces_it() {
    let mut app = Window::headless(|| apps::todo(3));
    let (x, y, w, h) = {
        let k = app.kernel();
        k.rect(k.find("new-todo").unwrap())
    };
    app.click_at(x + w / 2.0, y + h / 2.0);
    let caret0 = app.caret().expect("a focused field has a caret");
    app.event(InputEvent::Composition(("ni hao").into()));
    let caret1 = app.caret().unwrap();
    assert!(caret1.0 > caret0.0, "the caret follows the composition");
    assert_eq!(app.kernel().value_of("new-todo").unwrap(), "", "composition is not the value yet");
    let id = app.kernel().find("new-todo").unwrap();
    let dl = app.display_list();
    let quads: Vec<_> = dl.quads().filter(|q| q.source == id && q.rect.h <= 1.0).collect();
    assert_eq!(quads.len(), 1, "one underline under the composition");
    app.type_text("你好");
    assert_eq!(app.kernel().value_of("new-todo").unwrap(), "你好");
    assert_eq!(app.kernel().preedit(id), "");
    // The caret is drawn while focused: a 1 px wide quad as tall as a line.
    let caret = app.display_list().quads().filter(|q| q.source == id && q.rect.w == 1.0).count();
    assert_eq!(caret, 1);
}

#[test]
fn rename_fixture_keeps_a_file_edit_across_refresh_and_refuses_retired_delivery() {
    let mut window = Window::headless(rename::view);
    let begin = |window: &mut Window| {
        window.tap("rename-select").unwrap();
        window.tap("rename-start").unwrap();
        window.set_selection(5, 1);
        window.event(InputEvent::Preedit { text: "nihao".into(), selection: Some((5, 5)) });
        window.inspect().find("rename-editor").unwrap().target.clone()
    };
    let target = begin(&mut window);
    let before = window.inspect();
    for id in ["rename-file-1", "rename-file-2"] {
        assert!(before.find(id).unwrap().states.iter().any(|state| state == "selected"));
    }
    let initial_caret = before.text_input.unwrap().caret;
    for command in ["rename.reorder", "rename.refresh", "rename.scroll-away", "rename.scroll-back"] {
        window.invoke(command).unwrap();
        let snapshot = window.inspect();
        assert_eq!(snapshot.focus.as_deref(), Some("rename-editor"));
        assert_eq!(snapshot.find("rename-editor").unwrap().target, target);
        let edit = snapshot.text_input.unwrap();
        assert_eq!((edit.selection, edit.selection_utf16, edit.reversed), ([1, 5], [1, 3], true));
        assert_eq!((edit.text.as_str(), edit.display_text.as_str(), edit.composition.as_str()), ("A😀Z", "AnihaoZ", "nihao"));
        if command == "rename.reorder" {
            assert_ne!(edit.caret[1], initial_caret[1]);
        }
    }
    window.press_key("enter").unwrap();
    assert_eq!(window.kernel().text_of("rename-counts").as_deref(), Some("Edits: 0 · Attempts: 0 · Saves: 0"));
    window.event(InputEvent::Text("你好".into()));
    assert_eq!(window.inspect().text_input.unwrap().text, "A你好Z");
    assert_eq!(window.kernel().text_of("rename-counts").as_deref(), Some("Edits: 1 · Attempts: 0 · Saves: 0"));
    window.press_key("enter").unwrap();
    assert_eq!(window.kernel().text_of("rename-records").as_deref(), Some("2: A你好Z | 1: A😀Z"));
    assert_eq!(window.kernel().text_of("rename-counts").as_deref(), Some("Edits: 1 · Attempts: 1 · Saves: 1"));

    window.invoke("rename.reset").unwrap();
    let conflict_target = begin(&mut window);
    window.invoke("rename.external").unwrap();
    assert_eq!(window.inspect().text_input.unwrap().composition, "nihao");
    window.event(InputEvent::Text("你好".into()));
    window.press_key("enter").unwrap();
    let conflict = window.inspect();
    assert_eq!(conflict.text_input.as_ref().unwrap().text, "A你好Z");
    assert_eq!(conflict.find("rename-editor").unwrap().target, conflict_target);
    assert_eq!(window.kernel().text_of("rename-status").as_deref(), Some("Conflict in Directory B: External B.txt"));
    assert_eq!(window.kernel().text_of("rename-counts").as_deref(), Some("Edits: 1 · Attempts: 1 · Saves: 0"));
    window.press_key("escape").unwrap();
    assert!(window.inspect().text_input.is_none());
    assert_eq!(window.kernel().text_of("rename-records").as_deref(), Some("1: A😀Z | 2: External B.txt"));

    window.invoke("rename.reset").unwrap();
    let retired = begin(&mut window);
    let native = window.platform_window().as_any().downcast_ref::<HeadlessWindow>().unwrap().text_input.clone().unwrap();
    window.invoke("rename.replace").unwrap();
    let replacement = window.inspect();
    assert_ne!(replacement.find("rename-editor").unwrap().target, retired);
    assert!(!window.apply_text_edit(native.id, &native.document.unwrap(), &TextEdit::Insert { text: "迟到".into(), replacement: None }));
    let reply = inspect::handle_line(&mut window, &json!({"op":"focus", "target":retired}).to_string());
    assert_eq!(reply.code.as_deref(), Some("STALE_TARGET"));
    assert_eq!(reply.delivery, Some(Delivery::Refused));
    let field = window.inspect().text_input.unwrap();
    assert_eq!((field.text.as_str(), field.composition.as_str()), ("A😀Z", ""));
    assert_eq!(window.kernel().text_of("rename-counts").as_deref(), Some("Edits: 0 · Attempts: 0 · Saves: 0"));
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}
