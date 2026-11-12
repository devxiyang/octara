//! Document selection ([text core](https://github.com/octoii-works/octara/blob/main/docs/design/text.md)):
//! the texts of a selectable region selected as one document. A selection is data, the
//! window's: an anchor and a focus, each a [`TextPoint`] (a path from the region to a text, and
//! a byte offset in it). Paths step through list rows by key, so a row that unmounts and mounts
//! again finds its part of the selection; ends are ordered by the document (rows by their
//! list's order, then tree order), not by geometry. After each operation the window marks the
//! range each mounted text covers; the kernel draws it.

use super::Window;
use crate::kernel::select::Seg;
use crate::kernel::{Kernel, NodeId};
use std::cmp::Ordering;
use std::rc::Rc;

/// A place in a selectable region's text: the path to the text and a byte offset in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextPoint {
    pub path: Vec<Seg>,
    pub offset: usize,
}

/// A window's document selection.
#[derive(Clone, Debug)]
pub(crate) struct DocSelection {
    pub region: NodeId,
    pub anchor: TextPoint,
    pub focus: TextPoint,
}

/// A row's index by its key.
pub(crate) type IndexOf = Rc<dyn Fn(&str) -> Option<usize>>;
/// Builds a row off screen: its root node, and what disposes it.
pub(crate) type BuildRow = Rc<dyn Fn(&str) -> Option<(NodeId, Box<dyn FnOnce()>)>>;
/// A row built off screen: its path, and its texts' paths and strings.
type Built = (Vec<Seg>, Vec<(Vec<Seg>, String)>);

/// How a list orders its rows (registered by `each` and `virtual_each` on their container), and
/// how it builds a row it has not mounted (a virtual list's).
#[derive(Clone)]
pub(crate) struct ListOrder {
    /// A row's index by its key (as the path writes it).
    pub index_of: IndexOf,
    /// The row keys from `start` to `end` (exclusive).
    pub keys: Rc<dyn Fn(usize, usize) -> Vec<Rc<str>>>,
    /// Builds a row off screen: its root node, until the returned closure disposes it.
    pub build: Option<BuildRow>,
}

impl Window {
    /// The point at `(x, y)` in `region`: on the text there, else the nearest selectable text.
    pub(super) fn point_at(&self, region: NodeId, x: f32, y: f32) -> Option<TextPoint> {
        let k = self.w.kernel.borrow();
        let hit = k.hit_test(x, y).map(|h| if k.link_target(h).is_some() { k.parent(h).unwrap_or(h) } else { h });
        let text = match hit.filter(|h| k.selectable_in(region, *h)) {
            Some(t) => t,
            None => {
                // The nearest by distance to its box, vertical first.
                let mut best: Option<(f32, f32, NodeId)> = None;
                for t in k.selectable_texts(region) {
                    let (tx, ty, tw, th) = k.shown_rect(t);
                    let dy = if y < ty {
                        ty - y
                    } else if y > ty + th {
                        y - ty - th
                    } else {
                        0.0
                    };
                    let dx = if x < tx {
                        tx - x
                    } else if x > tx + tw {
                        x - tx - tw
                    } else {
                        0.0
                    };
                    if best.is_none_or(|b| (dy, dx) < (b.0, b.1)) {
                        best = Some((dy, dx, t));
                    }
                }
                best?.2
            }
        };
        let (tx, ty, _, _) = k.shown_rect(text);
        let ((ox, oy), _, _) = k.text_box(text);
        let lines = k.text_lines(text);
        let offset = crate::kernel::text::offset_at(&lines, x - tx - ox, y - ty - oy);
        Some(TextPoint { path: k.path_to(region, text)?, offset })
    }

    /// The word around `p` (Unicode word boundaries), or its whole text with `block`.
    pub(super) fn unit_at(&self, region: NodeId, p: &TextPoint, block: bool) -> Option<(TextPoint, TextPoint)> {
        let k = self.w.kernel.borrow();
        let text = k.resolve_path(region, &p.path)?;
        let s = k.text_str(text);
        let (start, end) = if block {
            (0, s.len())
        } else {
            use unicode_segmentation::UnicodeSegmentation;
            s.split_word_bound_indices()
                .map(|(i, w)| (i, i + w.len()))
                .find(|(a, b)| p.offset >= *a && p.offset < *b)
                .unwrap_or((p.offset, p.offset))
        };
        Some((TextPoint { path: p.path.clone(), offset: start }, TextPoint { path: p.path.clone(), offset: end }))
    }

