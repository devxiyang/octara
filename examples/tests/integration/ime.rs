//! Input method composition and the caret, headless.

use octara::app::{InputEvent, TargetPlatform};
use octara::inspect::{self, Delivery, LayoutState};
use octara::platform::HeadlessWindow;
use octara::prelude::*;
use octara::serde_json::json;
use octara_examples::{apps, rename};
use octara_primitives::text::TextEdit;

#[path = "../../src/bin/ime/choices.rs"]
mod choices;

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

#[test]
fn chinese_search_in_a_dialog_commits_the_revealed_business_key_after_composition() {
    let mut window = Window::headless(choices::view);
    window.app().set_target_platform(TargetPlatform::Ios);
    let safe_area = Rect::new(0.0, 50.0, 390.0, 760.0);
    let resize = |window: &mut Window, height| {
        assert!(window.event(InputEvent::Resize { width: 390.0, height, scale: 3.0, safe_area: Some(safe_area) }));
    };
    resize(&mut window, 844.0);
    window.tap("ime-choice-open").unwrap();
    window.tap("ime-choice").unwrap();
    resize(&mut window, 515.0);
    let search = "ime-choice-search";
    let last = "ime-choice-option-greeting-119";
    let list = "ime-choice-list";
    let count = "ime-choice-result";
    let search_target = window.inspect().find(search).unwrap().target.clone();
    let assert_popup = |snapshot: &inspect::Snapshot| {
        let [_, top, _, height] = snapshot.find("ime-choice-popover").unwrap().bounds;
        assert!(top >= safe_area.y + Placement::BELOW.margin);
        assert!(top + height <= snapshot.viewport[1].min(safe_area.bottom()) - Placement::BELOW.margin + 0.1);
        for id in [search, list] {
            let [_, y, _, child_height] = snapshot.find(id).unwrap().bounds;
            assert!(y >= top && y + child_height <= top + height + 0.1, "{id} is clipped by the popup");
        }
    };
    assert_popup(&window.inspect());
    let editor = |window: &Window| window.platform_window().as_any().downcast_ref::<HeadlessWindow>().unwrap().text_input.clone().unwrap();

    let initial = editor(&window);
    let mark = TextEdit::Mark { text: "nihao".into(), selection: Some(5..5), replacement: None };
    assert!(window.apply_text_edit(initial.id, &initial.document.unwrap(), &mark));
    window.press_key("enter").unwrap();
    assert_eq!(window.text_field().unwrap().composition, "nihao");
    assert_eq!(window.kernel().text_of(count).as_deref(), Some("提交次数 0 · 未选择"));
    let draft = editor(&window);
    let commit = TextEdit::Insert { text: "你好".into(), replacement: None };
    assert!(window.apply_text_edit(draft.id, &draft.document.unwrap(), &commit));
    let filtered = window.inspect();
    assert_eq!(filtered.text_input.as_ref().unwrap().text, "你好");
    assert_eq!(filtered.find(search).unwrap().active_descendant.as_deref(), Some("ime-choice-option-greeting-000"));
    assert!(filtered.find(last).is_none());

    for request in [r#"{"op":"key","chord":"up"}"#, r#"{"op":"invoke","command":"ime-choice.reorder"}"#] {
        let reply = inspect::handle_line(&mut window, request);
        assert_eq!(reply.delivery, Some(Delivery::Delivered), "{reply:?}");
        assert_eq!(reply.completion.unwrap().layout, LayoutState::Settled);
        let snapshot = window.inspect();
        assert_popup(&snapshot);
        let field = snapshot.find(search).unwrap();
        assert_eq!(snapshot.focus.as_deref(), Some(search));
        assert_eq!(field.target, search_target);
        assert_eq!(field.active_descendant.as_deref(), Some(last));
        let [_, y, _, height] = snapshot.find(last).unwrap().bounds;
        let [_, top, _, viewport_height] = snapshot.find(list).unwrap().bounds;
        assert!(y >= top - 0.1 && y + height <= top + viewport_height + 0.1);
        assert_eq!(window.kernel().text_of(count).as_deref(), Some("提交次数 0 · 未选择"));
    }

    // Removing the keyboard gives the open list its normal height again.
    resize(&mut window, 844.0);
    let expanded = window.inspect();
    assert_popup(&expanded);
    assert_eq!(expanded.find(list).unwrap().bounds[3], 240.0);
    assert_eq!(expanded.find(search).unwrap().target, search_target);

    window.press_key("enter").unwrap();
    let selected = window.inspect();
    assert_eq!(window.kernel().text_of(count).as_deref(), Some("提交次数 1 · greeting-119"));
    assert_eq!(selected.focus.as_deref(), Some("ime-choice"));
    assert!(selected.find(search).is_none());
    assert!(selected.find("ime-choice-dialog").is_some());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}
