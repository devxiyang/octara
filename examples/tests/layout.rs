//! Every example app lays out cleanly on a phone and on a desktop.

use octara::prelude::*;
use octara_examples::apps;

#[test]
fn example_apps_have_no_layout_issues_at_phone_and_desktop_sizes() {
    type Case = (&'static str, fn() -> View);
    let cases: Vec<Case> = vec![
        ("counter", apps::counter),
        ("todo", || apps::todo(50)),
        ("chat", || apps::chat(50)),
        ("settings", apps::settings),
        ("contacts", || apps::contacts(60)),
        ("chat_virtual", || apps::chat_virtual(10_000)),
        ("contacts_virtual", || apps::contacts_virtual(10_000)),
    ];
    for (name, view) in cases {
        let mut app = App::new(view);
        let r = app.at_sizes(&[(375.0, 667.0), (1280.0, 800.0)], |app| {
            let issues: Vec<_> =
                app.layout_issues().into_iter().filter(|i| i.code.severity != octara::diagnostics::Severity::Info).collect();
            if issues.is_empty() { Ok(()) } else { Err(format!("{issues:#?}")) }
        });
        assert_eq!(r, Ok(()), "{name}");
    }
}
