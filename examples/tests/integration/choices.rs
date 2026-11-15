//! Exercise the public showcase with the same commands available in its native window.

use octara::prelude::*;
use std::time::Duration;

fn scene() -> View {
    octara_examples::showcase::page("choices").unwrap()
}

fn settle(window: &Window) {
    window.app().run_until_idle(Duration::from_secs(3)).unwrap();
}

fn focused(window: &Window, id: &str) -> bool {
    window.kernel().state_of(id).is_some_and(|s| s.contains(State::FOCUSED))
}

#[test]
fn the_dialog_picker_survives_reload_and_returns_through_both_openers() {
    let mut window = Window::headless(scene);
    window.tap("open-choices-dialog").unwrap();
    settle(&window);
    window.tap("dialog-city").unwrap();
    settle(&window);
    assert!(focused(&window, "dialog-city-search"));
    window.type_text("深");
    settle(&window);
    let before = window.inspect().find("dialog-city-search").unwrap().target.clone();
    let before_popup = window.inspect().find("dialog-city-popover").unwrap().bounds;
    window.app().hot_reload();
    settle(&window);
    assert_ne!(window.inspect().find("dialog-city-search").unwrap().target, before);
    assert_eq!(window.inspect().find("dialog-city-popover").unwrap().bounds, before_popup);
    assert_eq!(window.kernel().value_of("dialog-city-search").as_deref(), Some("深"));
    window.press_key("enter").unwrap();
    settle(&window);
    assert_eq!(window.kernel().text_of("chosen-city").as_deref(), Some("已选择：深圳"));
    assert!(focused(&window, "dialog-city"));
    window.press_key("escape").unwrap();
    settle(&window);
    assert!(focused(&window, "open-choices-dialog"));
    assert!(!window.kernel().exists("choices-dialog"));
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

#[test]
fn live_option_updates_and_unmount_leave_no_popup_behind() {
    let mut window = Window::headless(scene);
    window.tap("city").unwrap();
    settle(&window);
    window.type_text("杭");
    window.press_key("mod+shift+u").unwrap();
    settle(&window);
    assert_eq!(window.kernel().value_of("city-search").as_deref(), Some("杭"));
    assert_eq!(window.kernel().text_of("chosen-city").as_deref(), Some("已选择：杭州 · 已更新"));
    assert!(focused(&window, "city-search"));
    window.press_key("mod+shift+x").unwrap();
    settle(&window);
    assert!(!window.kernel().exists("city"));
    assert!(!window.kernel().exists("city-popover"));
    window.tap("toggle-city").unwrap();
    settle(&window);
    assert!(window.kernel().exists("city"));
    assert!(!window.kernel().exists("city-popover"));
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

#[test]
fn choices_fit_phone_and_desktop_with_both_appearances() {
    let report = octara::inspect::check(scene, &Default::default());
    assert!(report.clean, "{}", report.to_json());
    for size in [(390.0, 844.0), (1000.0, 720.0)] {
        for dark in [false, true] {
            let mut window = Window::headless(scene);
            window.app().set_target_platform(if size.0 < 500.0 {
                octara::app::TargetPlatform::Ios
            } else {
                octara::app::TargetPlatform::MacOs
            });
            window.resize(size.0, size.1);
            if dark {
                window.app().set_theme(Theme::dark());
            }
            settle(&window);
            window.tap("city").unwrap();
            settle(&window);
            window.type_text("成都");
            settle(&window);
            assert!(window.layout_issues().is_empty(), "{size:?}/{dark}: {:?}", window.layout_issues());
            assert!(window.kernel().accessibility_issues().is_empty(), "{size:?}/{dark}: {:?}", window.kernel().accessibility_issues());
            assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
        }
    }
}
