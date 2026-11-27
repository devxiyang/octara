//! Markdown rendering and interaction through the mounted native-layout harness.

use octara::{HostCommand, HostReply, Input, InteractionEvent, NodeId, PointerId, PointerKind, Tree, WindowRuntime, prelude::*};
use octara_inspect::{AgentAction, AgentCommand, AgentRequest, HeadlessDriver};
use octara_markdown::{
    BlockSlot, BlockSlotRenderer, CodeBlockStyle, HeadingStyle, InlineCodeStyle, LinkStyle, MarkdownConfig, MarkdownLocale,
    MarkdownRenderer, MarkdownTheme, ParseOptions, QuoteStyle, RenderBlock, RuleStyle, Style, TableStyle, default_heading, markdown,
    markdown_with,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(feature = "math")]
#[path = "markdown/math.rs"]
mod math;

struct Mounted {
    driver: HeadlessDriver,
    id: WindowId,
}

impl Mounted {
    fn new(mut driver: HeadlessDriver, view: impl FnOnce(&mut BuildCx<'_>) -> El + 'static) -> Self {
        let id = driver.app_mut().open_window(WindowOptions::default(), view).unwrap();
        Self { driver, id }
    }

    fn window(&self) -> &WindowRuntime {
        self.driver.app().runtime().window(self.id).unwrap()
    }

    fn tree(&self) -> &Tree {
        self.window().tree()
    }

    fn node(&self, name: &str) -> NodeId {
        let [node] = self.tree().test_ids(name) else { panic!("expected one node named {name}") };
        *node
    }

    fn update<R>(&mut self, update: impl FnOnce(&mut WriteCx<'_>) -> R) -> R {
        self.driver.app_mut().update_window(self.id, update).unwrap()
    }

    fn a11y(&mut self, name: &str, action: A11yAction) -> bool {
        dispatch_a11y(self, self.node(name), action)
    }

    fn key(&mut self, key: Key) -> bool {
        self.driver.dispatch(self.id, Input::Interaction(InteractionEvent::Key(KeyEvent::new(key, Modifiers::empty())))).unwrap().handled
    }
}

fn block(mounted: &Mounted, key: &str) -> NodeId {
    let root = mounted.node("markdown");
    mounted
        .tree()
        .children(root)
        .iter()
        .copied()
        .find(|&node| mounted.tree().provenance(node).and_then(|origin| origin.row_key.as_ref()).is_some_and(|row| row.to_string() == key))
        .unwrap_or_else(|| panic!("missing block {key}"))
}

fn descendant(mounted: &Mounted, root: NodeId, predicate: impl Fn(NodeId) -> bool) -> NodeId {
    mounted.tree().subtree(root).find(|&node| predicate(node)).unwrap()
}

fn code_line(mounted: &Mounted, block_key: &str, line_key: &str) -> NodeId {
    let row = descendant(mounted, block(mounted, block_key), |node| {
        mounted.tree().provenance(node).and_then(|origin| origin.row_key.as_ref()).is_some_and(|row| row.to_string() == line_key)
    });
    descendant(mounted, row, |node| mounted.tree().text(node).is_some() && mounted.tree().user_select(node) != Some(UserSelect::None))
}

fn dispatch_a11y(mounted: &mut Mounted, node: NodeId, action: A11yAction) -> bool {
    let target = mounted.tree().guard(node);
    mounted.driver.dispatch(mounted.id, Input::Host(HostReply::AccessibilityAction { target, action })).unwrap().handled
}

fn assert_copied(mounted: &mut Mounted, anchor: TextPosition, focus: TextPosition, want: &str) {
    assert!(mounted.a11y("markdown", A11yAction::SetTextSelection { anchor, focus }));
    assert!(mounted.a11y("markdown", A11yAction::CopyTextSelection));
    assert!(mounted.driver.take_host_commands().iter().any(|out| matches!(out,
        HostCommand::Service { request: ServiceRequest::WriteClipboardText { text }, .. } if text == want
    )));
}

#[test]
fn markdown_pointer_selection_copies_source_in_tree_order() {
    let mut driver = HeadlessDriver::default();
    let source = driver.app_mut().build(|b| b.text_buffer("# Title\n\nFirst **strong**")).unwrap();
    let mut mounted = Mounted::new(driver, move |b| markdown(b, source).test_id("markdown"));
    let [title, paragraph] = [block(&mounted, "b0"), block(&mounted, "b1")];
    let point = |node| {
        let rect = mounted.window().geometry(node).unwrap().rect;
        Point::new(rect.x + 2.0, rect.y + rect.h / 2.0)
    };
    let [start, end] = [point(title), point(paragraph)];
    let target = mounted.window().hit_test(end).unwrap();
    let title_hit = mounted.window().hit_test(start).unwrap();
    assert_eq!(target.hit.node(), Some(paragraph));
    assert_eq!(title_hit.hit.node(), Some(title));
    let pointer = PointerId(1);
    for click in 0..3 {
        let down = InteractionEvent::PointerDown {
            id: pointer,
            kind: PointerKind::Mouse,
            target,
            pos: end,
            button: Button::Primary,
            mods: Modifiers::empty(),
        };
        let (pos, hit) = if click == 2 { (start, title_hit) } else { (end, target) };
        let up = InteractionEvent::PointerUp { id: pointer, target: Some(hit), pos, mods: Modifiers::empty() };
        mounted.driver.dispatch(mounted.id, Input::Interaction(down)).unwrap();
        mounted.driver.dispatch(mounted.id, Input::Interaction(up)).unwrap();
    }
    let selection = mounted.tree().doc_selection(mounted.node("markdown"));
    assert_eq!(selection, Some([(paragraph, "First strong".len()), (title, 0)]));
    let event = InteractionEvent::Key(KeyEvent::new(Key::Char('c'), Modifiers::CTRL));
    assert!(mounted.driver.dispatch(mounted.id, Input::Interaction(event)).unwrap().handled);
    assert!(mounted.driver.take_host_commands().iter().any(|out| matches!(out,
        HostCommand::Service { request: ServiceRequest::WriteClipboardText { text }, .. } if text == "# Title\n\nFirst **strong**"
    )));
}

#[test]
fn markdown_stream_keeps_completed_blocks_and_formats_copy() {
    let mut driver = HeadlessDriver::default();
    let source = driver.app_mut().build(|b| b.text_buffer("# Title\n\nFirst **strong")).unwrap();
    let mut mounted = Mounted::new(driver, move |b| markdown(b, source).test_id("markdown"));
    let title = block(&mounted, "b0");
    let paragraph = block(&mounted, "b1");
    assert_eq!(mounted.tree().semantics(title).unwrap().role, Some(Role::Heading));
    assert_eq!(mounted.tree().text(paragraph).unwrap().text.as_ref(), "First strong");
    mounted.update(|w| w.append_text(source, "**\n\n```rs\nlet n = 1;\n```"));
    assert_eq!(block(&mounted, "b0"), title);
    assert_eq!(block(&mounted, "b1"), paragraph);
    let code = block(&mounted, "b2");
    assert_eq!(mounted.tree().semantics(code).unwrap().role, Some(Role::Code));
    assert!(mounted.tree().subtree(code).any(|node| { mounted.tree().text(node).is_some_and(|text| text.text.as_ref() == "let n = 1;") }));
    let copy = mounted
        .tree()
        .subtree(code)
        .find(|&node| mounted.tree().semantics(node).is_some_and(|sem| sem.role == Some(Role::Button)))
        .unwrap();
    let target = mounted.tree().guard(copy);
    assert!(
        mounted
            .driver
            .dispatch(mounted.id, Input::Host(HostReply::AccessibilityAction { target, action: A11yAction::Press }),)
            .unwrap()
            .handled
    );
    assert!(mounted.driver.take_host_commands().iter().any(|out| matches!(out,
        HostCommand::Service { request: ServiceRequest::WriteClipboardText { text }, .. } if text == "let n = 1;"
    )));
    assert!(mounted.tree().subtree(mounted.node("markdown")).any(|node| {
        mounted.tree().semantics(node).is_some_and(|sem| sem.live == Some(Live::Polite) && sem.label.as_deref() == Some("First strong"))
    }));
    assert!(mounted.a11y(
        "markdown",
        A11yAction::SetTextSelection {
            anchor: TextPosition { node: title, offset: 0 },
            focus: TextPosition { node: paragraph, offset: "First strong".len() },
        }
    ));
    assert!(mounted.a11y("markdown", A11yAction::CopyTextSelection));
    assert!(mounted.driver.take_host_commands().iter().any(|out| matches!(out,
        HostCommand::Service { request: ServiceRequest::WriteClipboardText { text }, .. }
            if text == "# Title\n\nFirst **strong**"
    )));

    assert!(mounted.a11y(
        "markdown",
        A11yAction::SetTextSelection {
            anchor: TextPosition { node: paragraph, offset: 6 },
            focus: TextPosition { node: paragraph, offset: "First strong".len() },
        },
    ));
    assert!(mounted.a11y("markdown", A11yAction::CopyTextSelection));
    assert!(mounted.driver.take_host_commands().iter().any(|out| matches!(out,
        HostCommand::Service { request: ServiceRequest::WriteClipboardText { text }, .. }
            if text == "strong"
    )));
}

#[test]
fn markdown_links_and_nested_rows_update_in_place() {
    let mut driver = HeadlessDriver::default();
    let source = driver
        .app_mut()
        .build(|b| b.signal("[Example](https://example.com)\n\n- [ ] Task\n\n| Start | End |\n| :-- | --: |\n| A | B |".to_owned()))
        .unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        let source = move |r: &mut ReadCx<'_>| r.get(source);
        col().display(Display::Flex).child(markdown(b, source).test_id("markdown"))
    });
    let link = block(&mounted, "b0");
    let task = block(&mounted, "b1");
    let table = block(&mounted, "b2");
    assert!(mounted.tree().subtree(link).any(|node| mounted.tree().link(node).is_some()));
    mounted.update(|w| w.update(source, |text| *text = text.replace("- [ ]", "- [x]").replace("| A | B |", "| A | C |")));
    assert_eq!(block(&mounted, "b1"), task);
    assert_eq!(block(&mounted, "b2"), table);
    assert!(mounted.tree().subtree(task).any(|id| mounted.tree().semantics(id).is_some_and(|sem| sem.checked == Some(Checked::Checked))));
    assert!(mounted.tree().subtree(table).any(|id| mounted.tree().text(id).is_some_and(|text| text.text.as_ref() == "C")));
}

