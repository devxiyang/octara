use super::{Block, CalloutKind, Kind, ParseOptions, Scope, Style, parse};
use crate::DocumentStore;
use crate::stream::{StreamingFilter, StreamingPolicy, close_tail};

fn texts(doc: &DocumentStore) -> Vec<String> {
    fn walk(b: &Block, out: &mut Vec<String>) {
        match &b.kind {
            Kind::Paragraph(i) | Kind::Heading(_, i) | Kind::Spoiler(i) => out.push(i.text.clone()),
            Kind::Code { text, .. } => out.push(text.clone()),
            Kind::List { items, .. } => items.iter().for_each(|i| i.blocks.iter().for_each(|block| walk(block, out))),
            Kind::Quote { blocks, .. } => blocks.iter().for_each(|block| walk(block, out)),
            Kind::Image { alt, .. } => out.push(alt.clone()),
            Kind::Math { tex, .. } => out.push(tex.clone()),
            Kind::Directive { body, .. } => out.push(body.clone()),
            Kind::Video { src, .. } => out.push(src.clone()),
            Kind::Rule => out.push("---".into()),
            Kind::Table { head, rows, .. } => {
                out.push(head.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("|"));
                for r in rows {
                    out.push(r.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("|"));
                }
            }
        }
    }
    let mut out = Vec::new();
    doc.blocks().iter().for_each(|block| walk(block, &mut out));
    out
}

fn shown(src: &str) -> Vec<String> {
    let mut d = DocumentStore::new(true, ParseOptions::default());
    d.update(src);
    texts(&d)
}

const DOC: &str = "# Title\n\nHello **world** and [link](https://x.dev/a).\n\n- alpha\n- [x] beta\n  - nested *em*\n\n```rust\nfn main() {}\n```\n\n> quoted\n> text\n\n| a | b |\n|---|:-:|\n| 1 | `2` |\n\n---\n\n1. one\n2. two\n\nLast line.\n";

#[test]
fn blocks_and_styles() {
    let p = parse(DOC, 0, ParseOptions::default());
    let kinds: Vec<&str> = p
        .blocks
        .iter()
        .map(|b| match b.kind {
            Kind::Paragraph(_) => "p",
            Kind::Heading(..) => "h",
            Kind::Code { .. } => "code",
            Kind::List { .. } => "list",
            Kind::Quote { .. } => "quote",
            Kind::Image { .. } => "image",
            Kind::Math { .. } => "math",
            Kind::Spoiler(_) => "spoiler",
            Kind::Video { .. } => "video",
            Kind::Directive { .. } => "directive",
            Kind::Rule => "rule",
            Kind::Table { .. } => "table",
        })
        .collect();
    assert_eq!(kinds, ["h", "p", "list", "code", "quote", "table", "rule", "list", "p"]);
    assert_eq!(p.starts.len(), p.blocks.len());
    let Kind::Paragraph(i) = &p.blocks[1].kind else { panic!() };
    assert_eq!(i.text, "Hello world and link.");
    assert!(i.styles.contains(&(6..11, Style::Strong)));
    assert!(i.styles.contains(&(16..20, Style::Link("https://x.dev/a".into()))));
    assert_eq!(&DOC[p.blocks[1].source.clone()].trim_end(), &"Hello **world** and [link](https://x.dev/a).");
    let Kind::List { items, start, tight } = &p.blocks[2].kind else { panic!() };
    assert_eq!((*start, *tight, items.len(), items[1].task), (None, true, 2, Some(true)));
    assert_eq!(&DOC[items[1].task_source.clone().unwrap()], "[x]");
    assert!(matches!(items[1].blocks.last().unwrap().kind, Kind::List { .. }), "a nested list");
    let Kind::Code { lang, info, text, is_complete: true } = &p.blocks[3].kind else { panic!() };
    assert_eq!((lang.as_str(), info.as_str(), text.as_str()), ("rust", "rust", "fn main() {}"));
    let Kind::Table { head, rows, .. } = &p.blocks[5].kind else { panic!() };
    assert_eq!((head.len(), rows.len(), rows[0][1].styles.clone()), (2, 1, vec![(0..1, Style::Code)]));
    let Kind::List { start, .. } = &p.blocks[7].kind else { panic!() };
    assert_eq!(*start, Some(1));
    for (b, at) in p.blocks.iter().zip(&p.starts) {
        assert!(*at <= b.source.start && (*at == 0 || DOC.as_bytes()[at - 1] == b'\n'));
    }
}

