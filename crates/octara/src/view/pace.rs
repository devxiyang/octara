//! Time-based display of streaming text on the runtime clock.

use octara_runtime::{
    BuildCx, TextBuffer,
    graph::{delta::Changes, text_buf::TextEdit},
};
use std::{cell::Cell, rc::Rc, time::Duration};

const FRAME: Duration = Duration::from_micros(16_667);

/// Returns a scoped text buffer that reveals appended text over several clock ticks. Replacements
/// and deletions update the displayed buffer immediately.
#[track_caller]
pub fn paced(cx: &mut BuildCx<'_>, source: TextBuffer) -> TextBuffer {
    let initial = cx.sample_text(source);
    let displayed = cx.text_buffer(initial);
    let running = cx.signal(false);
    let delivered = Rc::new(Cell::new(None));
    let seen = delivered.clone();
    cx.watch(move |r| {
        let cursor = r.text_cursor(source);
        let appends_only = seen.get().is_some_and(|mut previous| {
            matches!(r.text_changes(source, &mut previous), Changes::Delta(edits) if edits.iter().all(|edit| matches!(edit, TextEdit::Append(_))))
        });
        (cursor, appends_only)
    }, move |w, &(cursor, appends_only), _| {
        delivered.set(Some(cursor));
        if !appends_only {
            let insert = w.text(source);
            let len = w.with_text(displayed, str::len);
            w.splice_text(displayed, TextEdit::Splice { range: 0..len, insert });
            return;
        }
        if w.get(running) {
            return;
        }
        w.set(running, true);
        w.spawn("paced text", move |task| async move {
            loop {
                task.sleep(FRAME).await;
                let behind = task.write(|w| {
                    let start = w.with_text(displayed, str::len);
                    let (piece, behind) = w.with_text(source, |source| {
                        let suffix = source.get(start..).unwrap_or_default();
                        let mut end = (suffix.len() / 4).max(8).min(suffix.len());
                        while !suffix.is_char_boundary(end) {
                            end += 1;
                        }
                        (suffix[..end].to_owned(), end < suffix.len())
                    });
                    w.append_text(displayed, &piece);
                    if !behind {
                        w.set(running, false);
                    }
                    behind
                });
                if behind != Some(true) {
                    break;
                }
            }
        });
    });
    displayed
}