#[test]
fn markdown_code_lines_retain_nodes_on_append_and_splice() {
    let mut driver = HeadlessDriver::default();
    let source = driver.app_mut().build(|b| b.text_buffer("```text\nfirst\nlast")).unwrap();
    let mut mounted = Mounted::new(driver, move |b| markdown(b, source).test_id("markdown"));
    let line = |mounted: &Mounted, key: &str| code_line(mounted, "b0", key);
    let first = line(&mounted, "l0");
    let last = line(&mounted, "l1");
    mounted.update(|w| w.append_text(source, " line\n\nend"));
    assert_eq!(line(&mounted, "l0"), first);
    assert_eq!(line(&mounted, "l1"), last);
    assert_eq!(mounted.tree().text(last).unwrap().text.as_ref(), "last line");
    assert_eq!(mounted.tree().text(line(&mounted, "l2")).unwrap().text.as_ref(), " ");
    assert!(mounted.update(|w| w.splice_text(source, TextBufferEdit::Splice { range: 8..13, insert: "rewritten".into() })));
    assert_eq!(line(&mounted, "l0"), first);
    assert_eq!(line(&mounted, "l1"), last);
    assert_eq!(mounted.tree().text(first).unwrap().text.as_ref(), "rewritten");
}

#[test]
fn markdown_styles_cover_inline_and_block_layout() {
    let ink = rgb(0x223344);
    let fill = rgb(0xe1e2e3);
    let accent = rgb(0x334455);
    let mut style = MarkdownTheme {
        block_gap: px(8.0),
        text_size: 17.0.into(),
        line_height: Some(LineHeight::Relative(1.5)),
        text_color: ink.into(),
        tight_list_gap: px(7.0),
        quote: QuoteStyle { bar_width: px(5.0), bar_color: accent.into(), text_color: accent.into(), ..QuoteStyle::default() },
        rule: RuleStyle { height: px(3.0), color: accent.into() },
        inline_code: InlineCodeStyle {
            background: Some(fill.into()),
            text_color: Some(ink.into()),
            border_color: Some(accent.into()),
            radius: 6.0,
            padding_x: 5.0,
            padding_y: 2.0,
            font_size: Some(13.0),
        },
        code_block: CodeBlockStyle {
            background: fill.into(),
            header_background: ink.into(),
            border_color: accent.into(),
            radius: 8.0.into(),
            padding: px(9.0),
            font_size: 11.0.into(),
            wrap_lines: true,
            show_copy: false,
            ..CodeBlockStyle::default()
        },
        table: TableStyle {
            border_color: accent.into(),
            header_background: ink.into(),
            row_background: fill.into(),
            striped_background: Some(accent.into()),
            cell_padding: [px(2.0), px(4.0), px(6.0), px(8.0)].into(),
            radius: 10.0.into(),
        },
        ..MarkdownTheme::default()
    };
    style.headings[5] = HeadingStyle { size: 21.0.into(), weight: 750, top_spacing: px(6.0), ..HeadingStyle::default() };
    let mounted = Mounted::new(HeadlessDriver::default(), move |b| {
        col()
            .child(markdown(b, "`default`").test_id("defaults"))
            .child(
                markdown_with(
                    b,
                    "###### Heading\n\nParagraph `inline`\n\n- One\n- Two\n\n> Quote\n\n---\n\n```lang\ncode\n```\n\n| Header |\n|---|\n| Row 1 |\n| Row 2 |",
                    MarkdownConfig::default().theme(style),
                )
                .test_id("markdown"),
            )
    });
    let tree = mounted.tree();
    let default_text = descendant(&mounted, mounted.node("defaults"), |node| tree.text(node).is_some());
    let default_span = tree.text(default_text).unwrap().runs[0].1;
    assert_eq!((default_span.radius, default_span.padding_x, default_span.padding_y), (Some(4.0), Some(4.0), Some(1.0)));
    assert_eq!(tree.layout_style(mounted.node("markdown")).gap.height, LayoutLength::Px(8.0));
    let heading = block(&mounted, "b0");
    assert_eq!(tree.layout_style(heading).margin.top, Some(LayoutLength::Px(6.0)));
    assert_eq!((tree.visual(heading).font_size, tree.visual(heading).font_weight), (21.0, 750));
    let paragraph = block(&mounted, "b1");
    let visual = tree.visual(paragraph);
    assert_eq!((visual.font_size, visual.line_height, visual.text), (17.0, 25.5, ink));
    let inline = tree.text(paragraph).unwrap().runs[0].1;
    assert_eq!(
        inline,
        Span {
            mono: true,
            size: Some(13.0),
            color: Some(ink.into()),
            background: Some(fill.into()),
            border_color: Some(accent.into()),
            radius: Some(6.0),
            padding_x: Some(5.0),
            padding_y: Some(2.0),
            ..Span::default()
        }
    );
    assert_eq!(tree.layout_style(block(&mounted, "b2")).gap.height, LayoutLength::Px(7.0));
    let quote = block(&mounted, "b3");
    assert_eq!(tree.layout_style(quote).border.left, LayoutLength::Px(5.0));
    assert_eq!(tree.visual(quote).border, accent);
    let quoted = descendant(&mounted, quote, |node| tree.text(node).is_some());
    assert_eq!(tree.visual(quoted).text, accent);
    let rule = block(&mounted, "b4");
    assert_eq!(tree.layout_style(rule).size.height, Some(LayoutLength::Px(3.0)));
    assert_eq!(tree.visual(rule).background, Some(accent));
    let code = block(&mounted, "b5");
    let visual = tree.visual(code);
    assert_eq!((visual.background, visual.border, visual.radius), (Some(fill), accent, [8.0; 4]));
    let code_header = descendant(&mounted, code, |node| tree.text(node).is_some_and(|text| text.text.as_ref() == "lang"));
    assert_eq!(tree.visual(tree.physical_parent(code_header).unwrap()).background, Some(ink));
    assert!(!tree.subtree(code).any(|node| tree.semantics(node).is_some_and(|slot| slot.role == Some(Role::Button))));
    let line = code_line(&mounted, "b5", "l0");
    assert_eq!((tree.visual(line).font_size, tree.visual(line).wrap), (11.0, TextWrap::Words));
    assert_eq!(tree.layout_style(tree.physical_parent(tree.physical_parent(line).unwrap()).unwrap()).padding.left, LayoutLength::Px(9.0));
    let table = block(&mounted, "b6");
    assert_eq!((tree.visual(table).border, tree.visual(table).radius), (accent, [10.0; 4]));
    let rows = tree.children(table);
    assert_eq!(rows.iter().map(|&row| tree.visual(row).background).collect::<Vec<_>>(), [Some(ink), Some(fill), Some(accent)]);
    assert_eq!(tree.layout_style(tree.children(tree.children(rows[1])[0])[0]).padding, [2.0, 4.0, 6.0, 8.0].map(LayoutLength::Px).into());
}

