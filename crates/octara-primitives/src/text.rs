//! Text editing operations shared by the document and native input hosts.

use serde::{Deserialize, Serialize};
use std::ops::Range;

/// An edit from a platform text input. Ranges count UTF-8 bytes in the displayed document,
/// including marked text. A missing replacement uses the marked range, then the selection.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextEdit {
    /// Replace a displayed range with uncommitted text. `selection` is relative to `text`;
    /// none hides the input method's cursor. Empty text cancels the composition.
    Mark { text: String, selection: Option<Range<usize>>, replacement: Option<Range<usize>> },
    /// Replace a displayed range and accept the resulting document as one committed edit.
    Insert { text: String, replacement: Option<Range<usize>> },
    /// Accept the displayed document, removing its input-method marking.
    Unmark,
    /// Restore the committed document and the selection from before composition began.
    Cancel,
}
