use octara::{HostReply, Input, Tree, prelude::*};
use octara_inspect::HeadlessDriver;
use octara_markdown::{MarkdownConfig, SmartCopy, StreamingEffects, StreamingOptions, markdown_stream, markdown_with};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn tree(driver: &HeadlessDriver, id: WindowId) -> &Tree {
    driver.app().runtime().window(id).unwrap().tree()
}

#[test]
fn paced_document_holds_a_transient_word_and_finalizes_after_the_drain() {
    let mut driver = HeadlessDriver::default();
    let (input, finished) = driver.app_mut().build(|b| (b.text_buffer(""), b.signal(false))).unwrap();
    let id = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |b| {
            markdown_stream(
                b,
                input,
                finished.read_only(),
                StreamingOptions { effects: StreamingEffects::Fade { duration_secs: 0.15 }, ..StreamingOptions::default() },
            )
            .test_id("document")
        })
        .unwrap();
    let paragraph = |driver: &HeadlessDriver| {
        let tree = tree(driver, id);
        let root = tree.test_ids("document")[0];
        tree.subtree(root).find(|&node| tree.semantics(node).is_some_and(|sem| sem.role == Some(Role::Paragraph))).unwrap()
    };
    driver.app_mut().update_window(id, |w| w.append_text(input, "Some incom")).unwrap();
    driver.advance_to(Duration::from_millis(80)).unwrap();
    let node = paragraph(&driver);
    assert_eq!(tree(&driver, id).text(node).unwrap().text.as_ref(), "Some");
    assert!(!tree(&driver, id).text_reveal(node).unwrap().finished);

    driver
        .app_mut()
        .update_window(id, |w| {
            w.append_text(input, "plete **literal");
            w.set(finished, true);
        })
        .unwrap();
    driver.advance_to(Duration::from_millis(280)).unwrap();
    assert_eq!(paragraph(&driver), node);
    assert_eq!(tree(&driver, id).text(node).unwrap().text.as_ref(), "Some incomplete **literal");
    assert!(tree(&driver, id).text_reveal(node).is_none_or(|reveal| reveal.finished));
}

#[test]
fn context_menu_offers_smart_copy_formats_for_the_selection() {
    const TABLE: &str = "| A | B |\n| --- | --- |\n| α | β |";
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let mut driver = HeadlessDriver::default();
    let id = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |b| {
            let config = MarkdownConfig::document().on_context_menu(move |_, event| {
                capture.lock().unwrap().push(event.items.clone());
                true
            });
            markdown_with(b, format!("{TABLE}\n\nA ![sample image](https://example.org/picture.png)"), config).test_id("document")
        })
        .unwrap();
    let document = tree(&driver, id).test_ids("document")[0];
    let find_text = |driver: &HeadlessDriver, expected: &str| {
        let tree = tree(driver, id);
        tree.subtree(document).find(|&node| tree.text(node).is_some_and(|text| text.text.as_ref() == expected)).unwrap()
    };
    let a = find_text(&driver, "A");
    let beta = find_text(&driver, "β");
    let image = find_text(&driver, "A sample image");
    let on_document = |driver: &mut HeadlessDriver, action| {
        let target = tree(driver, id).guard(document);
        assert!(driver.dispatch(id, Input::Host(HostReply::AccessibilityAction { target, action })).unwrap().handled);
    };
    let select = |driver: &mut HeadlessDriver, anchor, focus, end| {
        on_document(
            driver,
            A11yAction::SetTextSelection {
                anchor: TextPosition { node: anchor, offset: 0 },
                focus: TextPosition { node: focus, offset: end },
            },
        );
        // Target a selected text node so the runtime finds its selection region.
        let target = tree(driver, id).guard(focus);
        assert!(
            driver
                .dispatch(id, Input::Host(HostReply::AccessibilityAction { target, action: A11yAction::ShowContextMenu }))
                .unwrap()
                .handled
        );
    };
    select(&mut driver, a, beta, "β".len());
    select(&mut driver, image, image, "A sample image".len());
    let events = events.lock().unwrap();
    let has = |items: &[ContextMenuItem], format: SmartCopy, expected: &str| {
        items
            .iter()
            .any(|item| matches!(item, ContextMenuItem::Copy { format: actual, text } if *actual == format.into() && text == expected))
    };
    assert!(has(&events[0], SmartCopy::Plain, "A\nB\nα\nβ"));
    assert!(has(&events[0], SmartCopy::Markdown, TABLE));
    assert!(has(&events[0], SmartCopy::TableMarkdown, TABLE));
    assert!(has(&events[1], SmartCopy::ImageUrl, "https://example.org/picture.png"));
}
