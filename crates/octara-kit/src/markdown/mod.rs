//! Markdown ([text core](../../../../docs/design/text.md#markdown-octara-kit)): a source
//! parsed into blocks, each its own element with a stable key, so a growing source re-parses and
//! re-lays out only its end.

mod parse;
mod render;
mod stream;
#[cfg(test)]
mod tests;

pub use parse::{Align, Block, Inline, Item, Kind, Style};
pub use stream::Doc;

use octara::prelude::*;
use render::{Keyed, keyed, sync};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// What [`markdown`] renders: a text, a closure (followed), or a [`TextBuffer`] (a stream: its end
/// is repaired as it arrives).
pub enum Source {
    Text(String),
    Dyn(Rc<dyn Fn() -> String>),
    Buffer(TextBuffer),
}

pub trait IntoMarkdown {
    fn into_markdown(self) -> Source;
}
impl IntoMarkdown for &str {
    fn into_markdown(self) -> Source {
        Source::Text(self.to_owned())
    }
}
impl IntoMarkdown for String {
    fn into_markdown(self) -> Source {
        Source::Text(self)
    }
}
impl IntoMarkdown for &String {
    fn into_markdown(self) -> Source {
        Source::Text(self.clone())
    }
}
impl IntoMarkdown for TextBuffer {
    fn into_markdown(self) -> Source {
        Source::Buffer(self)
    }
}
impl IntoMarkdown for &TextBuffer {
    fn into_markdown(self) -> Source {
        Source::Buffer(self.clone())
    }
}
impl<F: Fn() -> String + 'static> IntoMarkdown for F {
    fn into_markdown(self) -> Source {
        Source::Dyn(Rc::new(self))
    }
}

/// Markdown as blocks in a selectable document: headings, paragraphs (emphasis, strong, strike,
/// inline code, links), code blocks (a copy button, one text per line), lists (ordered,
/// unordered, tasks), quotes, rules and tables. Top-level blocks are keyed `b{i}`, list items
/// `i{j}`, code lines `l{n}`, table rows `r{k}` and cells `c{j}`. A growing source (a
/// [`TextBuffer`] streaming in) parses again from its last finished block, and the blocks before
/// keep their nodes; a stream's end shows open spans styled and no stray markers, and assistive
/// technology hears each block as it finishes, not each token. Links come to `on_link`.
#[component]
#[track_caller]
pub fn markdown(source: impl IntoMarkdown) -> El {
    let source = source.into_markdown();
    let streaming = matches!(source, Source::Buffer(_));
    let doc = Rc::new(RefCell::new(Doc::new(streaming)));
    untrack(|| match &source {
        Source::Text(s) => doc.borrow_mut().update(s),
        Source::Dyn(f) => doc.borrow_mut().update(&f()),
        Source::Buffer(t) => t.with(|s| doc.borrow_mut().update(s)),
    });
    let source_of = doc.clone();
    // Its blocks live in a scope of its own: what it keeps is made from its source, so each
    // instance has its own, whatever its siblings.
    static INSTANCES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let instance = INSTANCES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    col().selectable().copy_markdown(move |copied| source_copy(&source_of.borrow(), copied)).gap_3().child(component_keyed(
        "markdown",
        instance,
        move || blocks(doc, source, streaming),
    ))
}

/// The blocks of a document, kept in step with its source.
fn blocks(doc: Rc<RefCell<Doc>>, source: Source, streaming: bool) -> View {
    let blocks = state_list("blocks", || keyed("b", doc.borrow().blocks().to_vec()), |e: &Keyed<Block>| e.0.clone());
    // What assistive technology hears: each block as it finishes (the one before the last).
    let heard = state("heard", String::new);
    let finished = Rc::new(Cell::new(doc.borrow().blocks().len().saturating_sub(1)));
    let follow = move |s: &str| {
        let mut d = doc.borrow_mut();
        d.update(s);
        sync(blocks, "b", d.blocks(), d.kept(), render::same_kind);
        let done = d.blocks().len().saturating_sub(1);
        if streaming && done > finished.get() {
            heard.set(d.blocks()[finished.get()..done].iter().map(plain).collect::<Vec<_>>().join("\n"));
        }
        finished.set(done);
    };
    match source {
        Source::Text(_) => {}
        Source::Dyn(f) => watch(
            move || f(),
            move |s, before| {
                if before.is_some() {
                    follow(s);
                }
            },
        ),
        Source::Buffer(t) => {
            let live = t.clone();
            watch(
                move || live.version(),
                move |_, before| {
                    if before.is_some() {
                        t.with(|s| follow(s));
                    }
                },
            );
        }
    }
    let mut parts = vec![each(move || blocks.source(), |_, b| render::block_view(b))];
    if streaming {
        parts.push(el().w(0.0).h(0.0).live(Live::Polite).label(move || heard.get()).into());
    }
    View::Fragment(parts)
}

/// The Markdown for a selection: the source of the blocks it touches, whole; the whole source
/// when it covers every block; `None` (the drawn text) when it is inside one block.
fn source_copy(doc: &Doc, copied: &[Copied]) -> Option<String> {
    let blocks = doc.blocks();
    let block_of = |c: &Copied| c.rows.first().and_then(|k| k.strip_prefix('b')?.parse::<usize>().ok()).filter(|i| *i < blocks.len());
    let mut touched: Vec<(usize, usize, bool)> = Vec::new();
    for c in copied {
        let i = block_of(c)?;
        match touched.last_mut() {
            Some((b, n, all)) if *b == i => {
                *n += 1;
                *all &= c.whole;
            }
            _ => touched.push((i, 1, c.whole)),
        }
    }
    let whole = |&(i, n, all): &(usize, usize, bool)| all && n == texts_in(&blocks[i]);
    let (first, last) = (touched.first()?.0, touched.last()?.0);
    if first == last && !whole(&touched[0]) {
        return None;
    }
    let src = doc.source();
    if first == 0
        && last == blocks.len() - 1
        && touched.len() == blocks.iter().filter(|b| texts_in(b) > 0).count()
        && touched.iter().all(whole)
    {
        return Some(src.to_owned());
    }
    let starts = doc.starts();
    let end = starts.get(last + 1).copied().unwrap_or(src.len());
    Some(src[starts[first]..end].trim_end().to_owned())
}

/// How many selectable texts a block draws (the ones with text).
fn texts_in(b: &Block) -> usize {
    match &b.kind {
        Kind::Paragraph(i) | Kind::Heading(_, i) => usize::from(!i.text.is_empty()),
        Kind::Code { text, .. } => text.split('\n').count(),
        Kind::List { items, .. } => items.iter().flat_map(|i| &i.blocks).map(texts_in).sum(),
        Kind::Quote(q) => q.iter().map(texts_in).sum(),
        Kind::Rule => 0,
        Kind::Table { head, rows, .. } => head.iter().chain(rows.iter().flatten()).filter(|c| !c.text.is_empty()).count(),
    }
}

/// A block's text as read aloud.
fn plain(b: &Block) -> String {
    match &b.kind {
        Kind::Paragraph(i) | Kind::Heading(_, i) => i.text.clone(),
        Kind::Code { text, .. } => text.clone(),
        Kind::List { items, .. } => {
            items.iter().map(|i| i.blocks.iter().map(plain).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n")
        }
        Kind::Quote(q) => q.iter().map(plain).collect::<Vec<_>>().join("\n"),
        Kind::Rule => String::new(),
        Kind::Table { head, rows, .. } => std::iter::once(head)
            .chain(rows)
            .map(|r| r.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(", "))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}