#[test]
fn images_math_and_code_keep_their_meaning_and_source() {
    let src = "![**Moon** rise](moon.png \"Night\")\n\nAn ![inline](sun.png \"Day\") image, $x^2$, and a footnote[^n].\n\n$$\nx + y\n$$\n\n```rust,ignore Example title\nfn f() {}\n```\n\n[^n]: Text\n";
    let mut doc = DocumentStore::default();
    doc.update(src);
    assert_eq!(doc.source(), src);
    let blocks = doc.blocks();
    assert_eq!(blocks[0].kind, Kind::Image { url: "moon.png".to_owned(), alt: "Moon rise".to_owned(), title: "Night".to_owned() });
    assert_eq!(src[blocks[0].source.clone()].trim_end(), "![**Moon** rise](moon.png \"Night\")");
    let Kind::Paragraph(inline) = &blocks[1].kind else { panic!() };
    assert_eq!(inline.text, "An inline image, x^2, and a footnote[^n].");
    assert_eq!(
        inline.styles.iter().map(|(range, style)| (&inline.text[range.clone()], style.clone())).collect::<Vec<_>>(),
        [
            ("inline", Style::Image { url: "sun.png".to_owned(), title: "Day".to_owned() }),
            ("x^2", Style::Math),
            ("[^n]", Style::FootnoteRef),
        ]
    );
    assert_eq!(blocks[2].kind, Kind::Math { tex: "\nx + y\n".to_owned(), is_complete: true });
    assert_eq!(src[blocks[2].source.clone()].trim_end(), "$$\nx + y\n$$");
    assert_eq!(
        blocks[3].kind,
        Kind::Code {
            lang: "rust".to_owned(),
            info: "rust,ignore Example title".to_owned(),
            text: "fn f() {}".to_owned(),
            is_complete: true
        }
    );
}

#[test]
fn an_images_alt_text_has_no_active_nested_links_or_images() {
    for (source, alt, link) in [
        ("Before ![foo [bar](/destination)](/outer) after.", "foo bar", None),
        ("Before [![foo ![bar](/inner)](/outer)](/destination) after.", "foo bar", Some("/destination")),
        ("Before [![![](/inner)](/outer)](/destination) after.", "/outer", Some("/destination")),
    ] {
        let parsed = parse(source, 0, ParseOptions::default());
        let [Block { kind: Kind::Paragraph(inline), .. }] = parsed.blocks.as_slice() else { panic!("{:?}", parsed.blocks) };
        assert_eq!(inline.text, format!("Before {alt} after."));
        let range = 7..7 + alt.len();
        let mut expected = vec![(range.clone(), Style::Image { url: "/outer".into(), title: String::new() })];
        if let Some(link) = link {
            expected.push((range, Style::Link(link.into())));
        }
        assert_eq!(inline.styles, expected);
    }
}