#[test]
fn markdown_links_keep_one_target_and_intercept_activation_before_host_navigation() {
    let mut driver = HeadlessDriver::default();
    let opened = driver.app_mut().build(|b| b.signal(String::new())).unwrap();
    let tint = rgb(0x445566);
    let mut mounted = Mounted::new(driver, move |b| {
        let options = MarkdownConfig::default()
            .theme(MarkdownTheme { link: LinkStyle { color: Some(tint.into()), underline: false }, ..MarkdownTheme::default() })
            .style_inline(|style, label, span| {
                if let Style::Link(href) = style
                    && href.starts_with("app:")
                {
                    assert_eq!(label, "View one");
                    span.radius = Some(3.0);
                    span.padding_x = Some(2.0);
                    span.background = Some(ColorToken::SurfacePressed.into());
                }
            })
            .on_link(move |w, event| {
                if event.kind == LinkEventKind::Press && event.href.starts_with("app:") {
                    assert_eq!(event.text.as_ref(), "View one");
                    assert!(event.rect.w > 0.0);
                    w.set(opened, event.href.to_string());
                    true
                } else {
                    false
                }
            });
        col()
            .child(markdown_with(b, "[View **one**](app:one) and [docs](https://example.com)", options).test_id("markdown"))
            .child(text(move |r: &mut ReadCx<'_>| r.get(opened)).test_id("opened"))
    });
    let paragraph = block(&mounted, "b0");
    let slot = mounted.tree().text(paragraph).unwrap();
    let [internal, external] = slot.links.as_slice() else { panic!("one accessible target per Markdown link") };
    let (internal, external) = (*internal, *external);
    let [(_, first), (_, strong), (_, docs)] = slot.runs.as_slice() else { panic!("expected individual inline runs") };
    assert_eq!((first.radius, strong.radius, strong.weight), (Some(3.0), Some(3.0), Some(700)));
    assert_eq!((docs.underline, docs.color), (false, Some(tint.into())));
    let internal_bounds = mounted.window().geometry(internal).unwrap().rect;
    let target = mounted.tree().guard(internal);
    let pos = point(internal_bounds.x + 2.0, internal_bounds.y + internal_bounds.h / 2.0);
    let receipt = mounted.driver.execute(AgentRequest::new(
        Some(mounted.id),
        AgentCommand::Act { target, action: AgentAction::Tap { pos, modifiers: Modifiers::empty() } },
    ));
    assert!(receipt.ok && receipt.handled == Some(true), "{receipt:?}");
    assert_eq!(mounted.tree().text(mounted.node("opened")).unwrap().text.as_ref(), "app:one");
    assert!(!mounted.driver.take_host_commands().iter().any(|command| matches!(command, HostCommand::OpenUrl { .. })));
    assert!(dispatch_a11y(&mut mounted, external, A11yAction::Focus));
    assert!(mounted.key(Key::Enter));
    assert!(
        mounted
            .driver
            .take_host_commands()
            .iter()
            .any(|command| matches!(command, HostCommand::OpenUrl { url } if url == "https://example.com"))
    );
}

#[cfg(feature = "highlight")]
struct RecordingHighlighter(Arc<Mutex<Vec<(String, String)>>>);

#[cfg(feature = "highlight")]
impl octara_markdown::CodeHighlighter for RecordingHighlighter {
    fn highlight(&self, lang: &str, code: &str) -> Option<Vec<Rich>> {
        self.0.lock().unwrap().push((lang.to_owned(), code.to_owned()));
        Some(code.split('\n').map(|line| rich().span(line, Span { weight: Some(700), ..Span::default() })).collect())
    }
}

#[cfg(feature = "highlight")]
#[test]
fn markdown_highlighter_receives_normalized_code_and_keeps_mixed_line_styles() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let highlighter_calls = calls.clone();
    let mounted = Mounted::new(HeadlessDriver::default(), move |b| {
        b.provide::<Arc<dyn octara_markdown::CodeHighlighter>>(Arc::new(RecordingHighlighter(highlighter_calls)));
        markdown(b, "```rust\r\nlet first = 1;\r\nsecond\rthird\nlast\r\n```").test_id("markdown")
    });
    let expected = ["let first = 1;", "second", "third", "last"];
    assert_eq!(*calls.lock().unwrap(), [("rust".to_owned(), expected.join("\n"))]);
    for (index, expected) in expected.into_iter().enumerate() {
        let node = code_line(&mounted, "b0", &format!("l{index}"));
        let line = mounted.tree().text(node).unwrap();
        assert_eq!(line.text.as_ref(), expected);
        assert_eq!(line.runs[0].1.weight, Some(700));
    }
}

#[cfg(feature = "highlight")]
#[test]
fn markdown_highlighter_only_revisits_changed_code_and_keeps_selectable_lines() {
    let mut driver = HeadlessDriver::default();
    let source = driver.app_mut().build(|b| b.text_buffer("Before\n\n```rust\nlet n = 1;\n```\n\n```js\nfirst\nlast")).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let highlighter_calls = calls.clone();
    let mut mounted = Mounted::new(driver, move |b| {
        b.provide::<Arc<dyn octara_markdown::CodeHighlighter>>(Arc::new(RecordingHighlighter(highlighter_calls)));
        markdown(b, source).test_id("markdown")
    });
    let before = block(&mounted, "b0");
    let rust = code_line(&mounted, "b1", "l0");
    let first = code_line(&mounted, "b2", "l0");
    let last = code_line(&mounted, "b2", "l1");
    assert_eq!(calls.lock().unwrap().len(), 2);
    let revision = mounted.tree().text(first).unwrap().text_rev;
    mounted.update(|w| w.append_text(source, " line"));
    assert_eq!(calls.lock().unwrap().last().unwrap(), &("js".to_owned(), "first\nlast line".to_owned()));
    assert_eq!(calls.lock().unwrap().len(), 3);
    assert_eq!(code_line(&mounted, "b1", "l0"), rust);
    assert_eq!(code_line(&mounted, "b2", "l0"), first);
    assert_eq!(code_line(&mounted, "b2", "l1"), last);
    assert_eq!(mounted.tree().text(first).unwrap().text_rev, revision);
    assert_eq!(mounted.tree().text(last).unwrap().runs[0].1.weight, Some(700));
    mounted.update(|w| w.append_text(source, "\n```\n\nAfter\n"));
    assert_eq!(calls.lock().unwrap().len(), 3, "closing a fence leaves its code snapshot equal");
    let after = block(&mounted, "b3");
    assert_copied(
        &mut mounted,
        TextPosition { node: before, offset: 0 },
        TextPosition { node: after, offset: 5 },
        "Before\n\n```rust\nlet n = 1;\n```\n\n```js\nfirst\nlast line\n```\n\nAfter\n",
    );
    let js = "Before\n\n```rust\nlet n = 1;\n```\n\n```".len();
    assert!(mounted.update(|w| w.splice_text(source, TextBufferEdit::Splice { range: js..js + 2, insert: "ts".into() })));
    assert_eq!(calls.lock().unwrap().last().unwrap(), &("ts".to_owned(), "first\nlast line".to_owned()));
    assert_eq!(calls.lock().unwrap().len(), 4);
    assert_eq!(code_line(&mounted, "b2", "l1"), last);
}

struct ChangeCount(Arc<Mutex<Vec<Arc<str>>>>);

impl BlockSlotRenderer for ChangeCount {
    fn render_slot(&self, _: &mut BuildCx<'_>, slot: &BlockSlot) -> Option<El> {
        let BlockSlot::Code { info, code, is_complete, .. } = slot else { return None };
        self.0.lock().unwrap().push(info.lang.clone());
        let (code, is_complete) = (*code, *is_complete);
        (info.lang.as_ref() == "diff").then(|| {
            text(move |r: &mut ReadCx<'_>| {
                let changes = r.with(code, |value| value.lines().count());
                let status = if r.get(is_complete) { "complete" } else { "streaming" };
                format!("{changes} changes ({status})")
            })
            .test_id("diff")
        })
    }
}

