//! Blocks into elements: each block its own element under a stable key, so a block that did
//! not change keeps its node, and the block that grows updates its text only.

use super::parse::{Align, Block, Inline, Kind, Style};
use octara::prelude::*;
use octara::reactive::untrack;
use std::cell::RefCell;
use std::fmt;
use std::mem::{Discriminant, discriminant};
use std::ops::Deref;
use std::rc::Rc;

/// Position is the display label; structure also participates in row identity so a new kind
/// gets a new element. Content changes keep the same row.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct BlockKey {
    index: usize,
    kind: Discriminant<Kind>,
    detail: u8,
}

impl fmt::Display for BlockKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "b{}", self.index)
    }
}

/// Reference identity lets a row's memo cut off an unchanged prefix without comparing or
/// cloning its blocks again when a snapshot changes elsewhere.
#[derive(Clone)]
pub(super) struct BlockRef(Rc<Block>);

impl PartialEq for BlockRef {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Deref for BlockRef {
    type Target = Block;

    fn deref(&self) -> &Block {
        &self.0
    }
}

#[derive(Default)]
pub(super) struct BlockCache(Vec<BlockRef>);

impl BlockCache {
    pub(super) fn update(&mut self, blocks: &[Block], kept: usize) -> Vec<(BlockKey, BlockRef)> {
        for (i, block) in blocks.iter().enumerate().skip(kept.min(self.0.len())) {
            if self.0.get(i).is_some_and(|old| **old == *block) {
                continue;
            }
            let block = BlockRef(Rc::new(block.clone()));
            match self.0.get_mut(i) {
                Some(old) => *old = block,
                None => self.0.push(block),
            }
        }
        self.0.truncate(blocks.len());
        self.0
            .iter()
            .enumerate()
            .map(|(index, block)| {
                let detail = match block.kind {
                    Kind::Heading(level, _) => level,
                    Kind::List { start, .. } => u8::from(start.is_some()),
                    _ => 0,
                };
                (BlockKey { index, kind: discriminant(&block.kind), detail }, block.clone())
            })
            .collect()
    }
}

/// The keyed entries for `items`: `{prefix}{index}`.
fn keyed<T>(prefix: &str, items: impl IntoIterator<Item = T>) -> Vec<(String, T)> {
    items.into_iter().enumerate().map(|(i, t)| (format!("{prefix}{i}"), t)).collect()
}

/// Blocks kept in step with `get`, each keyed `b{i}`.
pub(super) fn blocks_view(get: impl Fn() -> Vec<Block> + 'static) -> El {
    let cache = RefCell::new(BlockCache::default());
    let blocks = memo(move || {
        let blocks = get();
        cache.borrow_mut().update(&blocks, 0)
    });
    col().gap_3().child(each(move || blocks.get(), |_, b| block_view(b)))
}

/// One block's element: its kind is fixed (a block of another kind is a new entry); what it
/// shows follows its signal.
pub(super) fn block_view(b: ReadSignal<BlockRef>) -> El {
    let b = memo(move || b.get());
    let kind = untrack(|| b.with(|b| b.kind.clone()));
    match kind {
        Kind::Paragraph(_) => text(move || b.with(|b| inline_of(b))).role(Role::Paragraph),
        Kind::Heading(level, _) => {
            let t = text(move || b.with(|b| inline_of(b))).role(Role::Heading).level(level).font_semibold();
            match level {
                1 => t.text_2xl(),
                2 => t.text_xl(),
                3 => t.text_lg(),
                _ => t,
            }
        }
        Kind::Code { .. } => code_view(b),
        Kind::List { start, .. } => list_view(b, start),
        Kind::Quote(_) => {
            row().role(Role::Blockquote).gap_3().child(el().w(3.0).flex_shrink_0().rounded_full().bg(Token::Border).a11y_hidden()).child(
                blocks_view(move || {
                    b.with(|b| match &b.kind {
                        Kind::Quote(q) => q.clone(),
                        _ => Vec::new(),
                    })
                })
                .flex_1()
                .text_color(Token::TextMuted),
            )
        }
        Kind::Rule => el().h(1.0).w_full().bg(Token::Border).a11y_hidden(),
        Kind::Table { .. } => table_view(b),
    }
}

fn inline_of(b: &Block) -> Rich {
    match &b.kind {
        Kind::Paragraph(i) | Kind::Heading(_, i) => rich_of(i),
        _ => Rich::default(),
    }
}

/// Inline styles as runs: where styles nest or overlap, the pieces between their edges take
/// every style over them.
pub(super) fn rich_of(i: &Inline) -> Rich {
    let mut edges: Vec<usize> = vec![0, i.text.len()];
    for (r, _) in &i.styles {
        edges.extend([r.start, r.end]);
    }
    edges.sort_unstable();
    edges.dedup();
    let mut out = rich();
    for w in edges.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b || !i.text.is_char_boundary(a) || !i.text.is_char_boundary(b) {
            continue;
        }
        let over: Vec<&Style> = i.styles.iter().filter(|(r, _)| r.start <= a && b <= r.end).map(|(_, s)| s).collect();
        if over.is_empty() {
            out = out.text(&i.text[a..b]);
            continue;
        }
        let mut span = Span::default();
        for s in over {
            span = match s {
                Style::Strong => span.bold(),
                Style::Emphasis => span.italic(),
                Style::Strike => span.strike(),
                Style::Code => span.mono().background(Token::SurfacePressed),
                Style::Link(url) => span.link(url),
            };
        }
        out = out.span(&i.text[a..b], span);
    }
    out
}