#[test]
fn alerts_and_list_tightness_follow_block_structure() {
    let src = "> [!NOTE]\n> Note\n>\n>> [!TIP]\n>> Tip\n\n> [!IMPORTANT]\n> Important\n\n> [!WARNING]\n> Warning\n\n> [!CAUTION]\n> Caution\n\n> Ordinary quote\n\n- One\n  - Nested\n- Two\n\n  Second paragraph\n\n1. # Heading\n\n2. > Quote\n";
    let parsed = parse(src, 0, ParseOptions::default());
    let Kind::Quote { callout: Some(CalloutKind::Note), blocks } = &parsed.blocks[0].kind else { panic!() };
    assert!(matches!(blocks[1].kind, Kind::Quote { callout: Some(CalloutKind::Tip), .. }));
    for (block, kind) in
        parsed.blocks[1..5].iter().zip([Some(CalloutKind::Important), Some(CalloutKind::Warning), Some(CalloutKind::Caution), None])
    {
        assert!(matches!(block.kind, Kind::Quote { callout, .. } if callout == kind));
    }
    let Kind::List { tight: false, items, .. } = &parsed.blocks[5].kind else { panic!() };
    assert!(matches!(items[0].blocks[1].kind, Kind::List { tight: true, .. }));
    assert!(matches!(parsed.blocks[6].kind, Kind::List { start: Some(1), tight: false, .. }));
}

#[test]
fn parser_switches_apply_to_static_and_streaming_documents() {
    let src = "First\nsecond  \nthird ~~strike~~ and $x$.\n\n> [!NOTE]\n> note\n\n- [x] task\n\n| a | b |\n|---|---|\n";
    let options = ParseOptions { tables: false, strikethrough: false, tasklists: false, gfm_alerts: false, math: false, hard_breaks: true };
    for streaming in [true, false] {
        let mut doc = DocumentStore::new(streaming, options);
        doc.update(src);
        assert_eq!(texts(&doc), ["First\nsecond\nthird ~~strike~~ and $x$.", "[!NOTE]\nnote", "[x] task", "| a | b |\n|---|---|"]);
        let Kind::List { items, .. } = &doc.blocks()[2].kind else { panic!() };
        assert_eq!((items[0].task, items[0].task_source.clone()), (None, None));
        assert!(matches!(doc.blocks()[1].kind, Kind::Quote { callout: None, .. }));
        doc.update("Intro\n\n| a | b");
        assert_eq!(texts(&doc), ["Intro", "| a | b"]);
        doc.update("~x~ and ~~old");
        assert_eq!(texts(&doc), ["~x~ and ~~old"]);
        doc.update("- [x]");
        assert_eq!(texts(&doc), ["[x]"]);
    }
}

#[test]
fn a_complete_math_span_is_left_intact_during_tail_repair() {
    let mut doc = DocumentStore::new(true, ParseOptions::default());
    doc.update("$$\n| x | + * y $$");
    assert_eq!(doc.blocks()[0].kind, Kind::Math { tex: "\n| x | + * y ".to_owned(), is_complete: true });
    doc.update("A formula $x + *_2$ and **unfinished");
    let Kind::Paragraph(inline) = &doc.blocks()[0].kind else { panic!() };
    assert_eq!(inline.text, "A formula x + *_2 and unfinished");
    assert_eq!(inline.styles, [(10..17, Style::Math), (22..32, Style::Strong)]);
}

#[test]
fn stream_repairs_never_extend_original_ranges() {
    fn check_ranges(blocks: &[super::Block], source: &str) {
        for block in blocks {
            assert!(source.get(block.source.clone()).is_some(), "{block:?}");
            match &block.kind {
                Kind::List { items, .. } => {
                    for item in items {
                        if let Some(range) = &item.task_source {
                            assert_eq!(&source[range.clone()], "[X]");
                        }
                        check_ranges(&item.blocks, source);
                    }
                }
                Kind::Quote { blocks, .. } => check_ranges(blocks, source),
                _ => {}
            }
        }
    }
    let src = "Intro\n\n> [!NOTE]\n> Note and \\é\n>\n> - [X] `tool` and *unfinished";
    let mut doc = DocumentStore::new(true, ParseOptions::default());
    for (at, _) in src.char_indices().chain([(src.len(), ' ')]) {
        let prefix = &src[..at];
        doc.update(prefix);
        let mut fresh = DocumentStore::new(true, ParseOptions::default());
        fresh.update(prefix);
        assert_eq!(doc.blocks(), fresh.blocks(), "{prefix:?}");
        assert_eq!(doc.starts(), fresh.starts(), "{prefix:?}");
        assert_eq!(doc.source(), prefix);
        for block in doc.blocks() {
            check_ranges(std::slice::from_ref(block.as_ref()), prefix);
        }
    }
    assert_eq!(texts(&doc), ["Intro", "Note and \\é", "tool and unfinished"]);
    assert!(!doc.shared_leading_blocks().is_empty());

    let src = "Run **bold\u{3000}";
    doc.update(src);
    assert_eq!(texts(&doc), ["Run bold"]);
    for block in doc.blocks() {
        check_ranges(std::slice::from_ref(block.as_ref()), src);
    }
}

