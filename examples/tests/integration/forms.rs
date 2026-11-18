//! The actual Showcase form, including validation changes while its editor is composing.

use octara::app::InputEvent;
use octara::prelude::*;
use std::time::Duration;

fn scene() -> View {
    octara_examples::showcase::page("forms").unwrap()
}

fn settle(window: &Window) {
    window.app().run_until_idle(Duration::from_secs(3)).unwrap();
}

fn clean(window: &Window) {
    settle(window);
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.kernel().accessibility_issues().is_empty(), "{:?}", window.kernel().accessibility_issues());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

#[test]
fn invalid_fields_explain_why_save_is_disabled_and_enter_obeys_the_same_validation() {
    let mut window = Window::headless(scene);
    window.tap("profile-name").unwrap();
    window.press_key("mod+a").unwrap();
    window.type_text("admin");
    let snapshot = window.inspect();
    let input = snapshot.find("profile-name").unwrap();
    assert_eq!(input.required, Some(true));
    assert_eq!(input.invalid, Some(true));
    assert!(input.description.as_deref().unwrap().contains("此名称不可用"));
    let save = snapshot.find("forms-save").unwrap();
    assert!(save.states.iter().any(|s| s == "disabled"));
    assert!(save.description.as_deref().unwrap().contains("此名称不可用"));
    window.press_key("enter").unwrap();
    assert_eq!(window.kernel().text_of("forms-status").as_deref(), Some("尚未保存。"));
    window.press_key("mod+a").unwrap();
    window.type_text("  你好😀  ");
    window.press_key("enter").unwrap();
    assert_eq!(window.kernel().value_of("profile-name").as_deref(), Some("  你好😀  "), "saving must not rewrite the editor");
    assert_eq!(window.kernel().text_of("forms-status").as_deref(), Some("已保存：你好😀 · balanced"));
    assert_eq!(window.inspect().find("profile-name").unwrap().invalid, Some(false));
    assert!(!window.kernel().exists("profile-name-error"));
    clean(&window);
}

#[test]
fn form_metadata_and_radio_reordering_preserve_the_composing_editor() {
    let mut window = Window::headless(scene);
    window.tap("profile-name").unwrap();
    window.set_selection(0, 6);
    window.event(InputEvent::Preedit { text: "nihao".into(), selection: Some((0, 5)) });
    let input = window.text_field().unwrap();
    let snapshot = window.inspect();
    let target = snapshot.find("profile-name").unwrap().target.clone();
    let radio = snapshot.find("reply-style-option-balanced").unwrap().target.clone();
    window.invoke("forms.help").unwrap();
    window.invoke("forms.reorder").unwrap();
    let after = window.text_field().unwrap();
    assert_eq!(after.composition, input.composition);
    assert_eq!(after.composition_selection, input.composition_selection);
    assert_eq!(after.selection, input.selection);
    let snapshot = window.inspect();
    assert_eq!(snapshot.find("profile-name").unwrap().target, target);
    assert!(snapshot.find("profile-name").unwrap().description.as_deref().unwrap().contains("两端空格"));
    assert_eq!(snapshot.find("reply-style-option-balanced").unwrap().target, radio);
    window.press_key("enter").unwrap();
    assert_eq!(window.kernel().text_of("forms-status").as_deref(), Some("尚未保存。"), "IME Enter is not a form submission");
    window.event(InputEvent::Preedit { text: String::new(), selection: None });
    window.type_text("你好");
    window.press_key("tab").unwrap();
    assert_eq!(window.inspect().focus.as_deref(), Some("reply-style-option-balanced"));
    window.press_key("down").unwrap();
    assert_eq!(window.inspect().find("reply-style-option-fast").unwrap().checked, Some(true));
    window.press_key("down").unwrap();
    assert_eq!(window.inspect().focus.as_deref(), Some("reply-style-option-balanced"), "disabled Deep is skipped when wrapping");
    window.press_key("tab").unwrap();
    assert_eq!(window.inspect().focus.as_deref(), Some("forms-help"), "a group is one Tab stop");
    clean(&window);
}

#[test]
fn forms_fit_phone_and_desktop_in_both_appearances() {
    let report = octara::inspect::check(scene, &Default::default());
    assert!(report.clean, "{}", report.to_json());
    for (width, height) in [(320.0, 640.0), (390.0, 844.0), (1000.0, 720.0)] {
        for theme in [Theme::light(), Theme::dark()] {
            let mut window = Window::headless(scene);
            window.app().set_target_platform(if width < 500.0 {
                octara::app::TargetPlatform::Ios
            } else {
                octara::app::TargetPlatform::MacOs
            });
            window.resize(width, height);
            window.app().set_theme(theme);
            settle(&window);
            window.tap("profile-name").unwrap();
            window.press_key("mod+a").unwrap();
            window.type_text("admin");
            window.invoke("forms.help").unwrap();
            window.invoke("forms.reorder").unwrap();
            clean(&window);
        }
    }
}
