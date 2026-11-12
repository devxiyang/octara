//! Blocks into elements: each block its own element under a stable key, so a block that did
//! not change keeps its node, and the block that grows updates its text only.

use super::parse::{Align, Block, Inline, Item, Kind, Style};
use octara::prelude::*;

/// Keyed entries of a list kept in step with a value (`key` from the position).
pub(super) type Keyed<T> = (String, T);

fn key_of<T>(e: &Keyed<T>) -> String {
    e.0.clone()
}

/// The keyed entries for `items`: `{prefix}{index}`.
pub(super) fn keyed<T>(prefix: &str, items: impl IntoIterator<Item = T>) -> Vec<Keyed<T>> {
    items.into_iter().enumerate().map(|(i, t)| (format!("{prefix}{i}"), t)).collect()
}

/// Brings `list` to `items` from position `from` on (earlier entries are the same): an entry
/// whose value changed is updated, one whose kind changed is made again, extra ones are removed.
pub(super) fn sync<T: Clone + PartialEq + 'static>(
    list: List<String, Keyed<T>>,
    prefix: &str,
    items: &[T],
    from: usize,
    same_kind: impl Fn(&T, &T) -> bool,
) {
    let had = list.len();
    for (i, t) in items.iter().enumerate().skip(from.min(had)) {
        let key = format!("{prefix}{i}");
        if i < had {
            let Some(old) = list.item(&key).map(|s| s.get_untracked().1) else { continue };
            if old == *t {
                continue;
            }
            if same_kind(&old, t) {
                list.update(&key, |e| e.1 = t.clone());
            } else {
                list.remove(&key);
                list.insert(i, (key, t.clone()));
            }
        } else {
            list.push((key, t.clone()));
        }
    }
    for i in items.len()..had {
        list.remove(&format!("{prefix}{i}"));
    }
}

pub(super) fn same_kind(a: &Block, b: &Block) -> bool {
    std::mem::discriminant(&a.kind) == std::mem::discriminant(&b.kind)
        && match (&a.kind, &b.kind) {
            (Kind::Heading(x, _), Kind::Heading(y, _)) => x == y,
            (Kind::List { start: x, .. }, Kind::List { start: y, .. }) => x.is_some() == y.is_some(),
            _ => true,
        }
}

/// Blocks kept in step with `get`, each keyed `b{i}`.
pub(super) fn blocks_view(get: impl Fn() -> Vec<Block> + 'static) -> El {
    let blocks = list("blocks", || keyed("b", untrack(&get)), key_of::<Block>);
    watch(get, move |now, before| {
        if before.is_some() {
            sync(blocks, "b", now, 0, same_kind);
        }
    });
    col().gap_3().child(each(move || blocks.source(), |_, b| block_view(b)))
}

/// One block's element: its kind is fixed (a block of another kind is a new entry); what it
/// shows follows its signal.
pub(super) fn block_view(b: ReadSignal<Keyed<Block>>) -> View {
    let kind = b.with_untracked(|(_, b)| b.kind.clone());
    match kind {
        Kind::Paragraph(_) => text(move || b.with(|(_, b)| inline_of(b))).role(Role::Paragraph).into(),
        Kind::Heading(level, _) => {
            let t = text(move || b.with(|(_, b)| inline_of(b))).role(Role::Heading).level(level).font_semibold();
            match level {
                1 => t.text_2xl(),
                2 => t.text_xl(),
                3 => t.text_lg(),
                _ => t,
            }
            .into()
        }
        Kind::Code { .. } => code_view(b),
        Kind::List { start, .. } => list_view(b, start),
        Kind::Quote(_) => row()
            .role(Role::Blockquote)
            .gap_3()
            .child(el().w(3.0).flex_shrink_0().rounded_full().bg(Token::Border).a11y_hidden())
            .child(
                blocks_view(move || {
                    b.with(|(_, b)| match &b.kind {
                        Kind::Quote(q) => q.clone(),
                        _ => Vec::new(),
                    })
                })
                .flex_1()
                .text_color(Token::TextMuted),
            )
            .into(),
        Kind::Rule => el().h(1.0).w_full().bg(Token::Border).a11y_hidden().into(),
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
fn code_view(b: ReadSignal<Keyed<Block>>) -> View {
    let code = move || {
        b.with(|(_, b)| match &b.kind {
            Kind::Code { text, .. } => text.clone(),
            _ => String::new(),
        })
    };
    let lines_of = |s: &str| s.split('\n').map(str::to_owned).collect::<Vec<_>>();
    let lines = list("lines", || keyed("l", lines_of(&untrack(code))), key_of::<String>);
    watch(code, move |now, before| {
        if before.is_some() {
            let now = lines_of(now);
            // A stream changes the last line: earlier ones are kept.
            let from = before.map_or(0, |b| b.matches('\n').count());
            sync(lines, "l", &now, from, |_, _| true);
        }
    });
    // The language follows too: a fence's info string may still be arriving.
    let lang = move || {
        b.with(|(_, b)| match &b.kind {
            Kind::Code { lang, .. } if !lang.is_empty() => lang.clone(),
            _ => "text".to_owned(),
        })
    };
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
            move || lines.source(),
            |_, l| {
                // An empty line keeps its height.
                text(move || l.with(|(_, s)| if s.is_empty() { " ".to_owned() } else { s.clone() })).font_mono().text_sm().into()
            },
        )))
        .into()
}