#[test]
fn a_growing_source_parses_its_end_as_a_whole_parse_does() {
    // Every prefix, grown char by char, equals its whole parse; most are tail parses.
    for src in [DOC, "Intro\n\npara one\nstill one\n\npara two\n\n- a\n- b\n\n  lazy\n\nend", "a\n\n```\ncode\n\nmore\n```\n\nb\n"] {
        let mut d = DocumentStore::new(false, ParseOptions::default());
        for (i, _) in src.char_indices().chain([(src.len(), ' ')]) {
            d.update(&src[..i]);
            let mut whole = DocumentStore::new(false, ParseOptions::default());
            whole.update(&src[..i]);
            assert_eq!(d.blocks(), whole.blocks(), "prefix {:?}", &src[..i]);
            assert_eq!(d.starts(), whole.starts(), "prefix {:?}", &src[..i]);
        }
        assert!(d.full_parses() * 2 < src.len() as u64, "{} whole parses for {} steps", d.full_parses(), src.len());
    }
}

#[test]
fn a_reference_definition_parses_whole() {
    let mut d = DocumentStore::new(false, ParseOptions::default());
    d.update("see [x]\n\nmore\n\n[x]");
    let before = d.full_parses();
    d.update_with_completion("see [x]\n\nmore\n\n[x]: https://x.dev\n", true, false);
    assert_eq!(d.full_parses(), before + 1);
    let Kind::Paragraph(i) = &d.blocks()[0].kind else { panic!() };
    assert_eq!(i.styles, vec![(4..5, Style::Link("https://x.dev".into()))]);
}

#[test]
fn an_open_span_shows_styled_and_no_marker() {
    assert_eq!(shown("…and **"), ["…and"]);
    assert_eq!(shown("use `"), ["use"]);
    assert_eq!(shown("a **b** _"), ["a b"]);
    assert_eq!(shown("snake_"), ["snake_"]);
    assert_eq!(shown("2*"), ["2*"]);
    assert_eq!(shown("2 * 3"), ["2 * 3"]);
    assert_eq!(shown("takes ~5 min"), ["takes ~5 min"]);
    assert_eq!(shown("see [the do"), ["see the do"]);
    assert_eq!(shown("cite [1] here"), ["cite [1] here"]);
    assert_eq!(shown("Steps:\n\n*"), ["Steps:"]);
    let mut d = DocumentStore::new(true, ParseOptions::default());
    d.update("…and **Settings ›");
    let Kind::Paragraph(i) = &d.blocks()[0].kind else { panic!() };
    assert_eq!((i.text.as_str(), i.styles.clone()), ("…and Settings ›", vec![(7..i.text.len(), Style::Strong)]));
    d.update("Run `cargo te");
    let Kind::Paragraph(i) = &d.blocks()[0].kind else { panic!() };
    assert_eq!((i.text.as_str(), i.styles.clone()), ("Run cargo te", vec![(4..12, Style::Code)]));
    d.update("see [the docs](https://example.com");
    let Kind::Paragraph(i) = &d.blocks()[0].kind else { panic!() };
    assert_eq!(i.styles, vec![(4..12, Style::Link("https://example.com".into()))]);
    // A finished paragraph keeps its markers as written.
    assert_eq!(shown("a **b\n\nc"), ["a **b", "c"]);
    assert_eq!(close_tail("a **b\n\n", Scope::Para, ParseOptions::default(), std::iter::empty()), None);
}