#[test]
fn markdown_custom_code_view_follows_streaming_and_preserves_source_copy() {
    let mut driver = HeadlessDriver::default();
    let source = driver.app_mut().build(|b| b.text_buffer("Before\n\n```diff\n+ a")).unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let render_calls = calls.clone();
    let mut mounted = Mounted::new(driver, move |b| {
        b.provide::<Arc<dyn BlockSlotRenderer>>(Arc::new(ChangeCount(render_calls)));
        let options = MarkdownConfig::default().theme(MarkdownTheme {
            code_block: CodeBlockStyle { show_header: false, show_copy: false, ..CodeBlockStyle::default() },
            ..MarkdownTheme::default()
        });
        markdown_with(b, source, options).test_id("markdown")
    });
    let before = block(&mounted, "b0");
    let code = block(&mounted, "b1");
    let diff = mounted.node("diff");
    assert_eq!(mounted.tree().text(diff).unwrap().text.as_ref(), "1 changes (streaming)");
    mounted.update(|w| w.append_text(source, "\n- b\n```\n\nAfter\n"));
    assert_eq!(block(&mounted, "b1"), code);
    assert_eq!(mounted.node("diff"), diff);
    assert_eq!(mounted.tree().text(diff).unwrap().text.as_ref(), "2 changes (complete)");
    assert_eq!(calls.lock().unwrap().iter().map(AsRef::as_ref).collect::<Vec<_>>(), ["diff"]);
    let after = block(&mounted, "b2");
    assert_copied(
        &mut mounted,
        TextPosition { node: before, offset: 0 },
        TextPosition { node: after, offset: 5 },
        "Before\n\n```diff\n+ a\n- b\n```\n\nAfter\n",
    );
    assert!(mounted.update(|w| w.splice_text(source, TextBufferEdit::Splice { range: 11..15, insert: "text".into() })));
    assert_eq!(block(&mounted, "b1"), code);
    assert!(mounted.tree().test_ids("diff").is_empty());
    assert_eq!(calls.lock().unwrap().iter().map(AsRef::as_ref).collect::<Vec<_>>(), ["diff", "text"]);
    let first = code_line(&mounted, "b1", "l0");
    assert_eq!(mounted.tree().text(first).unwrap().text.as_ref(), "+ a");
    assert_eq!(mounted.tree().children(code).len(), 1);
    assert_eq!(mounted.tree().children(mounted.tree().children(code)[0]).len(), 1, "fallback only renders the body");
    let second = code_line(&mounted, "b1", "l1");
    assert!(mounted.update(|w| w.splice_text(source, TextBufferEdit::Splice { range: 11..15, insert: "data".into() })));
    assert_eq!(calls.lock().unwrap().iter().map(AsRef::as_ref).collect::<Vec<_>>(), ["diff", "text", "data"]);
    assert_eq!(code_line(&mounted, "b1", "l0"), first);
    assert_eq!(code_line(&mounted, "b1", "l1"), second);
}

