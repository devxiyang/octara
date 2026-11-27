//! Directional document selections over retained UTF-8 text.

use crate::{NodeFlags, NodeId, Tree};
use octara_model::style::UserSelect;
use std::ops::Range;
use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

/// A text node and a UTF-8 byte offset into its contents.
pub type Address = (NodeId, usize);
/// The anchor and focus of a selection, in that order.
pub type DocSelection = [Address; 2];

/// Unicode word, whitespace, or punctuation segment at a UTF-8 cluster start.
pub fn word_at(text: &str, byte: usize) -> Range<usize> {
    text.split_word_bound_indices()
        .find_map(|(start, word)| (byte < start + word.len()).then_some(start..start + word.len()))
        .unwrap_or(byte..byte)
}

pub fn paragraph_at(text: &str, byte: usize) -> Range<usize> {
    let start = text[..byte].rfind('\n').map_or(0, |index| index + 1);
    let end = text[byte..].find('\n').map_or(text.len(), |index| byte + index);
    start..(end - usize::from(text[start..end].ends_with('\r')))
}

pub fn word_edge(text: &str, byte: usize, backward: bool) -> usize {
    if backward {
        text.unicode_word_indices().take_while(|(start, _)| *start < byte).last().map_or(0, |(start, _)| start)
    } else {
        text.unicode_word_indices().map(|(start, word)| start + word.len()).find(|&end| end > byte).unwrap_or(text.len())
    }
}

pub fn grapheme_edge(text: &str, byte: usize, backward: bool) -> usize {
    let mut cursor = GraphemeCursor::new(byte, text.len(), true);
    if backward {
        cursor.prev_boundary(text, 0).ok().flatten().unwrap_or(0)
    } else {
        cursor.next_boundary(text, 0).ok().flatten().unwrap_or(text.len())
    }
}

impl Tree {
    /// The outermost selectable region containing the node, if selection is allowed there.
    pub fn region_of(&self, node: NodeId) -> Option<NodeId> {
        let mut region = None;
        let mut next = Some(node);
        while let Some(node) = next {
            if self.selection_excluded(node) {
                return None;
            }
            if self.user_select(node) == Some(UserSelect::Text) {
                region = Some(node);
            }
            next = self.logical_parent(node);
        }
        region
    }

    pub fn selectable_in(&self, region: NodeId, text: NodeId) -> bool {
        self.selection_region(region) && self.text(text).is_some() && self.text_in_region(region, text)
    }

    /// Mounted selectable texts in tree order, including the region itself if it is text.
    pub fn selectable_texts(&self, region: NodeId) -> Vec<NodeId> {
        if !self.selection_region(region) {
            return Vec::new();
        }
        let mut texts = Vec::new();
        let mut stack = vec![region];
        while let Some(node) = stack.pop() {
            if self.selection_excluded(node) {
                continue;
            }
            if self.text(node).is_some() {
                texts.push(node);
            } else {
                stack.extend(self.logical_children(node).rev());
            }
        }
        texts
    }

    /// Check that the endpoints are selectable text with valid UTF-8 offsets.
    pub fn valid_doc_selection(&self, region: NodeId, selection: DocSelection) -> bool {
        self.selection_region(region)
            && selection.into_iter().all(|(node, offset)| {
                self.text(node).is_some_and(|slot| slot.text.is_char_boundary(offset)) && self.text_in_region(region, node)
            })
    }

    pub fn selection_endpoints(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.doc_selections.values().flat_map(|selection| selection.iter().map(|&(node, _)| node))
    }

    /// Project a selection in logical text order. Empty slices at node boundaries are preserved.
    pub fn selected_text_ranges(&self, region: NodeId, selection: DocSelection) -> Option<Vec<(NodeId, Range<usize>)>> {
        if !self.valid_doc_selection(region, selection) {
            return None;
        }
        let [(anchor, anchor_offset), (focus, focus_offset)] = selection;
        if anchor == focus {
            return Some(vec![(anchor, anchor_offset.min(focus_offset)..anchor_offset.max(focus_offset))]);
        }
        let mut ranges = Vec::new();
        for node in self.selectable_texts(region) {
            let offset = if node == anchor {
                Some(anchor_offset)
            } else if node == focus {
                Some(focus_offset)
            } else {
                None
            };
            match (ranges.is_empty(), offset) {
                (true, None) => {}
                (true, Some(start)) => ranges.push((node, start..self.text(node)?.text.len())),
                (false, None) => ranges.push((node, 0..self.text(node)?.text.len())),
                (false, Some(end)) => {
                    ranges.push((node, 0..end));
                    return Some(ranges);
                }
            }
        }
        None
    }