#[test]
fn every_prefix_of_a_stream_shows_a_prefix_of_its_text() {
    let src = "Hello **world**, this is _quite_ a `long` answer with [a link](https://x.dev) and ~~old~~ text.";
    let full = shown(src).concat();
    let mut d = DocumentStore::new(true, ParseOptions::default());
    for (i, _) in src.char_indices() {
        d.update(&src[..i]);
        let now = texts(&d).concat();
        assert!(full.starts_with(&now), "prefix {:?} shows {now:?}", &src[..i]);
    }
}

#[test]
fn table_lines_wait_for_their_delimiter_row() {
    for held in ["| File", "| File |", "| File |\n", "| File |\n|", "Intro\n| File | Change |\n"] {
        assert!(shown(held).iter().all(|t| !t.contains("File")), "{held:?}: {:?}", shown(held));
    }
    assert_eq!(shown("| File | Change"), ["File|Change"], "two cells: a header");
    assert_eq!(shown("| File |\n|---|\n"), ["File"]);
    assert_eq!(shown("| File\nis a word"), ["| File is a word"]);
}

#[test]
fn a_stream_grown_step_by_step_equals_its_fresh_parse() {
    let cjk = "## 标题 *强调*\n\n中文段落，**加粗**和`代码`。😀 [链接](https://x.dev)\n\n- 一\n- 二 ~~旧~~\n\n| 列 | 值 |\n|---|---|\n| 甲 | 乙 |\n";
    let extensions =
        "Intro\n\n> [!NOTE]\n> See ![a](a.jpg) and $x^2$\n\n![Sky](sky.jpg)\n\n$$\nx + y\n$$\n\nA footnote[^here].\n\n[^here]: Note.\n";
    let opaque = "Intro\n\n:::think budget='low'\n```\n:::\n\n||**secret**||\n\n> [!NOTE]\n> :::details\n> a\n> :::\n\n$$x + y$$\n\nEnd.";
    for src in [DOC, cjk, extensions, opaque] {
        let mut d = DocumentStore::new(true, ParseOptions::default());
        for (i, _) in src.char_indices().chain([(src.len(), ' ')]) {
            d.update(&src[..i]);
            let mut fresh = DocumentStore::new(true, ParseOptions::default());
            fresh.update(&src[..i]);
            assert_eq!(d.blocks(), fresh.blocks(), "prefix {:?}", &src[..i]);
        }
    }
}

#[test]
fn a_stream_parses_its_last_block_per_token() {
    let mut answer = String::new();
    for i in 0..90 {
        answer.push_str(&format!("Paragraph {i} with **bold** and `code` over several words to make it long.\n\n"));
        if i % 5 == 2 {
            answer.push_str("- a point\n- another point\n\n```\nfn f() {}\n```\n\n");
        }
    }
    let chars: Vec<char> = answer.chars().collect();
    let tokens: Vec<String> = chars.chunks(3).map(|c| c.iter().collect()).collect();
    assert!(tokens.len() >= 2000);
    let mut d = DocumentStore::new(true, ParseOptions::default());
    let mut src = String::new();
    for t in &tokens {
        src.push_str(t);
        d.update(&src);
    }
    let per_token = d.parsed_bytes() / tokens.len() as u64;
    // Each token parses the block it is in (about one paragraph), not the answer.
    assert!(per_token < 200, "{per_token} bytes parsed per token of a {}-byte answer", answer.len());
    // Whole parses only while the source is its first two blocks (a cut at the start is whole).
    let first_two = answer.match_indices("\n\n").nth(1).unwrap().0 / 3 + 1;
    assert!(d.full_parses() as usize <= first_two, "{} whole parses", d.full_parses());
}