    /// Selects from `anchor` to `focus` in `region`.
    pub(super) fn set_doc_selection(&self, region: NodeId, anchor: TextPoint, focus: TextPoint) {
        *self.w.selection.borrow_mut() = Some(DocSelection { region, anchor, focus });
    }

    /// Clears the document selection; whether there was one.
    pub(super) fn clear_doc_selection(&self) -> bool {
        self.w.selection.borrow_mut().take().is_some()
    }

    /// The document selection's region and ends in document order.
    fn ordered(&self) -> Option<(NodeId, TextPoint, TextPoint)> {
        let sel = self.w.selection.borrow().clone()?;
        let k = self.w.kernel.borrow();
        if !k.is_alive(sel.region) {
            return None;
        }
        let lists = self.w.lists.borrow();
        Some(match cmp_points(&k, &lists, sel.region, &sel.anchor, &sel.focus) {
            Ordering::Greater => (sel.region, sel.focus, sel.anchor),
            _ => (sel.region, sel.anchor, sel.focus),
        })
    }

    /// Marks the range each mounted text of the selection's region covers (after each
    /// operation, before the frame is presented).
    pub(super) fn mark_selection(&self) {
        let ordered = self.ordered();
        if ordered.is_none() && self.w.selection.borrow().is_some() {
            self.w.selection.borrow_mut().take();
        }
        let mut now: Vec<NodeId> = Vec::new();
        if let Some((region, start, end)) = &ordered {
            let mut k = self.w.kernel.borrow_mut();
            let lists = self.w.lists.borrow();
            for t in k.selectable_texts(*region) {
                let Some(path) = k.path_to(*region, t) else { continue };
                let len = k.text_str(t).len();
                let range = covered(&k, &lists, *region, &path, len, start, end);
                if range.as_ref().is_some_and(|r| r.start < r.end) {
                    k.set_selected(t, range);
                    now.push(t);
                }
            }
        }
        let before = std::mem::replace(&mut *self.w.selected_texts.borrow_mut(), now.clone());
        let mut k = self.w.kernel.borrow_mut();
        for t in before {
            if !now.contains(&t) && k.is_alive(t) {
                k.set_selected(t, None);
            }
        }
    }

    /// The selected text of the document selection, as copying takes it: texts joined by a
    /// newline, rows of a list by a blank line; rows not mounted are built to read theirs.
    pub fn selected_text(&self) -> Option<String> {
        // Rows built to read them run as this window's code.
        self.enter(|| self.selected_text_here())
    }

    fn selected_text_here(&self) -> Option<String> {
        let (region, start, end) = self.ordered()?;
        let pieces = self.selected_pieces(region, &start, &end);
        let mut out = String::new();
        let mut last_rows: Option<Vec<Rc<str>>> = None;
        for (rows, piece) in pieces {
            if let Some(prev) = &last_rows {
                out.push_str(if *prev == rows { "\n" } else { "\n\n" });
            }
            out.push_str(&piece);
            last_rows = Some(rows);
        }
        (!out.is_empty()).then_some(out)
    }