    /// Copy the active selection as plain text with one newline between text nodes.
    pub fn selected_text(&self, region: NodeId) -> Option<String> {
        let ranges = self.selected_text_ranges(region, self.doc_selection(region)?)?;
        let mut result = String::new();
        for (index, (node, range)) in ranges.into_iter().enumerate() {
            if index != 0 {
                result.push('\n');
            }
            result.push_str(self.text(node)?.text.get(range)?);
        }
        Some(result)
    }

    fn selection_region(&self, region: NodeId) -> bool {
        self.user_select(region) == Some(UserSelect::Text) && self.region_of(region).is_some()
    }

    fn selection_excluded(&self, node: NodeId) -> bool {
        !self.is_locally_shown(node)
            || self.node(node).is_some_and(|core| core.flags.contains(NodeFlags::HIDDEN))
            || self.user_select(node) == Some(UserSelect::None)
    }

    fn text_in_region(&self, region: NodeId, mut text: NodeId) -> bool {
        while text != region {
            if self.selection_excluded(text) {
                return false;
            }
            let Some(parent) = self.logical_parent(text) else { return false };
            text = parent;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tables::AuthoredStyle;
    use crate::{NodeChange, NodeContent, NodeSpec, RuntimeOp, TreeId, TxBuilder};
    use octara_model::style::{State, StyleProp, When};

    #[test]
    fn reverse_selection_extracts_live_text_in_tree_order() {
        let mut tree = Tree::new(TreeId::new([1, 2], 0), Default::default());
        let mut builder = TxBuilder::new(tree.id(), tree.revision());
        let region = builder.create(NodeSpec::new(NodeContent::Box));
        let first = builder.create(NodeSpec::new(NodeContent::Text("Aé".into())));
        let private = builder.create(NodeSpec::new(NodeContent::Text("hidden".into())));
        let second = builder.create(NodeSpec::new(NodeContent::Text("日本".into())));
        builder.change(region, NodeChange::Flags(NodeFlags::SELECT_REGION));
        builder.change(private, NodeChange::Flags(NodeFlags::SELECT_NONE));
        builder.place(tree.node_ref(tree.root()), 0, region);
        for (index, child) in [first, private, second].into_iter().enumerate() {
            builder.place(region, index, child);
        }
        let (receipt, _) = tree.apply(builder.finish()).unwrap();
        let [region, first, private, second] = [region, first, private, second].map(|node| receipt.node(node).unwrap());

        assert_eq!(tree.region_of(second), Some(region));
        assert_eq!(tree.region_of(private), None);
        assert_eq!(tree.selectable_texts(region), [first, second]);
        let reverse = [(second, 3), (first, 1)];
        tree.apply_runtime(RuntimeOp::SetDocSelection { region, selection: Some(reverse) });
        assert_eq!(tree.doc_selection(region), Some(reverse));
        assert_eq!(tree.selected_text_ranges(region, reverse), Some(vec![(first, 1..3), (second, 0..3)]));
        assert_eq!(tree.selected_text(region).as_deref(), Some("é\n日"));

        assert!(!tree.valid_doc_selection(region, [(second, 3), (first, 2)]));
        assert!(!tree.valid_doc_selection(region, [(second, 3), (private, 2)]));
    }

    #[test]
    fn conditional_user_select_revalidates_retained_selection() {
        let mut tree = Tree::new(TreeId::new([1, 2], 1), Default::default());
        let mut tx = tree.transaction();
        let text = tx.create(NodeSpec::new(NodeContent::Text("Selectable".into())));
        tx.change(
            text,
            NodeChange::Style(Box::new(AuthoredStyle {
                base: [StyleProp::UserSelect(UserSelect::Text)].into_iter().collect(),
                overrides: [(When::State(State::SELECTED), vec![StyleProp::UserSelect(UserSelect::None)])].into_iter().collect(),
                ..Default::default()
            })),
        );
        tx.place(tree.node_ref(tree.root()), 0, text);
        let (receipt, _) = tree.apply(tx.finish()).unwrap();
        let text = receipt.node(text).unwrap();

        assert_eq!(tree.region_of(text), Some(text));
        assert_eq!(tree.selectable_texts(text), [text]);
        tree.apply_runtime(RuntimeOp::SetDocSelection { region: text, selection: Some([(text, 0), (text, 3)]) });
        assert_eq!(tree.selected_text(text).as_deref(), Some("Sel"));

        let delta = tree.apply_runtime(RuntimeOp::SetState { node: text, state: State::SELECTED, on: true });
        assert_eq!(tree.region_of(text), None);
        assert_eq!(tree.doc_selection(text), None);
        assert!(delta.paint_dirty.contains(&text));

        tree.apply_runtime(RuntimeOp::SetState { node: text, state: State::SELECTED, on: false });
        assert_eq!(tree.region_of(text), Some(text));
        assert_eq!(tree.doc_selection(text), None);
    }
}