#[test]
fn streaming_filters_hold_unfinished_blocks_until_they_close() {
    let hidden = StreamingFilter {
        tables: StreamingPolicy::Hidden,
        code_blocks: StreamingPolicy::Hidden,
        media: StreamingPolicy::Hidden,
        math: StreamingPolicy::Hidden,
        directives: StreamingPolicy::Hidden,
    };
    let mut suppressed = DocumentStore::new(true, ParseOptions::default()).with_streaming_filter(hidden);
    let mut progressive = DocumentStore::new(true, ParseOptions::default());
    for (unfinished, closing) in [
        ("\n\n```rust\nfoo", "\n```"),
        ("\n\n$$\nx + y", "\n$$"),
        ("\n\n$$x + y", "$$"),
        ("\n\n:::think model='small test'\n🪶\n", ":::"),
        ("\n\n![moon](moon", ".png)"),
        ("\n\n| A | B |\n|--", "-|---|\n"),
    ] {
        let prefix = format!("Intro{unfinished}");
        progressive.update(&prefix);
        suppressed.update(&prefix);
        assert_eq!(texts(&suppressed), ["Intro"], "{prefix:?}");
        let finished = prefix + closing;
        suppressed.update(&finished);
        assert_eq!(suppressed.blocks().len(), 2, "{finished:?} {:?}", suppressed.blocks());
        if !unfinished.contains("![") {
            assert_eq!(progressive.blocks().len(), 2, "{unfinished:?} {:?}", progressive.blocks());
        }
    }
}

#[test]
fn directives_are_lossless_opaque_and_can_appear_in_callouts() {
    let src = "> [!NOTE]\n> :::think model='small test' compact\n> *🪶*\n> [label]: /opaque\n> :::\n\nVisible [label].";
    let parsed = parse(src, 0, ParseOptions::default());
    let Kind::Quote { callout: Some(CalloutKind::Note), blocks } = &parsed.blocks[0].kind else { panic!() };
    let Kind::Directive { name, attrs, body, is_complete } = &blocks[0].kind else { panic!("{blocks:?}") };
    assert_eq!(name, "think");
    assert_eq!(attrs, &[("model".into(), "small test".into()), ("compact".into(), "".into())]);
    assert_eq!(body, "*🪶*\n[label]: /opaque\n");
    assert!(is_complete);
    assert_eq!(&src[blocks[0].source.clone()], ":::think model='small test' compact\n> *🪶*\n> [label]: /opaque\n> :::\n");
    let Kind::Paragraph(inline) = &parsed.blocks[1].kind else { panic!() };
    assert!(inline.styles.is_empty());
}

#[test]
fn an_opaque_block_cannot_open_a_markdown_fence_into_the_following_block() {
    let source = ":::think\n```\n:::\n\n:::next\ntail\n:::\n";
    let parsed = parse(source, 0, ParseOptions::default());
    assert_eq!(parsed.blocks.len(), 2, "{:?}", parsed.blocks);
    assert!(matches!(&parsed.blocks[0].kind, Kind::Directive { name, body, is_complete: true, .. } if name == "think" && body == "```\n"));
    assert!(matches!(&parsed.blocks[1].kind, Kind::Directive { name, body, is_complete: true, .. } if name == "next" && body == "tail\n"));
}

#[test]
fn spoilers_retain_nested_styles_and_you_can_escape_their_pipes() {
    let mut doc = DocumentStore::new(true, ParseOptions::default());
    doc.update("||**secret**");
    let Kind::Spoiler(inline) = &doc.blocks()[0].kind else { panic!("{:?}", doc.blocks()) };
    assert_eq!(inline.text, "secret");
    assert_eq!(inline.styles, [(0..6, Style::Strong), (0..6, Style::Spoiler)]);
    doc.update("||**secret**|");
    assert_eq!(texts(&doc), ["secret"]);
    doc.update("||**secret**||\n\nRead ||this|| beside `||code||` and \\||escaped||.");
    let Kind::Paragraph(inline) = &doc.blocks()[1].kind else { panic!() };
    assert_eq!(inline.text, "Read this beside ||code|| and ||escaped||.");
    assert_eq!(inline.styles.iter().filter(|(_, style)| *style == Style::Spoiler).count(), 1);
}