struct CodeSummary;

impl BlockSlotRenderer for CodeSummary {
    fn render_slot(&self, _: &mut BuildCx<'_>, slot: &BlockSlot) -> Option<El> {
        let BlockSlot::Code { info, .. } = slot else { return None };
        match info.lang.as_ref() {
            "summary" => Some(text("Summary")),
            "split" => Some(col().child(text("One").test_id("one")).child(text("Two").test_id("two"))),
            _ => None,
        }
    }
}

#[test]
fn markdown_custom_code_copy_uses_mounted_row_boundaries() {
    const SOURCE: &str = "```summary\nalpha\nbeta\n```\n\n```split\nx\n```\n\n```summary\ny\nz\n```\n";
    let mut mounted = Mounted::new(HeadlessDriver::default(), |b| {
        b.provide::<Arc<dyn BlockSlotRenderer>>(Arc::new(CodeSummary));
        markdown(b, SOURCE).test_id("markdown")
    });
    let summary = |mounted: &Mounted, key| descendant(mounted, block(mounted, key), |node| mounted.tree().text(node).is_some());
    let first = summary(&mounted, "b0");
    let last = summary(&mounted, "b2");
    let one = mounted.node("one");
    let two = mounted.node("two");
    let pos = |node, offset| TextPosition { node, offset };
    assert_copied(&mut mounted, pos(first, 0), pos(first, 7), "```summary\nalpha\nbeta\n```");
    assert_copied(&mut mounted, pos(first, 0), pos(first, 3), "Sum");
    assert_copied(&mut mounted, pos(one, 0), pos(one, 3), "One");
    assert_copied(&mut mounted, pos(two, 0), pos(two, 3), "Two");
    assert_copied(&mut mounted, pos(one, 0), pos(two, 3), "```split\nx\n```");
    assert_copied(&mut mounted, pos(last, 7), pos(first, 0), SOURCE);
}

struct SharedRenderer {
    compact: ReadSignal<bool>,
    headings: Signal<usize>,
}

impl MarkdownRenderer for SharedRenderer {
    fn theme(&self, read: &mut ReadCx<'_>) -> MarkdownTheme {
        if read.get(self.compact) { MarkdownTheme::compact() } else { MarkdownTheme::chat() }
    }

    fn parse_options(&self) -> ParseOptions {
        ParseOptions { hard_breaks: true, ..ParseOptions::default() }
    }

    fn render_heading(&self, build: &mut BuildCx<'_>, block: &RenderBlock, level: u8) -> El {
        let count = self.headings;
        default_heading(build, block, level).on_press(move |w| w.update(count, |n| *n += 1))
    }
}

