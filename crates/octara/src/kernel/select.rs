//! Document selection in the kernel ([text core](../../../../docs/design/text.md)): selectable
//! regions and the texts in them, paths that find a text again after its row was rebuilt, and
//! the ranges a selection covers, drawn behind the glyphs.

use super::{Content, Kernel, Kind, NodeId};
use std::ops::Range;
use std::rc::Rc;

/// One step from a selectable region down to a text: a child by its index, or a list row by its
/// key (with its index then, for a row that is gone).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Seg {
    Child(u32),
    Row { key: Rc<str>, index: u32 },
}

impl Kernel {
    pub fn set_region(&mut self, id: NodeId, on: bool) {
        self.node_mut(id).region = on;
    }
    pub fn is_region(&self, id: NodeId) -> bool {
        self.node(id).region
    }
    pub fn set_select_none(&mut self, id: NodeId, on: bool) {
        self.node_mut(id).select_none = on;
    }
    pub fn set_row_key(&mut self, id: NodeId, key: Option<Rc<str>>) {
        self.node_mut(id).row_key = key;
    }
    pub fn row_key(&self, id: NodeId) -> Option<&Rc<str>> {
        self.node(id).row_key.as_ref()
    }

    /// The selectable region `id` is in, if any: the outermost, as regions inside a region are
    /// one document with it (a selectable transcript of selectable messages selects across them;
    /// each message still copies its part as Markdown its own way).
    pub fn region_of(&self, id: NodeId) -> Option<NodeId> {
        let mut out = None;
        let mut n = Some(id);
        while let Some(x) = n {
            if self.node(x).region {
                out = Some(x);
            }
            n = self.node(x).parent;
        }
        out
    }

    /// Whether `text` can be selected in `region`: a shown text in it, nothing between marked
    /// `select_none`.
    pub fn selectable_in(&self, region: NodeId, text: NodeId) -> bool {
        if self.kind(text) != Kind::Text || !self.is_alive(text) {
            return false;
        }
        let mut n = Some(text);
        while let Some(x) = n {
            let node = self.node(x);
            if node.select_none || node.hidden || node.layout.display == taffy::Display::None {
                return false;
            }
            if x == region {
                return true;
            }
            n = node.parent;
        }
        false
    }

    /// The selectable texts of `region` in tree order (what is mounted).
    pub fn selectable_texts(&self, region: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![region];
        while let Some(id) = stack.pop() {
            let n = self.node(id);
            if n.select_none || n.hidden {
                continue;
            }
            if n.kind() == Kind::Text {
                out.push(id);
                continue;
            }
            stack.extend(n.children.iter().rev().copied().filter(|c| self.node(*c).kind() != Kind::Link));
        }
        out
    }

    /// The path from `region` down to `node`.
    pub fn path_to(&self, region: NodeId, node: NodeId) -> Option<Vec<Seg>> {
        let mut segs = Vec::new();
        let mut n = node;
        while n != region {
            let parent = self.node(n).parent?;
            let index = self.node(parent).children.iter().position(|c| *c == n)? as u32;
            segs.push(match &self.node(n).row_key {
                Some(key) => Seg::Row { key: key.clone(), index },
                None => Seg::Child(index),
            });
            n = parent;
        }
        segs.reverse();
        Some(segs)
    }

    /// The node at `path` under `region`, if it is mounted.
    pub fn resolve_path(&self, region: NodeId, path: &[Seg]) -> Option<NodeId> {
        let mut n = region;
        for seg in path {
            let children = &self.node(n).children;
            n = match seg {
                Seg::Child(i) => *children.get(*i as usize)?,
                Seg::Row { key, .. } => *children.iter().find(|c| self.node(**c).row_key.as_ref() == Some(key))?,
            };
        }
        Some(n)
    }

    /// Sets the range of `text` a selection covers (`None`: none); drawn behind its glyphs.
    pub fn set_selected(&mut self, text: NodeId, range: Option<Range<usize>>) -> bool {
        if self.node(text).selected == range {
            return false;
        }
        self.node_mut(text).selected = range;
        self.changed(text, super::Dirty::PAINT, "selected", String::new);
        true
    }
    /// Sets a region's selection as assistive technology gets it: anchor and focus in mounted
    /// texts (`None`: none, or neither end mounted).
    pub fn set_doc_selection(&mut self, region: NodeId, ends: Option<[(NodeId, usize); 2]>) {
        if self.node(region).doc_selection != ends {
            self.node_mut(region).doc_selection = ends;
            self.a11y_touch(region);
        }
    }
    pub fn doc_selection(&self, region: NodeId) -> Option<[(NodeId, usize); 2]> {
        self.node(region).doc_selection
    }