#[test]
fn an_open_inline_formula_is_repaired_without_changing_earlier_paragraphs() {
    let mut doc = DocumentStore::new(true, ParseOptions::default());
    doc.update("Leave **this\r\n\r\nEvaluate $x^2");
    assert_eq!(texts(&doc), ["Leave **this", "Evaluate x^2"]);
    let Kind::Paragraph(inline) = &doc.blocks()[1].kind else { panic!("{:?}", doc.blocks()) };
    #[cfg(not(feature = "math"))]
    assert_eq!(inline.styles, [(9..12, Style::Math)]);
    #[cfg(feature = "math")]
    assert_eq!(inline.styles, [(9..12, Style::MathSource { source: "$x^2".into(), display: false, is_complete: false })]);
}

#[test]
fn progressive_image_does_not_create_an_attachment_for_a_partial_destination() {
    let filter = StreamingFilter { media: StreamingPolicy::Progressive, ..StreamingFilter::default() };
    let mut doc = DocumentStore::new(true, ParseOptions::default()).with_streaming_filter(filter);
    doc.update("See ![blue moon](https://example.org");
    let Kind::Paragraph(inline) = &doc.blocks()[0].kind else { panic!("{:?}", doc.blocks()) };
    assert_eq!(inline.text, "See blue moon");
    assert!(inline.styles.is_empty());
    doc.update("![:::think](https://example.org");
    let Kind::Paragraph(inline) = &doc.blocks()[0].kind else { panic!("{:?}", doc.blocks()) };
    assert_eq!(inline.text, ":::think");
    assert!(inline.styles.is_empty());
}

#[test]
fn trailing_table_rows_wait_for_the_newline_or_the_end_of_the_stream() {
    let mut doc = DocumentStore::new(true, ParseOptions::default());
    doc.update("| A | B |\n|---|---|\n| 1 | 2");
    assert_eq!(texts(&doc), ["A|B"]);
    doc.update("| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |");
    assert_eq!(texts(&doc), ["A|B", "1|2"]);
    doc.set_finished(true);
    assert_eq!(texts(&doc), ["A|B", "1|2", "3|4"]);
}

#[test]
fn fenced_code_closes_at_the_parser_recognized_marker_inside_quotes() {
    let parsed = parse("> ```rust\r\n> f()\r\n> ```\r\n\r\n> ~~~\n> > ~~~", 0, ParseOptions::default());
    let Kind::Quote { blocks, .. } = &parsed.blocks[0].kind else { panic!() };
    assert!(matches!(blocks[0].kind, Kind::Code { is_complete: true, .. }), "{blocks:?}");
    let Kind::Quote { blocks, .. } = &parsed.blocks[1].kind else { panic!() };
    assert!(matches!(blocks[0].kind, Kind::Code { is_complete: false, .. }), "{blocks:?}");
}

#[test]
fn only_a_standalone_allowlisted_video_becomes_a_block_when_enabled() {
    let src = "<video src='clip.mp4' poster='still.jpg' controls></video>\n\nInline <video src='other.mp4'></video>.";
    let parsed = parse(src, 0, ParseOptions::default());
    #[cfg(feature = "media")]
    assert!(matches!(&parsed.blocks[0].kind, Kind::Video { src, poster } if src == "clip.mp4" && poster.as_deref() == Some("still.jpg")));
    #[cfg(not(feature = "media"))]
    assert!(matches!(&parsed.blocks[0].kind, Kind::Paragraph(inline) if inline.text.contains("<video")));
    assert!(matches!(&parsed.blocks[1].kind, Kind::Paragraph(inline) if inline.text.starts_with("Inline <video")));
}