#[test]
fn markdown_shares_renderer_and_reacts_to_density_and_subtree_tokens() {
    let light = rgb(0xf9fbff);
    let bubble = rgb(0x173f9a);
    let mut driver = HeadlessDriver::default();
    let (compact, headings, source) =
        driver.app_mut().build(|b| (b.signal(false), b.signal(0_usize), b.text_buffer("# Heading\n\nOne\nTwo `code`"))).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        let renderer: Arc<dyn MarkdownRenderer> = Arc::new(SharedRenderer { compact: compact.read_only(), headings });
        b.provide(renderer);
        col()
            .bg(bubble)
            .theme_patch(ThemePatch::default().color(ColorToken::Text, light).color(ColorToken::SurfacePressed, bubble))
            .child(markdown(b, source).test_id("markdown"))
            .child(markdown(b, "# Other").test_id("other"))
    });
    let first = block(&mounted, "b0");
    let paragraph = block(&mounted, "b1");
    let second = descendant(&mounted, mounted.node("other"), |node| mounted.tree().text(node).is_some());
    let before = mounted.tree().visual(first).font_size;
    assert_eq!(mounted.tree().visual(paragraph).text, light);
    assert_eq!(mounted.tree().text(paragraph).unwrap().text.as_ref(), "One\nTwo code");
    assert_eq!(mounted.tree().text(paragraph).unwrap().runs[0].1.padding_y, Some(1.0));
    assert!(dispatch_a11y(&mut mounted, first, A11yAction::Press));
    assert!(dispatch_a11y(&mut mounted, second, A11yAction::Press));
    assert_eq!(mounted.update(|w| w.read(|r| r.get(headings))), 2);

    mounted.update(|w| w.set(compact, true));
    assert_eq!(block(&mounted, "b0"), first);
    assert!(mounted.tree().visual(first).font_size < before);
    assert_eq!(mounted.tree().visual(first).font_size, mounted.tree().visual(second).font_size);
    assert_eq!(mounted.tree().visual(paragraph).text, light);
    mounted.update(|w| w.append_text(source, " and more"));
    assert_eq!(block(&mounted, "b0"), first);
    assert_eq!(block(&mounted, "b1"), paragraph);
}

#[test]
fn markdown_callouts_and_code_controls_use_locale_and_theme() {
    let locale = MarkdownLocale { copied: "Copied to the clipboard", ..MarkdownLocale::zh_cn() };
    let mut mounted = Mounted::new(HeadlessDriver::default(), move |b| {
        b.provide(locale);
        let mut theme = MarkdownTheme::chat();
        theme.code_block.line_numbers = true;
        theme.code_block.max_height = Some(px(70.0));
        markdown_with(b, "> [!WARNING]\n> Look out\n\n```rust example.rs\na\nb\n```", MarkdownConfig::chat().theme(theme))
            .test_id("markdown")
    });
    let quote = block(&mounted, "b0");
    let code = block(&mounted, "b1");
    assert!(mounted.tree().subtree(quote).any(|node| mounted.tree().icon(node).is_some()));
    assert!(mounted.tree().subtree(quote).any(|node| mounted.tree().text(node).is_some_and(|text| text.text.as_ref() == locale.warning)));
    let header = descendant(&mounted, code, |node| mounted.tree().text(node).is_some_and(|text| text.text.as_ref() == "rust example.rs"));
    let header = mounted.tree().physical_parent(header).unwrap();
    assert_eq!(mounted.tree().layout_style(header).border.bottom, LayoutLength::Px(1.0));
    let line = code_line(&mounted, "b1", "l0");
    assert_eq!(mounted.tree().text(line).unwrap().text.as_ref(), "a");
    let number = descendant(&mounted, code, |node| mounted.tree().text(node).is_some_and(|text| text.text.as_ref() == "1"));
    assert!(mounted.tree().visual(number).tabular_nums);
    assert_eq!(mounted.tree().user_select(number), Some(UserSelect::None));
    let copy = descendant(&mounted, code, |node| mounted.tree().semantics(node).is_some_and(|sem| sem.role == Some(Role::Button)));
    let label = |mounted: &Mounted| mounted.tree().semantics(copy).unwrap().label.clone();
    assert_eq!(label(&mounted).as_deref(), Some(locale.copy));
    let press_copy = |mounted: &mut Mounted, outcome| {
        assert!(dispatch_a11y(mounted, copy, A11yAction::Press));
        let id = mounted
            .driver
            .take_host_commands()
            .into_iter()
            .find_map(|command| match command {
                HostCommand::Service { id, request: ServiceRequest::WriteClipboardText { text }, .. } => {
                    assert_eq!(text, "a\nb");
                    Some(id)
                }
                _ => None,
            })
            .expect("clipboard request");
        mounted.driver.dispatch(mounted.id, Input::Host(HostReply::ServiceReply { id, outcome })).unwrap();
    };
    press_copy(&mut mounted, ServiceOutcome::Error(ServiceError::Denied));
    assert_eq!(label(&mounted).as_deref(), Some(locale.copy));
    press_copy(&mut mounted, ServiceOutcome::ClipboardWritten);
    assert_eq!(label(&mounted).as_deref(), Some(locale.copied));
    mounted.driver.advance_to(Duration::from_millis(750)).unwrap();
    press_copy(&mut mounted, ServiceOutcome::ClipboardWritten);
    mounted.driver.advance_to(Duration::from_millis(1500)).unwrap();
    assert_eq!(label(&mounted).as_deref(), Some(locale.copied));
    mounted.driver.advance_to(Duration::from_millis(2250)).unwrap();
    assert_eq!(label(&mounted).as_deref(), Some(locale.copy));
}

#[test]
fn markdown_table_uses_aligned_intrinsic_columns_and_scrolls() {
    let mut driver = HeadlessDriver::default();
    let wide = driver.app_mut().build(|b| b.signal(true)).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        col().w(200).child(
            markdown(b, move |r: &mut ReadCx<'_>| {
                let description = if r.get(wide) { "long column description ".repeat(12) } else { "small".into() };
                format!("| N | Description |\n| --: | :-- |\n| 1 | {description} |\n| 2 | short |")
            })
            .test_id("markdown"),
        )
    });
    let table = block(&mounted, "b0");
    let rows = mounted.tree().children(table).to_vec();
    let cells: Vec<_> = rows.iter().map(|&row| mounted.tree().children(row).to_vec()).collect();
    let rect = |mounted: &Mounted, node| mounted.window().geometry(node).unwrap().rect;
    let columns = |mounted: &Mounted| [rect(mounted, cells[0][0]), rect(mounted, cells[0][1])];
    let [narrow, description] = columns(&mounted);
    assert_eq!(mounted.tree().layout_style(table).overflow_x, Overflow::Scroll);
    assert!(narrow.w < description.w);
    assert!(description.w > rect(&mounted, table).w);
    for row in &cells[1..] {
        for (&cell, column) in row.iter().zip([narrow, description]) {
            let bounds = rect(&mounted, cell);
            assert!((bounds.w - column.w).abs() < 0.5 && (bounds.x - column.x).abs() < 0.5);
        }
    }
    let viewport = rect(&mounted, table);
    let wheel = InteractionEvent::Wheel {
        pos: point(viewport.x + viewport.w / 2.0, viewport.y + viewport.h / 2.0),
        delta: Vec2::new(120.0, 0.0),
        phase: octara::WheelPhase::Discrete,
        mods: Modifiers::empty(),
    };
    assert!(mounted.driver.dispatch(mounted.id, Input::Interaction(wheel)).unwrap().handled);
    assert!(mounted.tree().scroll(table).unwrap().offset.x > 0.0);

    mounted.update(|w| w.set(wide, false));
    assert_eq!(block(&mounted, "b0"), table);
    assert_eq!(mounted.tree().children(table), rows);
    let [after_narrow, after_description] = columns(&mounted);
    assert_eq!(after_narrow.w, narrow.w);
    assert!(after_description.w < description.w);
}