    pub fn selected(&self, text: NodeId) -> Option<Range<usize>> {
        self.node(text).selected.clone()
    }

    /// The selection's rects in `text`, in its node's coordinates.
    pub(crate) fn selected_rects(&self, text: NodeId) -> Vec<crate::paint::Rect> {
        let Some(range) = self.node(text).selected.clone() else { return Vec::new() };
        let ((ox, oy), _, _) = self.text_box(text);
        let lines = self.text_lines(text);
        let mut rects = super::text::range_rects(&lines, range.clone());
        // A selection reaching past a line's end marks its break, as editors do.
        for l in lines.iter() {
            if range.start <= l.end && range.end > l.end && l.end < self.node(text).text().len() {
                let x = l.x_of(l.end);
                rects.push(crate::paint::Rect { x, y: l.top, w: (self.node(text).visual.font_size * 0.3).max(4.0), h: l.height });
            }
        }
        rects.into_iter().map(|r| crate::paint::Rect { x: r.x + ox, y: r.y + oy, ..r }).collect()
    }

    /// The text of `id` (a text node's own).
    pub fn text_str(&self, id: NodeId) -> &str {
        match &self.node(id).content {
            Content::Text(t) => &t.text,
            _ => "",
        }
    }
}

/// A selected text as a copy as Markdown gets it: the keys of the list rows between the element
/// that copies and the text, the selected part of the text, and whether that is all of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Copied {
    pub rows: Vec<Rc<str>>,
    pub text: String,
    pub whole: bool,
}

/// What an element copies as Markdown for the selected texts in it (`None`: their text).
pub type CopyMarkdown = Rc<dyn Fn(&[Copied]) -> Option<String>>;

impl Kernel {
    pub fn set_copy_markdown(&mut self, id: NodeId, f: Option<CopyMarkdown>) {
        self.node_mut(id).copy_markdown = f;
    }
    pub fn copy_markdown(&self, id: NodeId) -> Option<CopyMarkdown> {
        self.node(id).copy_markdown.clone()
    }

    /// The roles from below `top` down to `node`, one per step of the path between them.
    pub fn roles_down(&self, top: NodeId, node: NodeId) -> Vec<super::Role> {
        let mut out = Vec::new();
        let mut n = node;
        while n != top {
            out.push(self.role(n));
            let Some(p) = self.node(n).parent else { break };
            n = p;
        }
        out.reverse();
        out
    }

    /// The nearest element from `node` up to `top` (both included) that copies as Markdown,
    /// and how many steps below `top` it is.
    pub fn copier_above(&self, top: NodeId, node: NodeId) -> Option<(usize, CopyMarkdown)> {
        let mut n = node;
        let mut depth = self.path_to(top, node)?.len();
        loop {
            if let Some(f) = &self.node(n).copy_markdown {
                return Some((depth, f.clone()));
            }
            if n == top {
                return None;
            }
            n = self.node(n).parent?;
            depth -= 1;
        }
    }
}

/// Where a node is, as a rebuild finds it again: the nearest element at or above it with a test
/// id (or the root), and the child indices from there down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub id: Option<String>,
    pub path: Vec<u32>,
}

impl Kernel {
    /// `n`'s address.
    pub fn address(&self, n: NodeId) -> Address {
        let mut path = Vec::new();
        let mut at = n;
        loop {
            if let Some(id) = self.test_id(at) {
                path.reverse();
                return Address { id: Some(id.to_owned()), path };
            }
            let Some(parent) = self.node(at).parent else { break };
            path.push(self.node(parent).children.iter().position(|c| *c == at).unwrap_or(0) as u32);
            at = parent;
        }
        path.reverse();
        Address { id: None, path }
    }

    /// The node at `address`, if it is there.
    pub fn find_address(&self, a: &Address) -> Option<NodeId> {
        let mut n = match &a.id {
            Some(id) => self.find(id)?,
            None => self.root,
        };
        for i in &a.path {
            n = *self.node(n).children.get(*i as usize)?;
        }
        Some(n)
    }
}