/// A list: its items (`i{j}`), each a marker (not selected, not read) and the item's blocks.
fn list_view(b: ReadSignal<Keyed<Block>>, start: Option<u64>) -> View {
    let items_of = move || {
        b.with(|(_, b)| match &b.kind {
            Kind::List { items, .. } => items.clone(),
            _ => Vec::new(),
        })
    };
    let items = list("items", || keyed("i", untrack(items_of)), key_of::<Item>);
    watch(items_of, move |now, before| {
        if before.is_some() {
            sync(items, "i", now, 0, |a, b| a.task.is_some() == b.task.is_some());
        }
    });
    let ordered = start.is_some();
    col()
        .role(Role::List)
        .gap_1()
        .child(each(
            move || items.source(),
            move |k, it| {
                let j: u64 = k[1..].parse().unwrap_or(0);
                let marker = match start {
                    Some(n) => format!("{}.", n + j),
                    None => "•".to_owned(),
                };
                let task = it.with_untracked(|(_, i)| i.task);
                row()
                    .role(Role::ListItem)
                    .gap_2()
                    .items_start()
                    // A task item shows its box, not a bullet.
                    .children((task.is_none() || ordered).then(|| {
                        text(marker.clone()).select_none().a11y_hidden().text_color(Token::TextMuted).min_w(if ordered {
                            20.0
                        } else {
                            12.0
                        })
                    }))
                    .children(task.map(|_| {
                        let done = move || it.with(|(_, i)| i.task == Some(true));
                        text(move || if done() { "☑" } else { "☐" }).select_none().role(Role::CheckBox).checked(done)
                    }))
                    .child(blocks_view(move || it.with(|(_, i)| i.blocks.clone())).flex_1().gap_2())
                    .into()
            },
        ))
        .into()
}

/// A table: a head row and body rows (`h`, `r{k}`), cells (`c{j}`) sharing the width equally.
fn table_view(b: ReadSignal<Keyed<Block>>) -> View {
    let rows_of = move || {
        b.with(|(_, b)| match &b.kind {
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
    };
    let rows = list("rows", || keyed("r", untrack(rows_of)), key_of::<Vec<(Inline, Align)>>);
    watch(rows_of, move |now, before| {
        if before.is_some() {
            sync(rows, "r", now, 0, |_, _| true);
        }
    });
    // Rules between rows: the table's border color through a 1 px gap.
    col()
        .role(Role::Table)
        .rounded_md()
        .border()
        .border_color(Token::Border)
        .bg(Token::Border)
        .gap(1.0)
        .overflow_hidden()
        .child(each(
            move || rows.source(),
            |k, r| {
                let head = k == "r0";
                let cells_of = move || r.with(|(_, r)| r.clone());
                let cells = list("cells", || keyed("c", untrack(cells_of)), key_of::<(Inline, Align)>);
                watch(cells_of, move |now, before| {
                    if before.is_some() {
                        sync(cells, "c", now, 0, |_, _| true);
                    }
                });
                let line = row().role(Role::Row);
                let line = if head { line.bg(Token::Surface).font_semibold() } else { line.bg(Token::Background) };
                line.child(each(
                    move || cells.source(),
                    |_, c| {
                        let align = c.with_untracked(|(_, (_, a))| *a);
                        let cell = col().role(Role::Cell).flex_1().flex_basis(0.0).px_3().py_2();
                        let cell = match align {
                            Align::Center => cell.items_center(),
                            Align::Right => cell.items_end(),
                            _ => cell.items_start(),
                        };
                        cell.child(text(move || c.with(|(_, (i, _))| rich_of(i)))).into()
                    },
                ))
                .into()
            },
        ))
        .into()
}
