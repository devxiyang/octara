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
            // A candidate can reproduce the original text while advancing the caret.
            if committed.text() == self.text && (self.selection.is_none() || self.selection == committed.selection()) {
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

    #[test]
    fn multiline_candidate_preserves_the_surrounding_breaks() {
        let original = TextDocument::new("A\n😀\r\nZ".into(), 6, 2);
        let mut document = original.clone();
        let draft = Draft { text: "A\nni hao\r\nZ".into(), marked: Some(2..8), selection: Some(5..8) };
        let edit = draft.edit(&document, false).unwrap();
        assert_eq!(edit, TextEdit::Mark { text: "ni hao".into(), selection: Some(3..6), replacement: Some(2..6) });
        assert!(document.apply(&edit));
        assert_eq!(document.text(), draft.text);
        assert_eq!(document.marked(), draft.marked);
        assert_eq!(document.selection(), draft.selection);

        let cleared = Draft { text: "A\n\r\nZ".into(), marked: None, selection: Some(2..2) };
        let mut cancelled = document.clone();
        assert!(cancelled.apply(&cleared.edit(&cancelled, true).unwrap()));
        assert_eq!(cancelled, original);

        // The snapshot can include a later edit made after confirming the candidate.
        let commit = Draft { text: "A\n你好\r\nZ!".into(), marked: None, selection: Some(12..12) };
        assert!(document.apply(&commit.edit(&document, false).unwrap()));
        assert_eq!(document, TextDocument::new(commit.text.clone(), 12, 12));
        assert_eq!(commit.edit(&document, false), None);
    }

    #[test]
    fn inserting_a_repeated_break_keeps_the_native_caret_for_the_next_edit() {
        let mut document = TextDocument::new("😀\r\nline\nend".into(), 10, 10);
        let split = Draft { text: "😀\r\nline\n\nend".into(), marked: None, selection: Some(11..11) };
        let edit = split.edit(&document, false).unwrap();
        assert_eq!(edit, TextEdit::Insert { text: "\n".into(), replacement: Some(10..10) });
        assert!(document.apply(&edit));
        assert_eq!(document.selection(), split.selection);

        let pasted = Draft { text: "😀\r\nline\n語\r\n🙂\n\nend".into(), marked: None, selection: Some(21..21) };
        assert!(document.apply(&pasted.edit(&document, false).unwrap()));
        assert_eq!(document, TextDocument::new(pasted.text, 21, 21));
    }

    #[test]
    fn committing_the_original_text_can_collapse_the_selection() {
        let mut document = TextDocument::new("A\n你\r\nZ".into(), 5, 2);
        let draft = Draft { text: "A\nni\r\nZ".into(), marked: Some(2..4), selection: Some(4..4) };
        assert!(document.apply(&draft.edit(&document, false).unwrap()));

        let restored = Draft { text: "A\n你\r\nZ".into(), marked: None, selection: Some(2..5) };
        assert_eq!(restored.edit(&document, false), Some(TextEdit::Cancel));

        let commit = Draft { selection: Some(5..5), ..restored };
        assert!(document.apply(&commit.edit(&document, false).unwrap()));
        assert_eq!(document, TextDocument::new(commit.text, 5, 5));
    }
}