    /// Each selected text's piece in document order, with the row keys on its path.
    fn selected_pieces(&self, region: NodeId, start: &TextPoint, end: &TextPoint) -> Vec<(Vec<Rc<str>>, String)> {
        let mut out = Vec::new();
        let k = self.w.kernel.borrow();
        let lists = self.w.lists.borrow();
        let rows_of = |path: &[Seg]| -> Vec<Rc<str>> {
            path.iter()
                .filter_map(|s| match s {
                    Seg::Row { key, .. } => Some(key.clone()),
                    Seg::Child(_) => None,
                })
                .collect()
        };
        let mounted: Vec<(NodeId, Vec<Seg>)> =
            k.selectable_texts(region).into_iter().filter_map(|t| k.path_to(region, t).map(|p| (t, p))).collect();
        // Rows of virtual lists between the ends that are not mounted are built to read them.
        let mut missing: Vec<(Vec<Seg>, Rc<str>)> = Vec::new();
        for (container, order) in lists.iter() {
            let (Some(cpath), true) = (k.path_to(region, *container), order.build.is_some()) else { continue };
            let row_index = |p: &TextPoint| match p.path.get(cpath.len()) {
                Some(Seg::Row { key, index }) if p.path.starts_with(&cpath) => Some((order.index_of)(key).unwrap_or(*index as usize)),
                _ => None,
            };
            let from = row_index(start).unwrap_or(0);
            let to = row_index(end).map_or(usize::MAX, |i| i + 1);
            // Only lists the selection runs through.
            let before = cmp_paths(&k, &lists, region, &cpath, &end.path) != Ordering::Greater;
            let after = cmp_paths(&k, &lists, region, &cpath, &start.path) != Ordering::Less || start.path.starts_with(&cpath);
            if !(before && after) {
                continue;
            }
            let keys = (order.keys)(from, to.min(from + MAX_ROWS));
            for (i, key) in keys.into_iter().enumerate() {
                if k.resolve_path(region, &[cpath.clone(), vec![Seg::Row { key: key.clone(), index: (from + i) as u32 }]].concat())
                    .is_none()
                {
                    let mut p = cpath.clone();
                    p.push(Seg::Row { key: key.clone(), index: (from + i) as u32 });
                    missing.push((p, key));
                }
            }
        }
        drop(lists);
        drop(k);
        // Everything in document order: mounted texts and the rows to build, by path.
        let mut built: Vec<Built> = Vec::new();
        for (row_path, key) in missing {
            let container_path = &row_path[..row_path.len() - 1];
            let order = {
                let k = self.w.kernel.borrow();
                let lists = self.w.lists.borrow();
                k.resolve_path(region, container_path).and_then(|c| lists.get(&c).cloned())
            };
            let Some(build) = order.and_then(|o| o.build) else { continue };
            let Some((root, dispose)) = build(&key) else { continue };
            let texts = {
                let k = self.w.kernel.borrow();
                k.selectable_texts(root)
                    .into_iter()
                    .filter_map(|t| {
                        let mut p = row_path.clone();
                        p.extend(k.path_to(root, t)?);
                        Some((p, k.text_str(t).to_owned()))
                    })
                    .collect()
            };
            dispose();
            built.push((row_path, texts));
        }
        let k = self.w.kernel.borrow();
        let lists = self.w.lists.borrow();
        let mut all: Vec<(Vec<Seg>, Option<NodeId>, Option<String>)> = mounted.into_iter().map(|(t, p)| (p, Some(t), None)).collect();
        for (_, texts) in built {
            all.extend(texts.into_iter().map(|(p, s)| (p, None, Some(s))));
        }
        all.sort_by(|a, b| cmp_paths(&k, &lists, region, &a.0, &b.0));
        for (path, node, text) in all {
            let s = match (node, &text) {
                (Some(t), _) => k.text_str(t),
                (None, Some(s)) => s.as_str(),
                _ => continue,
            };
            if let Some(r) = covered(&k, &lists, region, &path, s.len(), start, end)
                && r.start < r.end
            {
                out.push((rows_of(&path), s[r].to_owned()));
            }
        }
        out
    }

    /// Copies the document selection to the clipboard; whether there was one.
    pub(super) fn copy_doc_selection(&self) -> bool {
        let Some(text) = self.selected_text() else { return false };
        self.app.system.borrow_mut().write_text(&text);
        true
    }

    /// Selects everything in `region`: from before its first text to after its last, rows of
    /// its lists that are not mounted included.
    pub(super) fn select_region(&self, region: NodeId) -> bool {
        if self.w.kernel.borrow().selectable_texts(region).is_empty() {
            return false;
        }
        self.set_doc_selection(
            region,
            TextPoint { path: Vec::new(), offset: 0 },
            TextPoint { path: vec![Seg::Child(u32::MAX)], offset: usize::MAX },
        );
        true
    }

    /// The commands a document selection handles when no field has focus.
    pub(super) fn document_command(&self, command: &str) -> bool {
        match command {
            "edit.copy" => self.copy_doc_selection(),
            "edit.select_all" => {
                let region = self.w.selection.borrow().as_ref().map(|s| s.region).or_else(|| {
                    let at = self.w.interaction.borrow().pointer.hover_at;
                    let k = self.w.kernel.borrow();
                    at.and_then(|(x, y)| k.hit_test(x, y)).and_then(|h| k.region_of(h))
                });
                region.is_some_and(|r| self.select_region(r))
            }
            _ => false,
        }
    }

