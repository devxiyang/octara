//! Numeric editing and command menus in the actual Showcase controls scene.

use octara::prelude::*;
use std::time::Duration;

fn scene() -> View {
    octara_examples::showcase::page("controls").unwrap()
}
fn settle(window: &Window) {
    window.app().run_until_idle(Duration::from_secs(3)).unwrap();
}
fn clean(window: &Window) {
    settle(window);
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

#[test]
fn numeric_drafts_commit_explicitly_and_semantic_slider_updates_the_same_model() {
    let mut window = Window::headless(scene);
    window.tap("controls-number").unwrap();
    window.press_key("mod+a").unwrap();
    window.type_text("-");
    window.press_key("enter").unwrap();
    assert_eq!(window.kernel().value_of("controls-number").as_deref(), Some("-"));
    let snapshot = window.inspect();
    assert_eq!(snapshot.find("controls-number").unwrap().invalid, Some(true));
    assert_eq!(snapshot.find("controls-slider").unwrap().numeric_value, Some(0.35));
    window.press_key("mod+a").unwrap();
    window.type_text("0.2600");
    assert_eq!(window.kernel().value_of("controls-number").as_deref(), Some("0.2600"));
    window.press_key("enter").unwrap();
    assert_eq!(window.kernel().value_of("controls-number").as_deref(), Some("0.25"));
    assert_eq!(window.inspect().find("controls-progress").unwrap().numeric_value, Some(25.0));
    let target = window.inspect().find("controls-slider").unwrap().target.clone();
    let reply = octara::inspect::handle_line(
        &mut window,
        &json!({"op":"set_value","id":"controls-slider","value":0.76,"target":target}).to_string(),
    );
    assert!(reply.ok, "{reply:?}");
    assert_eq!(window.kernel().value_of("controls-number").as_deref(), Some("0.75"));
    assert_eq!(window.inspect().find("controls-progress").unwrap().numeric_value, Some(75.0));
    clean(&window);
}

#[test]
fn command_menu_opens_submenus_and_disabled_controls_refuse_numeric_changes() {
    let mut window = Window::headless(scene);
    window.dispatch(Event::Focus("controls-menu".into())).unwrap();
    window.press_key("down").unwrap();
    settle(&window);
    assert_eq!(window.inspect().focus.as_deref(), Some("controls-menu-action-controls.reset"));
    window.press_key("end").unwrap();
    window.press_key("right").unwrap();
    settle(&window);
    window.press_key("end").unwrap();
    window.press_key("enter").unwrap();
    settle(&window);
    assert!(!window.kernel().exists("controls-menu-menu"));
    assert_eq!(window.inspect().focus.as_deref(), Some("controls-menu"));
    assert_eq!(window.inspect().find("controls-slider").unwrap().numeric_value, Some(1.0));
    window.press_key("down").unwrap();
    settle(&window);
    window.press_key("down").unwrap();
    window.press_key("enter").unwrap();
    settle(&window);
    assert!(window.inspect().find("controls-number").unwrap().states.iter().any(|state| state == "disabled"));
    assert!(window.dispatch(Event::NumericInput("controls-slider".into(), 0.0)).is_err());
    assert_eq!(window.inspect().find("controls-slider").unwrap().numeric_value, Some(1.0));
    window.invoke("controls.toggle_disabled").unwrap();
    window.invoke("controls.reset").unwrap();
    clean(&window);
}

#[test]
fn progress_unknown_state_does_not_pretend_to_be_zero_and_layouts_fit() {
    for theme in [Theme::light(), Theme::dark()] {
        for width in [320.0, 390.0, 1000.0] {
            let mut window = Window::headless(scene);
            window.resize(width, 640.0);
            window.app().set_theme(theme.clone());
            settle(&window);
            window.tap("controls-progress-mode").unwrap();
            let snapshot = window.inspect();
            let progress = snapshot.find("controls-progress").unwrap();
            assert_eq!(progress.numeric_value, None);
            assert_eq!(progress.max_numeric_value, Some(100.0));
            assert!(!progress.actions.iter().any(|action| action == "set_value"));
            window.tap("controls-progress-mode").unwrap();
            clean(&window);
            window.tap("controls-menu").unwrap();
            clean(&window);
        }
    }
}