/// A code block: its language and a copy button over its lines, one text per line (`l{n}`), so a
/// stream into it re-lays out its last line.
fn code_view(b: ReadSignal<BlockRef>) -> El {
    let code = move || {
        b.with(|b| match &b.kind {
            Kind::Code { text, .. } => text.clone(),
            _ => String::new(),
        })
    };
    let cache = RefCell::new(Lines::default());
    let lines = memo(move || {
        b.with(|b| match &b.kind {
            Kind::Code { text, .. } => cache.borrow_mut().update(text),
            _ => Vec::new(),
        })
    });
    // The language follows too: a fence's info string may still be arriving.
    let lang = memo(move || {
        b.with(|b| match &b.kind {
            Kind::Code { lang, .. } if !lang.is_empty() => lang.clone(),
            _ => "text".to_owned(),
        })
    });
    col()
        .role(Role::Code)
        .rounded_md()
        .border()
        .border_color(Token::Border)
        .bg(Token::Surface)
        .child(
            row()
                .select_none()
                .items_center()
                .px_3()
                .py_1()
                .child(text(lang).text_xs().text_color(Token::TextMuted))
                .child(spacer())
                .child(crate::ghost_button("Copy").text_xs().on_press(move || write_to_clipboard(code()))),
        )
        .child(el().h(1.0).bg(Token::Border).a11y_hidden())
        .child(col().p_3().overflow_x_scroll().child(each(
            move || lines.get(),
            |_, l| {
                let l = memo(move || l.get());
                // An empty line keeps its height.
                text(move || l.with(|s| if s.is_empty() { " ".to_owned() } else { s.to_string() })).font_mono().text_sm()
            },
        )))
}

/// Only a proven append retains earlier line allocations. A rewrite can change any line.
#[derive(Default)]
struct Lines {
    source: String,
    lines: Vec<Rc<str>>,
}