    /// Selects from `from` to `to` (test ids of texts and byte offsets), as a drag would.
    pub fn select(&mut self, from: (&str, usize), to: (&str, usize)) -> Result<(), String> {
        let mut result = Ok(());
        self.input_frame(|| {
            let k = self.w.kernel.borrow();
            let find = |id: &str| k.find(id).ok_or(format!("no element has test id `{id}`"));
            let (a, b) = match (find(from.0), find(to.0)) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(e), _) | (_, Err(e)) => {
                    result = Err(e);
                    return;
                }
            };
            let Some(region) = k.region_of(a).filter(|r| k.region_of(b) == Some(*r)) else {
                result = Err(format!("`{}` and `{}` are not in one selectable region", from.0, to.0));
                return;
            };
            let point = |t: NodeId, offset: usize| TextPoint {
                path: k.path_to(region, t).unwrap_or_default(),
                offset: offset.min(k.text_str(t).len()),
            };
            let (pa, pb) = (point(a, from.1), point(b, to.1));
            drop(k);
            self.set_doc_selection(region, pa, pb);
        });
        result
    }

    /// Selects everything in the selectable region with test id `region`.
    pub fn select_all(&mut self, region: &str) -> Result<(), String> {
        let n = self.w.kernel.borrow().find(region).ok_or(format!("no element has test id `{region}`"))?;
        let mut done = false;
        self.input_frame(|| done = self.select_region(n));
        if done { Ok(()) } else { Err(format!("`{region}` has no selectable text")) }
    }

    /// Clears the document selection.
    pub fn clear_selection(&mut self) {
        self.input_frame(|| {
            self.clear_doc_selection();
        });
    }
}

/// Rows a copy builds at most.
const MAX_ROWS: usize = 10_000;

/// The child a path step leads to under `parent`, when mounted.
fn step(k: &Kernel, parent: NodeId, seg: &Seg) -> Option<NodeId> {
    let children = k.children(parent);
    match seg {
        Seg::Child(i) => children.get(*i as usize).copied(),
        Seg::Row { key, .. } => children.iter().copied().find(|c| k.row_key(*c) == Some(key)),
    }
}

/// Where a step stands among its siblings: a list row by its list's order, else by index.
fn rank(k: &Kernel, lists: &std::collections::HashMap<NodeId, ListOrder>, parent: Option<NodeId>, seg: &Seg) -> (u64, u64) {
    match seg {
        Seg::Row { key, index } => {
            let order = parent.and_then(|p| lists.get(&p));
            // The list's order, else where the row is now among its siblings' rows, else where
            // it was.
            let i = order.and_then(|o| (o.index_of)(key)).unwrap_or_else(|| {
                parent
                    .and_then(|p| {
                        let rows = k.children(p).iter().filter(|c| k.row_key(**c).is_some());
                        rows.clone().position(|c| k.row_key(*c) == Some(key))
                    })
                    .unwrap_or(*index as usize)
            });
            // A list's rows sit after the siblings before them; rows compare among themselves.
            let base = parent.and_then(|p| k.children(p).iter().position(|c| k.row_key(*c).is_some())).unwrap_or(0) as u64;
            (base, i as u64)
        }
        Seg::Child(i) => (*i as u64, 0),
    }
}

/// Two paths in document order.
pub(crate) fn cmp_paths(
    k: &Kernel,
    lists: &std::collections::HashMap<NodeId, ListOrder>,
    region: NodeId,
    a: &[Seg],
    b: &[Seg],
) -> Ordering {
    let mut node = Some(region);
    for (sa, sb) in a.iter().zip(b) {
        if sa == sb {
            node = node.and_then(|n| step(k, n, sa));
            continue;
        }
        if let (Seg::Row { key: ka, .. }, Seg::Row { key: kb, .. }) = (sa, sb)
            && ka == kb
        {
            node = node.and_then(|n| step(k, n, sa));
            continue;
        }
        return rank(k, lists, node, sa).cmp(&rank(k, lists, node, sb));
    }
    a.len().cmp(&b.len())
}

fn cmp_points(k: &Kernel, lists: &std::collections::HashMap<NodeId, ListOrder>, region: NodeId, a: &TextPoint, b: &TextPoint) -> Ordering {
    match cmp_paths(k, lists, region, &a.path, &b.path) {
        Ordering::Equal => a.offset.cmp(&b.offset),
        o => o,
    }
}

/// The range of a text at `path` (of `len` bytes) the selection from `start` to `end` covers.
fn covered(
    k: &Kernel,
    lists: &std::collections::HashMap<NodeId, ListOrder>,
    region: NodeId,
    path: &[Seg],
    len: usize,
    start: &TextPoint,
    end: &TextPoint,
) -> Option<std::ops::Range<usize>> {
    let same = |p: &TextPoint| cmp_paths(k, lists, region, path, &p.path) == Ordering::Equal;
    let after_start = cmp_paths(k, lists, region, path, &start.path) != Ordering::Less;
    let before_end = cmp_paths(k, lists, region, path, &end.path) != Ordering::Greater;
    if !(after_start && before_end) {
        return None;
    }
    let s = if same(start) { start.offset.min(len) } else { 0 };
    let e = if same(end) { end.offset.min(len) } else { len };
    Some(s..e)
}
