//! Group opacity: overlapping children fade as one layer.

use super::section;
use octara::prelude::*;
use octara_kit::switch;

#[component]
pub fn card() -> El {
    let faded = state("faded", || true);
    section("Group opacity")
        .child(
            row().gap_6().items_center().child(
                el().w(150)
                    .h(90)
                    .opacity(move || if faded.get() { 0.5 } else { 1.0 })
                    .child(el().absolute().top_0().left_0().size_16().rounded_lg().bg(rgb(0xef4444)))
                    .child(el().absolute().top(26.0).left(40.0).size_16().rounded_full().bg(rgb(0x3b82f6)))
                    .child(text("Layer").absolute().bottom_0().right_0().font_semibold()),
            ),
        )
        .child(row().gap_2().items_center().child(switch(faded).label("Half opacity")).child(text("Half opacity").text_sm()))
}