#[test]
fn markdown_task_toggle_reports_the_current_source_range() {
    let mut driver = HeadlessDriver::default();
    let source = driver.app_mut().build(|b| b.text_buffer("Before\n\n- [ ] Task")).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        let config = MarkdownConfig::document().on_task_toggle(move |w, range, checked| {
            assert!(checked);
            assert!(w.splice_text(source, TextBufferEdit::Splice { range, insert: "[x]".into() }));
        });
        markdown_with(b, source, config).test_id("markdown")
    });
    let list = block(&mounted, "b1");
    let marker = descendant(&mounted, list, |node| mounted.tree().semantics(node).is_some_and(|sem| sem.role == Some(Role::CheckBox)));
    mounted.update(|w| w.splice_text(source, TextBufferEdit::Splice { range: 0..6, insert: "Updated preface".into() }));
    assert!(mounted.key(Key::Tab));
    assert_eq!(mounted.tree().focus(), Some(marker));
    assert!(mounted.key(Key::Space));
    assert_eq!(mounted.tree().semantics(marker).unwrap().checked, Some(Checked::Checked));
    assert_eq!(mounted.update(|w| w.read(|r| r.with_text(source, str::to_owned))), "Updated preface\n\n- [x] Task");
}

#[test]
fn markdown_without_a_link_callback_inherits_an_ancestor_handler() {
    let mut driver = HeadlessDriver::default();
    let seen = driver.app_mut().build(|b| b.signal(String::new())).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        col()
            .on_link(move |w, link| {
                if link.kind != LinkEventKind::Press {
                    return false;
                }
                assert_eq!(&*link.text, "Open");
                assert!(link.rect.w > 0.0);
                w.set(seen, link.href.to_string());
                true
            })
            .child(markdown(b, "[Open](app:panel)").test_id("markdown"))
    });
    let link = mounted.tree().text(block(&mounted, "b0")).unwrap().links[0];
    assert!(dispatch_a11y(&mut mounted, link, A11yAction::Focus));
    assert!(mounted.key(Key::Enter));
    assert_eq!(mounted.update(|w| w.read(|r| r.get(seen))), "app:panel");
}

#[cfg(not(feature = "media"))]
#[test]
fn markdown_images_and_math_have_accessible_selectable_defaults() {
    let mut mounted =
        Mounted::new(HeadlessDriver::default(), |b| markdown(b, "![Chart](image.png \"Title\")\n\n$$x+1$$").test_id("markdown"));
    let image = block(&mounted, "b0");
    let math = descendant(&mounted, block(&mounted, "b1"), |node| mounted.tree().text(node).is_some());
    let formula = if cfg!(feature = "math") { "$$x+1$$" } else { "x+1" };
    let semantics = mounted.tree().semantics(image).unwrap();
    assert_eq!(semantics.role, Some(Role::Image));
    assert_eq!(semantics.label.as_deref(), Some("Chart"));
    assert_eq!(mounted.tree().text(math).unwrap().text.as_ref(), formula);
    assert_copied(
        &mut mounted,
        TextPosition { node: image, offset: 0 },
        TextPosition { node: math, offset: formula.len() },
        "![Chart](image.png \"Title\")\n\n$$x+1$$",
    );
}

struct GrowingSlots(Arc<Mutex<Vec<octara_markdown::SlotId>>>);