impl Lines {
    fn update(&mut self, source: &str) -> Vec<(String, Rc<str>)> {
        let appended = source.starts_with(&self.source);
        let (from, cut) =
            if appended { (self.lines.len().saturating_sub(1), self.source.rfind('\n').map_or(0, |at| at + 1)) } else { (0, 0) };
        let mut end = from;
        for line in source[cut..].split('\n') {
            match self.lines.get_mut(end) {
                Some(old) if **old == *line => {}
                Some(old) => *old = Rc::from(line),
                None => self.lines.push(Rc::from(line)),
            }
            end += 1;
        }
        self.lines.truncate(end);
        if appended {
            self.source.push_str(&source[self.source.len()..]);
        } else {
            self.source.clear();
            self.source.push_str(source);
        }
        keyed("l", self.lines.clone())
    }
}

/// A list: its items (`i{j}`), each a marker (not selected, not read) and the item's blocks.
fn list_view(b: ReadSignal<BlockRef>, start: Option<u64>) -> El {
    let items = memo(move || {
        b.with(|b| match &b.kind {
            Kind::List { items, .. } => items.clone(),
            _ => Vec::new(),
        })
    });
    let ordered = start.is_some();
    let start = memo(move || b.with(|b| if let Kind::List { start, .. } = b.kind { start } else { None }));
    col().role(Role::List).gap_1().child(each(
        move || items.with(|items| keyed("i", items.clone())),
        move |k, it| {
            let it = memo(move || it.get());
            let j: u64 = k[1..].parse().unwrap_or(0);
            let marker = move || match start.get() {
                Some(n) => format!("{}.", n + j),
                None => "•".to_owned(),
            };
            row()
                .role(Role::ListItem)
                .gap_2()
                .items_start()
                // A task item shows its box, not a bullet.
                .child(show(
                    move || ordered || it.with(|i| i.task.is_none()),
                    move || text(marker).select_none().a11y_hidden().text_color(Token::TextMuted).min_w(if ordered { 20.0 } else { 12.0 }),
                ))
                .child(show(
                    move || it.with(|i| i.task.is_some()),
                    move || {
                        let done = move || it.with(|i| i.task == Some(true));
                        text(move || if done() { "☑" } else { "☐" }).select_none().role(Role::CheckBox).checked(done)
                    },
                ))
                .child(blocks_view(move || it.with(|i| i.blocks.clone())).flex_1().gap_2())
        },
    ))
}

/// A table: a head row and body rows (`h`, `r{k}`), cells (`c{j}`) sharing the width equally.
fn table_view(b: ReadSignal<BlockRef>) -> El {
    let rows = memo(move || {
        b.with(|b| match &b.kind {
            Kind::Table { head, rows, align } => {
                let width = head.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
                std::iter::once(head)
                    .chain(rows)
                    .map(|r| {
                        let mut cells: Vec<(Inline, Align)> =
                            r.iter().enumerate().map(|(j, c)| (c.clone(), align.get(j).copied().unwrap_or(Align::None))).collect();
                        cells.resize(width, (Inline::default(), Align::None));
                        cells
                    })
                    .collect::<Vec<_>>()
            }
            _ => Vec::new(),
        })
    });
    // Rules between rows: the table's border color through a 1 px gap.
    col().role(Role::Table).rounded_md().border().border_color(Token::Border).bg(Token::Border).gap(1.0).overflow_hidden().child(each(
        move || rows.with(|rows| keyed("r", rows.clone())),
        |k, r| {
            let r = memo(move || r.get());
            let head = k == "r0";
            let line = row().role(Role::Row);
            let line = if head { line.bg(Token::Surface).font_semibold() } else { line.bg(Token::Background) };
            line.child(each(
                move || r.with(|r| keyed("c", r.clone())),
                |_, c| {
                    let c = memo(move || c.get());
                    col()
                        .role(Role::Cell)
                        .flex_1()
                        .flex_basis(0.0)
                        .px_3()
                        .py_2()
                        .style_with(move |p| {
                            c.with(|(_, align)| match align {
                                Align::Center => p.items_center(),
                                Align::Right => p.items_end(),
                                _ => p.items_start(),
                            })
                        })
                        .child(text(move || c.with(|(i, _)| rich_of(i))))
                },
            ))
        },
    ))
}
