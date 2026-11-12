//! Paced text ([text core](../../../../docs/design/text.md)): a `TextBuffer` that follows another
//! one, revealing what is appended to it over a few frames instead of in bursts (a model's reply
//! arriving in chunks). A backlog drains by a share each frame, so a burst shows within about a
//! quarter of a second and a steady trickle shows as it comes. Edits other than appends, and
//! Reduce Motion, show at once. It is motion: `settle_motion` lands it, and pending work lists it
//! while it is behind.

use crate::reactive::graph::{rt, watch};
use crate::reactive::text::{TextBuffer, TextChange};
use crate::task::{WorkKind, channel, sleep, spawn_with};
use std::panic::Location;
use std::rc::Rc;
use std::time::Duration;

/// One frame at 60 Hz.
const FRAME: Duration = Duration::from_micros(16_667);
/// The share of the backlog shown each frame.
const SHARE: usize = 4;
/// The fewest bytes shown in a frame.
const MIN_STEP: usize = 8;

/// A paced text the window knows of: what it follows and what it shows.
pub(crate) struct Paced {
    pub(crate) source: TextBuffer,
    pub(crate) shown: TextBuffer,
    pub(crate) at: &'static Location<'static>,
}

impl Paced {
    /// Whether it has not shown all of its source yet.
    pub(crate) fn behind(&self) -> bool {
        self.shown.len_untracked() < self.source.len_untracked()
    }
    /// Shows everything now.
    pub(crate) fn land(&self) {
        let (from, to) = (self.shown.len_untracked(), self.source.len_untracked());
        if from < to {
            self.shown.append(&self.source.slice_untracked(from..to));
        }
    }
    /// Shows the next frame's share; whether it is still behind.
    fn step(&self) -> bool {
        let (from, to) = (self.shown.len_untracked(), self.source.len_untracked());
        if from >= to {
            return false;
        }
        let want = ((to - from) / SHARE).max(MIN_STEP);
        let cut = self.source.char_boundary_at((from + want).min(to));
        self.shown.append(&self.source.slice_untracked(from..cut));
        cut < to
    }
}

/// A text that follows `source`, showing what is appended to it paced over a few frames (pass
/// it to `text` or the kit's `markdown` in place of the source). It lives as long as the current
/// scope.
#[track_caller]
pub fn paced(source: &TextBuffer) -> TextBuffer {
    let at = Location::caller();
    let shown = TextBuffer::new(source.get_untracked());
    let u = crate::app::window();
    let p = Rc::new(Paced { source: source.clone(), shown: shown.clone(), at });
    let id = {
        let n = u.next.get() + 1;
        u.next.set(n);
        u.paced.borrow_mut().push((n, p.clone()));
        n
    };
    {
        let u = u.clone();
        rt(|rt| rt.on_cleanup(Box::new(move || u.paced.borrow_mut().retain(|(i, _)| *i != id))));
    }
    let (tx, rx) = channel::<()>();
    let cursor = source.subscribe();
    {
        let (p, u, source) = (p.clone(), u.clone(), source.clone());
        watch(
            move || source.version(),
            move |_, before| {
                if before.is_none() {
                    return;
                }
                let edited = p.source.take(&cursor).iter().any(|c| !matches!(c, TextChange::Append(_)));
                if edited || p.shown.len_untracked() > p.source.len_untracked() {
                    // Not an append: what it shows is replaced at once.
                    p.shown.set(&p.source.get_untracked());
                } else if u.kernel.borrow().reduced_motion() {
                    p.land();
                } else {
                    let _ = tx.send(());
                }
            },
        );
    }
    let fut = async move {
        while rx.recv().await.is_some() {
            while p.step() {
                sleep(FRAME).await;
            }
        }
    };
    spawn_with(fut, WorkKind::Subscription, "paced text".into(), at);
    shown
}
