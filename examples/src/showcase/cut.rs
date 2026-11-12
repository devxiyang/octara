//! Cut text: an ellipsis on one line, and a clamp at two.

use super::section;
use octara::prelude::*;

pub fn card() -> El {
    section("Cut text")
        .child(text("A sentence that is far too long to fit in this narrow box, cut with an ellipsis").w(240).truncate())
        .child(
            text("Two lines at most: the rest of this paragraph is hidden behind an ellipsis at the end of the second line.")
                .w(240)
                .line_clamp(2),
        )
}
