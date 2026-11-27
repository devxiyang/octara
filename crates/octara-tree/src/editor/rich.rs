use super::EditorRope;
use octara_model::text::{AtomicRange, InlineAttachment, RichSpan};
use std::{ops::Range, sync::Arc};

/// Authored styling and atomic ranges in the editor's logical UTF-8 text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorRichText {
    pub decorations: Arc<[(Range<usize>, RichSpan)]>,
    pub attachments: Arc<[InlineAttachment]>,
    pub atoms: Arc<[AtomicRange]>,
}

impl EditorRichText {
    pub fn is_empty(&self) -> bool {
        self.decorations.is_empty() && self.attachments.is_empty() && self.atoms.is_empty()
    }

    pub(super) fn validated(mut self, text: &EditorRope) -> Self {
        let valid =
            |range: &Range<usize>| range.start <= range.end && text.is_char_boundary(range.start) && text.is_char_boundary(range.end);
        if self.decorations.iter().any(|(range, _)| range.is_empty() || !valid(range)) {
            self.decorations = self.decorations.iter().filter(|(range, _)| !range.is_empty() && valid(range)).cloned().collect();
        }
        if !self.attachments.is_sorted_by_key(|a| (a.range.start, a.range.end)) {
            Arc::make_mut(&mut self.attachments).sort_by_key(|a| (a.range.start, a.range.end));
        }
        if self.attachments.iter().any(|attachment| !valid(&attachment.range))
            || self.attachments.windows(2).any(|pair| pair[0].range.end > pair[1].range.start)
        {
            self.attachments = Arc::default();
        }
        if !self.atoms.is_empty() {
            let mut atoms: Vec<_> = self.atoms.iter().filter(|a| !a.range.is_empty() && valid(&a.range)).cloned().collect();
            atoms.sort_by_key(|a| (a.range.start, a.range.end));
            let mut previous_end = 0;
            atoms.retain(|atom| {
                if atom.range.start < previous_end {
                    return false;
                }
                previous_end = atom.range.end;
                true
            });
            self.atoms = atoms.into();
        }
        self
    }

    fn atom_at(&self, byte: usize) -> Option<&AtomicRange> {
        let index = self.atoms.partition_point(|a| a.range.end <= byte);
        self.atoms.get(index).filter(|atom| atom.range.start < byte)
    }

    /// Snaps a point inside an atom to its nearest edge. A selection extends to
    /// include any atom it intersects, preserving anchor direction.
    pub(super) fn snap_selection(&self, (anchor, caret): (usize, usize)) -> (usize, usize) {
        match anchor.cmp(&caret) {
            std::cmp::Ordering::Equal => {
                let byte = self.atom_at(caret).map_or(caret, |atom| {
                    if caret - atom.range.start <= atom.range.end - caret { atom.range.start } else { atom.range.end }
                });
                (byte, byte)
            }
            std::cmp::Ordering::Less => {
                let range = self.expand_replacement(anchor..caret);
                (range.start, range.end)
            }
            std::cmp::Ordering::Greater => {
                let range = self.expand_replacement(caret..anchor);
                (range.end, range.start)
            }
        }
    }

    /// An atom immediately before or after a collapsed caret.
    pub fn adjacent_atom(&self, caret: usize, backward: bool) -> Option<&AtomicRange> {
        if backward {
            let index = self.atoms.partition_point(|atom| atom.range.end < caret);
            self.atoms.get(index).filter(|atom| atom.range.end == caret)
        } else {
            let index = self.atoms.partition_point(|atom| atom.range.start < caret);
            self.atoms.get(index).filter(|atom| atom.range.start == caret)
        }
    }

    /// Moves an arrow-key result to the destination edge of an atom.
    pub fn snap_movement(&self, byte: usize, backward: bool) -> usize {
        self.atom_at(byte).map_or(byte, |atom| if backward { atom.range.start } else { atom.range.end })
    }

    /// Includes any partially covered atoms and snaps pure insertions to an edge.
    pub(super) fn expand_replacement(&self, range: Range<usize>) -> Range<usize> {
        if range.is_empty() {
            let (byte, _) = self.snap_selection((range.start, range.start));
            return byte..byte;
        }
        let start = self.atom_at(range.start).map_or(range.start, |a| a.range.start);
        let end = self.atom_at(range.end).map_or(range.end, |a| a.range.end);
        start..end
    }

    pub(super) fn intersects_atom(&self, range: &Range<usize>) -> bool {
        let index = self.atoms.partition_point(|atom| atom.range.end <= range.start);
        self.atoms.get(index).is_some_and(|atom| atom.range.start < range.end)
    }

    /// Remaps ranges through a logical splice. Replacement objects and atoms
    /// touched by the splice are discarded.
    pub(super) fn after_replace(&self, replaced: Range<usize>, inserted: usize) -> Self {
        if self.is_empty() {
            return self.clone();
        }
        let inserted_end = replaced.start + inserted;
        let shift = |byte: usize| inserted_end + (byte - replaced.end);
        let preserve = |range: &Range<usize>| {
            if range.end <= replaced.start {
                Some(range.clone())
            } else if range.start >= replaced.end {
                Some(shift(range.start)..shift(range.end))
            } else {
                None
            }
        };
        let decorations = self
            .decorations
            .iter()
            .filter_map(|(range, style)| {
                let range = preserve(range).or_else(|| {
                    let leading = range.start < replaced.start;
                    let trailing = range.end > replaced.end;
                    match (leading, trailing) {
                        (true, true) => Some(range.start..shift(range.end)),
                        (true, false) => Some(range.start..replaced.start),
                        (false, true) => Some(inserted_end..shift(range.end)),
                        (false, false) => None,
                    }
                })?;
                Some((range, *style))
            })
            .collect();
        let attachments =
            self.attachments.iter().filter_map(|a| Some(InlineAttachment { range: preserve(&a.range)?, ..a.clone() })).collect();
        let atoms = self.atoms.iter().filter_map(|a| Some(AtomicRange { range: preserve(&a.range)?, ..a.clone() })).collect();
        Self { decorations, attachments, atoms }
    }
}
