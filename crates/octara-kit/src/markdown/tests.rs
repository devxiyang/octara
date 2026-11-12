use super::parse::{Kind, Scope, Style, parse};
use super::stream::{Doc, close_tail};

fn texts(doc: &Doc) -> Vec<String> {
    fn walk(bs: &[super::Block], out: &mut Vec<String>) {
        for b in bs {
            match &b.kind {
                Kind::Paragraph(i) | Kind::Heading(_, i) => out.push(i.text.clone()),
                Kind::Code { text, .. } => out.push(text.clone()),
                Kind::List { items, .. } => items.iter().for_each(|i| walk(&i.blocks, out)),
                Kind::Quote(q) => walk(q, out),
                Kind::Rule => out.push("---".into()),
                Kind::Table { head, rows, .. } => {
                    out.push(head.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("|"));
                    for r in rows {
                        out.push(r.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join("|"));
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(doc.blocks(), &mut out);
    out
}

fn shown(src: &str) -> Vec<String> {
    let mut d = Doc::new(true);
    d.update(src);
    texts(&d)
}

const DOC: &str = "# Title\n\nHello **world** and [link](https://x.dev/a).\n\n- alpha\n- [x] beta\n  - nested *em*\n\n```rust\nfn main() {}\n```\n\n> quoted\n> text\n\n| a | b |\n|---|:-:|\n| 1 | `2` |\n\n---\n\n1. one\n2. two\n\nLast line.\n";

#[test]
fn blocks_and_styles() {
    let p = parse(DOC, 0);
    let kinds: Vec<&str> = p
        .blocks
        .iter()
        .map(|b| match b.kind {
            Kind::Paragraph(_) => "p",
            Kind::Heading(..) => "h",
            Kind::Code { .. } => "code",
            Kind::List { .. } => "list",
            Kind::Quote(_) => "quote",
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
    let Kind::List { items, start } = &p.blocks[2].kind else { panic!() };
    assert_eq!((*start, items.len(), items[1].task), (None, 2, Some(true)));
    assert!(matches!(items[1].blocks.last().unwrap().kind, Kind::List { .. }), "a nested list");
    let Kind::Code { lang, text } = &p.blocks[3].kind else { panic!() };
    assert_eq!((lang.as_str(), text.as_str()), ("rust", "fn main() {}"));
    let Kind::Table { head, rows, .. } = &p.blocks[5].kind else { panic!() };
    assert_eq!((head.len(), rows.len(), rows[0][1].styles.clone()), (2, 1, vec![(0..1, Style::Code)]));
    let Kind::List { start, .. } = &p.blocks[7].kind else { panic!() };
    assert_eq!(*start, Some(1));
    for (b, at) in p.blocks.iter().zip(&p.starts) {
        assert!(*at <= b.source.start && (*at == 0 || DOC.as_bytes()[at - 1] == b'\n'));
    }
}

#[test]
fn a_growing_source_parses_its_end_as_a_whole_parse_does() {
    // Every prefix, grown char by char, equals its whole parse; most are tail parses.
    for src in [DOC, "Intro\n\npara one\nstill one\n\npara two\n\n- a\n- b\n\n  lazy\n\nend", "a\n\n```\ncode\n\nmore\n```\n\nb\n"] {
        let mut d = Doc::new(false);
        for (i, _) in src.char_indices().chain([(src.len(), ' ')]) {
            d.update(&src[..i]);
            let mut whole = Doc::new(false);
            whole.update(&src[..i]);
            assert_eq!(d.blocks(), whole.blocks(), "prefix {:?}", &src[..i]);
            assert_eq!(d.starts(), whole.starts(), "prefix {:?}", &src[..i]);
        }
        assert!(d.full_parses * 2 < src.len() as u64, "{} whole parses for {} steps", d.full_parses, src.len());
    }
}

#[test]
fn a_reference_definition_parses_whole() {
    let mut d = Doc::new(false);
    d.update("see [x]\n\nmore\n\n");
    let before = d.full_parses;
    d.update("see [x]\n\nmore\n\n[x]: https://x.dev\n");
    assert_eq!(d.full_parses, before + 1);
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
    let mut d = Doc::new(true);
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
    assert_eq!(close_tail("a **b\n\n", Scope::Para), None);
}

#[test]
fn every_prefix_of_a_stream_shows_a_prefix_of_its_text() {
    let src = "Hello **world**, this is _quite_ a `long` answer with [a link](https://x.dev) and ~~old~~ text.";
    let full = shown(src).concat();
    let mut d = Doc::new(true);
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
    for src in [DOC, cjk] {
        let mut d = Doc::new(true);
        for (i, _) in src.char_indices().chain([(src.len(), ' ')]) {
            d.update(&src[..i]);
            let mut fresh = Doc::new(true);
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
    let mut d = Doc::new(true);
    let mut src = String::new();
    for t in &tokens {
        src.push_str(t);
        d.update(&src);
    }
    let per_token = d.parsed_bytes / tokens.len() as u64;
    // Each token parses the block it is in (about one paragraph), not the answer.
    assert!(per_token < 200, "{per_token} bytes parsed per token of a {}-byte answer", answer.len());
    // Whole parses only while the source is its first two blocks (a cut at the start is whole).
    let first_two = answer.match_indices("\n\n").nth(1).unwrap().0 / 3 + 1;
    assert!(d.full_parses as usize <= first_two, "{} whole parses", d.full_parses);
}
