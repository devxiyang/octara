//! A simulated search whose most recent query owns the response.

use super::section;
use octara::prelude::*;
use octara_kit::{ButtonVariant, Size as KitSize, button, search_field};
use std::time::Duration;

#[ui]
pub fn view(cx: &mut BuildCx<'_>) -> El {
    let query = cx.signal("octara".to_owned());
    let refresh = cx.signal(0_u32);
    let result = cx.signal("Searching…".to_owned());
    let request = cx.signal(None);
    cx.watch(
        move |r| (r.get(query), r.get(refresh)),
        move |w, (query, _), _| {
            if let Some(previous) = w.get(request) {
                w.cancel_task(previous);
            }
            w.set(result, "Searching…".to_owned());
            let query = query.trim().to_owned();
            let task = w.spawn("simulated search", move |task| async move {
                task.sleep(Duration::from_millis(600)).await;
                task.write(|w| {
                    let reply = if query.is_empty() {
                        "Type something to search.".to_owned()
                    } else {
                        format!("{} results for “{query}”", query.len() * 7)
                    };
                    w.set(result, reply);
                    w.set(request, None);
                });
            });
            w.set(request, Some(task));
        },
    );
    section("Async search")
        .child(
            search_field(cx, "query", "Search", query.into(), move |editor| {
                editor.on_submit(move |w| w.update(refresh, |value| *value += 1))
            })
            .w(260),
        )
        .child(text(result).role(Role::Status).live(Live::Polite))
        .child(button("Search again", KitSize::Md, ButtonVariant::Secondary).on_press(move |w| w.update(refresh, |value| *value += 1)))
}
