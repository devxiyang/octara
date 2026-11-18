//! The composed dialog, tab panels and suggestions used by the real-window runner.

use octara::app::{Event, InputEvent};
use octara::prelude::*;
use std::time::Duration;

fn scene() -> View {
    octara_examples::showcase::page("interaction").unwrap()
}
fn settle(window: &Window) {
    window.app().run_until_idle(Duration::from_secs(3)).unwrap();
}
fn clean(window: &Window) {
    settle(window);
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
    assert!(window.kernel().accessibility_issues().is_empty(), "{:?}", window.kernel().accessibility_issues());
}

#[test]
fn dialog_suggestions_keep_the_editor_and_choose_without_submitting() {
    let mut window = Window::headless(scene);
    window.tap("interaction-open").unwrap();
    settle(&window);
    assert_eq!(window.inspect().focus.as_deref(), Some("suggest-editor"));
    window.type_text("/");
    settle(&window);
    let target = window.inspect().find("suggest-editor").unwrap().target.clone();
    window.press_key("down").unwrap();
    assert_eq!(window.inspect().find("suggest-editor").unwrap().active_descendant.as_deref(), Some("suggest-editor-option-summary"));
    window.press_key("enter").unwrap();
    assert_eq!(window.kernel().value_of("suggest-editor").as_deref(), Some("请概括当前内容。"));
    assert_eq!(window.kernel().text_of("interaction-submissions").as_deref(), Some("提交次数：0"));
    assert!(!window.kernel().exists("suggest-editor-list"));
    assert_eq!(window.inspect().focus.as_deref(), Some("suggest-editor"));
    assert_eq!(window.inspect().find("suggest-editor").unwrap().target, target);
    window.press_key("enter").unwrap();
    assert_eq!(window.kernel().text_of("interaction-submissions").as_deref(), Some("提交次数：1"));
    window.press_key("escape").unwrap();
    settle(&window);
    assert!(!window.kernel().exists("editor-dialog"));
    assert_eq!(window.inspect().focus.as_deref(), Some("interaction-open"));
    window.tap("interaction-open").unwrap();
    settle(&window);
    assert_eq!(window.kernel().value_of("suggest-editor").as_deref(), Some("请概括当前内容。"));
    assert_ne!(window.inspect().find("suggest-editor").unwrap().target, target);
    clean(&window);
}

#[test]
fn tab_panels_preserve_edit_history_and_ime_owns_suggestion_navigation() {
    let mut window = Window::headless(scene);
    window.tap("interaction-open").unwrap();
    settle(&window);
    window.type_text("/");
    let before = window.inspect().find("suggest-editor").unwrap().target.clone();
    window.set_selection(1, 1);
    window.event(InputEvent::Preedit { text: "nihao".into(), selection: Some((0, 5)) });
    let editor = window.text_field().unwrap();
    let active = window.inspect().find("suggest-editor").unwrap().active_descendant.clone();
    for key in ["down", "up", "space", "tab", "enter", "escape"] {
        window.press_key(key).unwrap();
    }
    window.invoke("interaction.options").unwrap();
    assert_eq!(window.text_field().unwrap().composition, editor.composition);
    assert_eq!(window.inspect().focus.as_deref(), Some("suggest-editor"));
    assert_eq!(window.inspect().find("suggest-editor").unwrap().active_descendant, active);
    window.event(InputEvent::Preedit { text: String::new(), selection: None });
    window.type_text("你好");
    window.tap("editor-tabs-tab-draft").unwrap();
    window.press_key("right").unwrap();
    assert!(window.inspect().find("editor-tabs-tab-draft").unwrap().states.iter().any(|s| s == "selected"));
    window.press_key("enter").unwrap();
    settle(&window);
    assert!(window.kernel().text_of("editor-preview-text").unwrap().contains("你好"));
    window.press_key("left").unwrap();
    window.press_key("enter").unwrap();
    settle(&window);
    assert_eq!(window.inspect().find("suggest-editor").unwrap().target, before);
    window.dispatch(Event::Focus("suggest-editor".into())).unwrap();
    window.press_key("mod+z").unwrap();
    assert_eq!(window.kernel().value_of("suggest-editor").as_deref(), Some("/"));
    clean(&window);
}

#[test]
fn composed_dialog_and_suggestions_fit_narrow_and_desktop_windows() {
    for theme in [Theme::light(), Theme::dark()] {
        for width in [320.0, 390.0, 1000.0] {
            let mut window = Window::headless(scene);
            window.resize(width, 640.0);
            window.app().set_theme(theme.clone());
            settle(&window);
            window.tap("interaction-open").unwrap();
            settle(&window);
            window.type_text("/");
            clean(&window);
            window.tap("suggest-editor-option-review").unwrap();
            assert_eq!(window.inspect().focus.as_deref(), Some("suggest-editor"));
            assert_eq!(window.kernel().text_of("interaction-chosen").as_deref(), Some("已选择：review"));
            clean(&window);
        }
    }
}
