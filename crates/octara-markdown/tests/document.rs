use octara::{HostReply, Input, NodeId, Tree, model::motion::TextRevealTrack, prelude::*};
use octara_inspect::HeadlessDriver;
use octara_markdown::{MarkdownConfig, MarkdownRenderer, SmartCopy, StreamingEffects, StreamingOptions, markdown_stream, markdown_with};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn tree(driver: &HeadlessDriver, id: WindowId) -> &Tree {
    driver.app().runtime().window(id).unwrap().tree()
}

fn paragraph(tree: &Tree, document: &str) -> NodeId {
    let root = tree.test_ids(document)[0];
    tree.subtree(root).find(|&node| tree.semantics(node).is_some_and(|sem| sem.role == Some(Role::Paragraph))).unwrap()
}

fn set_reduced_motion(driver: &mut HeadlessDriver, id: WindowId, reduced_motion: bool) {
    let preferences = SystemPreferences { reduced_motion, ..Default::default() };
    driver.dispatch(id, Input::Host(HostReply::SystemPreferencesChanged(preferences))).unwrap();
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
                StreamingOptions { effects: Some(StreamingEffects::Fade { duration_secs: 0.15 }), ..StreamingOptions::default() },
            )
            .test_id("document")
        })
        .unwrap();
    driver.app_mut().update_window(id, |w| w.append_text(input, "Some incom")).unwrap();
    driver.advance_to(Duration::from_millis(80)).unwrap();
    let node = paragraph(tree(&driver, id), "document");
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
    assert_eq!(paragraph(tree(&driver, id), "document"), node);
    assert_eq!(tree(&driver, id).text(node).unwrap().text.as_ref(), "Some incomplete **literal");
    assert!(tree(&driver, id).text_reveal(node).is_none_or(|reveal| reveal.finished));
}

struct EffectRenderer(ReadSignal<StreamingEffects>);

impl MarkdownRenderer for EffectRenderer {
    fn streaming_effects(&self, read: &mut ReadCx<'_>) -> StreamingEffects {
        read.get(self.0)
    }
}

#[test]
fn scoped_streaming_effects_follow_live_preferences_without_replaying_visible_text() {
    let mut driver = HeadlessDriver::default();
    let (input, finished, effect) = driver
        .app_mut()
        .build(|b| (b.text_buffer("café"), b.signal(false).read_only(), b.signal(StreamingEffects::Fade { duration_secs: 0.25 })))
        .unwrap();
    let id = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |b| {
            b.provide::<Arc<dyn MarkdownRenderer>>(Arc::new(EffectRenderer(effect.read_only())));
            let default = StreamingOptions { pace: false, ..Default::default() };
            let no_effect = StreamingOptions { effects: Some(StreamingEffects::None), ..default };
            col()
                .child(markdown_stream(b, input, finished, default).test_id("scoped"))
                .child(markdown_stream(b, input, finished, no_effect).test_id("no-effect"))
        })
        .unwrap();
    let node = paragraph(tree(&driver, id), "scoped");
    let no_effect = paragraph(tree(&driver, id), "no-effect");
    assert_eq!(tree(&driver, id).text_reveal(node).unwrap().kind, TextRevealKind::Fade { duration_secs: 0.25 });
    assert!(tree(&driver, id).text_reveal(no_effect).is_none());

    set_reduced_motion(&mut driver, id, true);
    assert!(tree(&driver, id).text_reveal(node).is_none());
    driver
        .app_mut()
        .update_window(id, |w| {
            w.append_text(input, " noir");
            w.set(effect, StreamingEffects::Gradient { trail_bytes: 4, tint: None, glide_bytes_per_sec: 16.0 });
        })
        .unwrap();
    assert!(tree(&driver, id).text_reveal(node).is_none());

    set_reduced_motion(&mut driver, id, false);
    let reveal = *tree(&driver, id).text_reveal(node).unwrap();
    let prefix = "café noir".len();
    assert_eq!(reveal.visible_bytes, prefix);
    assert_eq!(reveal.kind, TextRevealKind::Gradient { trail_bytes: 4, tint: None, glide_bytes_per_sec: 16.0 });
    let track = TextRevealTrack::update(None, reveal, 0.0);
    assert!(track.rest_at().is_none());

    driver.app_mut().update_window(id, |w| w.append_text(input, " again")).unwrap();
    let reveal = *tree(&driver, id).text_reveal(node).unwrap();
    assert_eq!(reveal.revealed_bytes, "café noir again".len());
    let track = TextRevealTrack::update(Some(track), reveal, 0.0);
    assert_eq!(track.opacity(prefix, 0.0), 1.0);
    assert_eq!(track.opacity(prefix + 1, 0.0), 0.0);
    assert!(tree(&driver, id).text_reveal(no_effect).is_none());
}

#[test]
fn markdown_config_can_opt_in_to_reveal_under_reduced_motion() {
    let mut driver = HeadlessDriver::default();
    let (input, seconds) = driver.app_mut().build(|b| (b.text_buffer("café"), b.signal(0.25))).unwrap();
    let id = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |b| {
            let renderer = MarkdownConfig::document()
                .streaming_effects_with(move |r| StreamingEffects::Fade { duration_secs: r.get(seconds) })
                .allow_reduced_motion_effects(true);
            markdown_with(b, input, renderer).test_id("document")
        })
        .unwrap();
    let node = paragraph(tree(&driver, id), "document");
    set_reduced_motion(&mut driver, id, true);
    let reveal = tree(&driver, id).text_reveal(node).unwrap();
    assert!(reveal.allow_reduced_motion);
    assert_eq!(reveal.kind, TextRevealKind::Fade { duration_secs: 0.25 });

    driver
        .app_mut()
        .update_window(id, |w| {
            w.set(seconds, 0.5);
            w.append_text(input, " noir");
        })
        .unwrap();
    let reveal = tree(&driver, id).text_reveal(node).unwrap();
    assert_eq!(reveal.revealed_bytes, "café noir".len());
    assert_eq!(reveal.kind, TextRevealKind::Fade { duration_secs: 0.5 });
    assert_eq!(reveal.visible_bytes, "café".len());
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
