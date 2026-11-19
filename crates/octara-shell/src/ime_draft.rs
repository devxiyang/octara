use octara::kernel::TextDocument;
use octara_primitives::text::TextEdit;
use std::ops::Range;

pub(crate) struct Draft {
    pub(crate) text: String,
    pub(crate) marked: Option<Range<usize>>,
    pub(crate) selection: Option<Range<usize>>,
}

impl Draft {
    pub(crate) fn edit(&self, document: &TextDocument, cancelled: bool) -> Option<TextEdit> {
        let previous = document.text();
        if let Some(marked) = &self.marked {
            let prefix = self.text.get(..marked.start)?;
            let suffix = self.text.get(marked.end..)?;
            let replaced = previous.strip_prefix(prefix)?.strip_suffix(suffix)?;
            let selection = self
                .selection
                .as_ref()
                .filter(|r| r.start >= marked.start && r.end <= marked.end)
                .map(|r| r.start - marked.start..r.end - marked.start);
            return Some(TextEdit::Mark {
                text: self.text.get(marked.clone())?.to_owned(),
                selection,
                replacement: Some(marked.start..marked.start + replaced.len()),
            });
        }
        if document.marked().is_some() {
            if cancelled {
                return Some(TextEdit::Cancel);
            }
            if previous == self.text {
                return Some(TextEdit::Unmark);
            }
            let mut committed = document.clone();
            committed.apply(&TextEdit::Cancel);
            if committed.text() == self.text {
                return Some(TextEdit::Cancel);
            }
        }
        if previous == self.text {
            return None;
        }
        // The active selection/mark disambiguates equal text on either side of the edit.
        let preferred = document.marked().or_else(|| document.selection());
        if let Some(range) = preferred
            && let Some(text) = self.text.strip_prefix(&previous[..range.start]).and_then(|s| s.strip_suffix(&previous[range.end..]))
            && self.selection.as_ref().is_some_and(|s| s.start == range.start + text.len())
        {
            return Some(TextEdit::Insert { text: text.to_owned(), replacement: Some(range) });
        }
        let start = previous.chars().zip(self.text.chars()).take_while(|(a, b)| a == b).map(|(c, _)| c.len_utf8()).sum();
        let suffix: usize = previous[start..]
            .chars()
            .rev()
            .zip(self.text[start..].chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum();
        Some(TextEdit::Insert {
            text: self.text[start..self.text.len() - suffix].to_owned(),
            replacement: Some(start..previous.len() - suffix),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinyin_replaces_the_selected_emoji_and_commits_once() {
        let mut document = TextDocument::new("A😀Z".into(), 5, 1);
        let draft = Draft { text: "Ani haoZ".into(), marked: Some(1..7), selection: Some(7..7) };
        assert!(document.apply(&draft.edit(&document, false).unwrap()));
        assert_eq!(document.marked(), Some(1..7));
        let commit = Draft { text: "A你好Z".into(), marked: None, selection: Some(7..7) };
        assert!(document.apply(&commit.edit(&document, false).unwrap()));
        assert_eq!(document.text(), "A你好Z");
        assert_eq!(document.marked(), None);
        assert_eq!(commit.edit(&document, false), None);
    }

    #[test]
    fn empty_mark_cancellation_restores_the_replaced_selection() {
        let original = TextDocument::new("A😀Z".into(), 5, 1);
        let mut document = original.clone();
        document.apply(&TextEdit::Mark { text: "ni".into(), selection: None, replacement: None });
        let cleared = Draft { text: "AZ".into(), marked: None, selection: Some(1..1) };
        document.apply(&cleared.edit(&document, true).unwrap());
        assert_eq!(document, original);
    }
}
