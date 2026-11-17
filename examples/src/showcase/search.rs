//! An async derivation: a pretend search answering on the app's clock.

use super::section;
use octara::prelude::*;
use octara_kit::{button, text_field};
use std::time::Duration;

#[ui]
pub fn card() -> El {
    let query = state("query", || "octara".to_string());
    // A pretend search: answers 600 ms later on the app's clock.
    let results = async_memo(
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
    section("Async search")
        .child(text_field(query).id("query").label("Search").w(260).flex_shrink())
        .child(text(move || {
            results.with(|state| match state {
                AsyncState::Pending { previous: Some(value) } => format!("{value} (updating…)"),
                AsyncState::Pending { previous: None } => "Searching…".to_string(),
                AsyncState::Ready(value) => value.clone(),
                AsyncState::Failed { error, previous: Some(value) } => format!("{value} — {error}"),
                AsyncState::Failed { error, previous: None } => error.clone(),
            })
        }))
        .child(row().gap_2().child(button("Search again").on_press(move || results.refresh())))
}
