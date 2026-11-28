//! Check rich-text layout and reference pixels without a GPU device.

#[path = "support/rich_text.rs"]
mod fixture;

#[test]
fn rich_span_pill_renders_on_cpu() {
    fixture::rich_span_pill(|_, _, _| {});
}

#[test]
fn inline_pill_renders_on_cpu() {
    fixture::inline_pill(|_, _, _| {});
}

#[test]
fn marked_clauses_render_on_cpu() {
    fixture::marked_clauses(|_, _, _| {});
}