impl BlockSlotRenderer for GrowingSlots {
    fn render_slot(&self, _: &mut BuildCx<'_>, slot: &BlockSlot) -> Option<El> {
        let (id, body, done, tag) = match slot {
            BlockSlot::Math { id, tex, is_complete, .. } => (id, *tex, *is_complete, "math"),
            BlockSlot::Directive { id, name, attrs, body, is_complete, .. } => {
                assert_eq!(name.as_ref(), "think");
                assert_eq!(attrs[0].0.as_ref(), "model");
                (id, *body, *is_complete, "directive")
            }
            _ => return None,
        };
        self.0.lock().unwrap().push(id.clone());
        Some(text(move |r: &mut ReadCx<'_>| format!("{} {}", r.get(body).trim(), r.get(done))).test_id(tag))
    }
}

#[test]
fn markdown_math_and_directive_slots_keep_mounted_state_on_fence_close() {
    let cases = [("$$\nx", " + y", "\n$$\n", "math", "x + y"), (":::think model=small\none", " two", "\n:::\n", "directive", "one two")];
    for (opening, middle, closing, tag, expected) in cases {
        let mut driver = HeadlessDriver::default();
        let buffer = driver.app_mut().build(|b| b.text_buffer(opening)).unwrap();
        let slots = Arc::new(Mutex::new(Vec::new()));
        let render_slots = slots.clone();
        let mut mounted = Mounted::new(driver, move |b| {
            b.provide::<Arc<dyn BlockSlotRenderer>>(Arc::new(GrowingSlots(render_slots)));
            markdown(b, buffer).test_id("markdown")
        });
        let node = mounted.node(tag);
        mounted.update(|w| w.append_text(buffer, middle));
        assert_eq!(mounted.node(tag), node);
        assert_eq!(mounted.tree().text(node).unwrap().text.as_ref(), format!("{expected} false"));
        mounted.update(|w| w.append_text(buffer, closing));
        assert_eq!(mounted.node(tag), node);
        assert_eq!(mounted.tree().text(node).unwrap().text.as_ref(), format!("{expected} true"));
        assert_eq!(slots.lock().unwrap().len(), 1);
    }
}

#[test]
fn markdown_spoilers_reveal_with_keyboard_and_keep_nested_inline_styles() {
    let mut mounted = Mounted::new(HeadlessDriver::default(), |b| {
        markdown(b, "Read ||**first**|| before ||second||.\n\n||**block spoiler**||").test_id("markdown")
    });
    let paragraph = block(&mounted, "b0");
    let shown = |m: &Mounted| m.tree().text(paragraph).unwrap().text.to_string();
    assert_eq!(shown(&mounted), "Read Show spoiler before Show spoiler.");
    let first = mounted.tree().text(paragraph).unwrap().links[0];
    assert!(dispatch_a11y(&mut mounted, first, A11yAction::Focus));
    assert!(mounted.key(Key::Enter));
    assert_eq!(shown(&mounted), "Read first before Show spoiler.");
    let span = &mounted.tree().text(paragraph).unwrap().runs[0].1;
    assert_eq!(span.weight, Some(700));
    let second = mounted.tree().text(paragraph).unwrap().links[0];
    assert!(dispatch_a11y(&mut mounted, second, A11yAction::Focus));
    assert!(mounted.key(Key::Enter));
    assert_eq!(shown(&mounted), "Read first before second.");

    let block_spoiler = block(&mounted, "b1");
    let button =
        descendant(&mounted, block_spoiler, |node| mounted.tree().semantics(node).is_some_and(|sem| sem.role == Some(Role::Button)));
    assert!(dispatch_a11y(&mut mounted, button, A11yAction::Focus));
    assert!(mounted.key(Key::Space));
    let revealed = descendant(&mounted, block_spoiler, |node| mounted.tree().text(node).is_some());
    let text = mounted.tree().text(revealed).unwrap();
    assert_eq!(text.text.as_ref(), "block spoiler");
    assert_eq!(text.runs[0].1.weight, Some(700));
}

#[test]
fn markdown_reports_code_and_image_presses_and_image_url_context_action() {
    let mut driver = HeadlessDriver::default();
    let (image_pressed, code_pressed, image_menu) =
        driver.app_mut().build(|b| (b.signal(false), b.signal(false), b.signal(false))).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        let config = MarkdownConfig::default()
            .on_image_press(move |w, slot| {
                let BlockSlot::Image { url, occurrence_index, .. } = slot else { panic!("expected an image slot") };
                assert_eq!(url.as_ref(), "https://example.test/moon.png");
                assert_eq!(*occurrence_index, 0);
                w.set(image_pressed, true);
            })
            .on_code_block_press(move |w, slot| {
                let BlockSlot::Code { info, code, .. } = slot else { panic!("expected a code slot") };
                assert_eq!(info.lang.as_ref(), "rs");
                assert_eq!(w.read(|r| r.get(*code)).as_ref(), "let n = 1;");
                w.set(code_pressed, true);
            })
            .on_context_menu(move |w, menu| {
                assert!(menu.items.contains(&ContextMenuItem::Copy {
                    format: octara_markdown::SmartCopy::ImageUrl.into(),
                    text: "https://example.test/moon.png".into(),
                }));
                w.set(image_menu, true);
                true
            });
        markdown_with(b, "![Moon](https://example.test/moon.png)\n\n```rs\nlet n = 1;\n```", config).test_id("markdown")
    });
    let image = block(&mounted, "b0");
    let code = block(&mounted, "b1");
    assert!(dispatch_a11y(&mut mounted, image, A11yAction::Press));
    assert!(dispatch_a11y(&mut mounted, code, A11yAction::Press));
    assert!(dispatch_a11y(&mut mounted, image, A11yAction::ShowContextMenu));
    assert!(mounted.update(|w| w.read(|r| r.get(image_pressed) && r.get(code_pressed) && r.get(image_menu))));
}

#[test]
fn markdown_streaming_effect_tracks_rendered_text_instead_of_markdown_delimiters() {
    let mut driver = HeadlessDriver::default();
    let source = driver.app_mut().build(|b| b.text_buffer("**猫")).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        let renderer = MarkdownConfig::default().streaming_effects(octara_markdown::StreamingEffects::Fade { duration_secs: 0.25 });
        markdown_with(b, source, renderer).test_id("markdown")
    });
    let first = block(&mounted, "b0");
    let reveal = mounted.tree().text_reveal(first).unwrap();
    assert_eq!(reveal.revealed_bytes, "猫".len());
    assert!(!reveal.finished);
    mounted.update(|w| w.append_text(source, "**\n\n犬"));
    assert_eq!(block(&mounted, "b0"), first);
    let next = block(&mounted, "b1");
    assert!(mounted.tree().text_reveal(first).unwrap().finished);
    assert_eq!(mounted.tree().text_reveal(next).unwrap().revealed_bytes, "犬".len());
}

#[test]
fn markdown_inline_images_report_occurrences_and_preserve_wrapping_links() {
    let mut driver = HeadlessDriver::default();
    let seen = driver.app_mut().build(|b| b.signal(Vec::<String>::new())).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        let config = MarkdownConfig::default()
            .on_image_press(move |w, slot| {
                let BlockSlot::Image { url, occurrence_index, .. } = slot else { panic!("expected an image slot") };
                w.update(seen, |seen| seen.push(format!("{occurrence_index}: {url}")));
            })
            .on_link(move |w, event| {
                if event.kind == LinkEventKind::Press {
                    w.update(seen, |seen| seen.push(event.href.to_string()));
                    return true;
                }
                false
            });
        markdown_with(
            b,
            "See ![Moon](https://example.test/moon.png) and [![Sun](https://example.test/sun.png)](https://example.test/view).",
            config,
        )
        .test_id("markdown")
    });
    let paragraph = block(&mounted, "b0");
    let links = mounted.tree().text(paragraph).unwrap().links.clone();
    assert_eq!(links.len(), 2);
    for link in links {
        assert!(dispatch_a11y(&mut mounted, link, A11yAction::Focus));
        assert!(mounted.key(Key::Enter));
    }
    assert_eq!(mounted.update(|w| w.read(|r| r.get(seen))), ["0: https://example.test/moon.png", "https://example.test/view"],);
}

#[cfg(feature = "media")]
#[test]
fn markdown_inline_media_keeps_outer_images_and_trailing_decorative_images() {
    let mounted = Mounted::new(HeadlessDriver::default(), |b| {
        markdown(
            b,
            "before ![foo ![inner](https://example.test/inner.png)](https://example.test/outer.png) ![](https://example.test/decorative.png)",
        )
        .test_id("markdown")
    });
    let paragraph = block(&mounted, "b0");
    let text = mounted.tree().text(paragraph).unwrap();
    let [outer, decorative] = text.attachments.as_slice() else { panic!("expected the outer and decorative images") };
    assert_eq!(&text.text[outer.range.clone()], "foo inner");
    assert_eq!(decorative.range, text.text.len()..text.text.len());
}
