use octara::{HostCommand, HostReply, Input, prelude::*};
use octara_inspect::HeadlessDriver;
use octara_markdown::{LinkPillContent, LinkVariantRule, LinkVariantRules, MarkdownConfig, MarkdownTheme, TextRecognizer, markdown_with};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn metadata_updates_link_presentation_without_changing_its_destination() {
    const URL: &str = "https://example.test/issues/128";
    let mut driver = HeadlessDriver::default();
    let metadata = driver.app_mut().build(|build| build.signal(None::<LinkPillContent>)).unwrap();
    let background = ColorValue::from(rgb(0x243D52));
    let tint = ColorValue::from(rgb(0x49BAE8));
    let variants = LinkVariantRules::new([LinkVariantRule::new(
        "https://example.test/issues/",
        Span { background: Some(background), padding_x: Some(4.0), ..Span::default() },
    )])
    .unwrap();
    let theme_reads = Arc::new(AtomicUsize::new(0));
    let reads = theme_reads.clone();
    let window = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |build| {
            let config = MarkdownConfig::document()
                .theme_with(move |_| {
                    reads.fetch_add(1, Ordering::Relaxed);
                    MarkdownTheme::document()
                })
                .link_variant_rules(variants)
                .link_pill_content(move |read, url| if url == URL { read.get(metadata) } else { None });
            markdown_with(build, "Ticket [Ori**gin**al](https://example.test/issues/128)", config).test_id("markdown")
        })
        .unwrap();
    let tree = driver.app().runtime().window(window).unwrap().tree();
    let root = tree.test_ids("markdown")[0];
    let paragraph = tree.subtree(root).find(|&node| tree.text(node).is_some()).unwrap();
    let text = tree.text(paragraph).unwrap();
    assert_eq!(text.text.as_ref(), "Ticket Original");
    assert!(text.runs.iter().any(|(_, span)| span.background == Some(background)));
    let initial_theme_reads = theme_reads.load(Ordering::Relaxed);

    driver
        .app_mut()
        .update_window(window, |write| {
            write.set(
                metadata,
                Some(LinkPillContent { label: Some("Issue title".into()), icon: Some(octara_icons::HASH.into()), tint: Some(tint) }),
            );
        })
        .unwrap();
    let tree = driver.app().runtime().window(window).unwrap().tree();
    let text = tree.text(paragraph).unwrap();
    assert_eq!(text.text.as_ref(), format!("Ticket {URL}"));
    let [link] = text.links.as_slice() else { panic!("expected one rich text link") };
    let link = tree.link(*link).unwrap();
    assert_eq!(link.href.as_ref(), URL);
    assert_eq!(&text.text[link.range.clone()], URL);
    let [attachment] = text.attachments.as_slice() else { panic!("expected one inline pill") };
    let InlineContent::Pill(pill) = &attachment.content else { panic!("expected an inline pill") };
    assert_eq!(attachment.range, link.range);
    assert_eq!(pill.label.as_ref(), "Issue title");
    assert_eq!(pill.icon.as_ref().unwrap().name, "hash");
    assert_eq!(pill.style.background, Some(background));
    assert_eq!(pill.style.color, Some(tint));
    assert_eq!(theme_reads.load(Ordering::Relaxed), initial_theme_reads);

    let target = tree.guard(root);
    let start = TextPosition { node: paragraph, offset: link.range.start };
    let end = TextPosition { node: paragraph, offset: link.range.end };
    for action in [A11yAction::SetTextSelection { anchor: start, focus: end }, A11yAction::CopyTextSelection] {
        assert!(driver.dispatch(window, Input::Host(HostReply::AccessibilityAction { target, action })).unwrap().handled);
    }
    assert!(driver.take_host_commands().iter().any(|command| {
        matches!(command, HostCommand::Service { request: ServiceRequest::WriteClipboardText { text }, .. } if text == URL)
    }));

    driver
        .app_mut()
        .update_window(window, |write| {
            write.set(metadata, Some(LinkPillContent { label: Some("".into()), icon: Some(octara_icons::HASH.into()), tint: Some(tint) }));
        })
        .unwrap();
    let tree = driver.app().runtime().window(window).unwrap().tree();
    let text = tree.text(paragraph).unwrap();
    assert_eq!(text.text.as_ref(), "Ticket Original");
    let [link] = text.links.as_slice() else { panic!("expected one rich text link") };
    let [attachment] = text.attachments.as_slice() else { panic!("expected one inline pill") };
    let InlineContent::Pill(pill) = &attachment.content else { panic!("expected an inline pill") };
    let link = tree.link(*link).unwrap();
    assert_eq!(link.href.as_ref(), URL);
    assert_eq!(link.range, attachment.range);
    assert_eq!(&text.text[attachment.range.clone()], "Original");
    assert_eq!(pill.label.as_ref(), "Original");
    assert_eq!(pill.icon.as_ref().unwrap().name, "hash");
    assert_eq!(pill.style.color, Some(tint));
}

#[test]
fn recognition_preserves_explicit_links_and_code() {
    let mut driver = HeadlessDriver::default();
    let window = driver
        .app_mut()
        .open_window(WindowOptions::default(), |build| {
            let recognizer = TextRecognizer::new()
                .file_paths(|path| Some(format!("file:{path}")))
                .issues(|issue| Some(format!("issue:{}", &issue[1..])))
                .mentions(|mention| Some(format!("user:{}", &mention[1..])));
            let document = "@sam, #12, src/main.rs:42, ./Cargo.toml#L10-L20, [@alice](https://example.test), and `#9`.";
            markdown_with(build, document, MarkdownConfig::document().text_recognizer(recognizer)).test_id("markdown")
        })
        .unwrap();
    let tree = driver.app().runtime().window(window).unwrap().tree();
    let root = tree.test_ids("markdown")[0];
    let text = tree.subtree(root).find_map(|node| tree.text(node)).unwrap();
    let links: Vec<_> =
        text.links.iter().filter_map(|&node| tree.link(node)).map(|link| (&text.text[link.range.clone()], link.href.as_ref())).collect();
    assert_eq!(
        links,
        [
            ("@sam", "user:sam"),
            ("#12", "issue:12"),
            ("src/main.rs:42", "file:src/main.rs:42"),
            ("./Cargo.toml#L10-L20", "file:./Cargo.toml#L10-L20"),
            ("@alice", "https://example.test"),
        ],
    );
}

#[cfg(feature = "media")]
#[test]
fn pill_consumes_inline_media_inside_its_link_range() {
    let mut driver = HeadlessDriver::default();
    let window = driver
        .app_mut()
        .open_window(WindowOptions::default(), |build| {
            let config = MarkdownConfig::document()
                .link_pill_content(|_, _| Some(LinkPillContent { icon: Some(octara_icons::HASH.into()), ..Default::default() }));
            markdown_with(build, "See [first ![](https://example.test/image) and last](https://example.test/link).", config)
                .test_id("markdown")
        })
        .unwrap();
    let tree = driver.app().runtime().window(window).unwrap().tree();
    let root = tree.test_ids("markdown")[0];
    let text = tree.subtree(root).find_map(|node| tree.text(node)).unwrap();
    let [attachment] = text.attachments.as_slice() else { panic!("expected one inline pill") };
    let InlineContent::Pill(pill) = &attachment.content else { panic!("expected an inline pill") };
    assert_eq!(pill.label.as_ref(), "first https://example.test/image and last");
    assert_eq!(&text.text[attachment.range.clone()], pill.label.as_ref());
}
