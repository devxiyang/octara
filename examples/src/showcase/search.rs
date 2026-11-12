//! An async resource: a pretend search answering on the app's clock.

use super::section;
use octara::prelude::*;
use octara_kit::{button, text_field};
use std::time::Duration;

pub fn card() -> El {
    let query = state("query", || "octara".to_string());
    // A pretend search: answers 600 ms later on the app's clock.
    let results = resource(
        move || query.get(),
        |q: String| async move {
            sleep(Duration::from_millis(600)).await;
            if q.trim().is_empty() {
                Err("type something".to_string())
            } else {
                Ok(format!("{} results for “{}”", q.len() * 7, q.trim()))
            }
        },
    );
    section("Async resource")
        .child(text_field(query).id("query").label("Search").w(260).flex_shrink())
        .child(text(move || match (results.loading(), results.value(), results.error()) {
            (true, Some(v), _) => format!("{v} (updating…)"),
            (true, None, _) => "Searching…".to_string(),
            (false, _, Some(e)) => e,
            (false, v, None) => v.unwrap_or_default(),
        }))
        .child(row().gap_2().child(button("Search again").on_press(move || results.refetch())))
}
