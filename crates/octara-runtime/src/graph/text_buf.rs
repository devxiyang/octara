//! A capability-scoped UTF-8 text source with a bounded, replayable edit log.

use std::ops::Range;

use super::delta::{Changes, Cursor, DeltaLog};
use super::text_stream::{TextStreamDrain, TextStreamSink};
use crate::arena::{NodeKind, ReactiveId, ReactiveWorld};
use crate::cx::{BuildCx, ReadCx, WriteCx};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextBuffer(ReactiveId);

/// A change to the authoritative buffer. Splice offsets count UTF-8 bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextEdit {
    Append(String),
    Splice { range: Range<usize>, insert: String },
}

struct TextData {
    value: String,
    log: DeltaLog<TextEdit>,
    stream: Option<TextStreamDrain>,
}

impl BuildCx<'_> {
    #[track_caller]
    pub fn text_buffer(&mut self, init: impl Into<String>) -> TextBuffer {
        TextBuffer(self.world.alloc_value(
            NodeKind::TextBuffer,
            self.scope,
            self.window,
            TextData { value: init.into(), log: DeltaLog::default(), stream: None },
        ))
    }

    /// Connect a producer to this buffer. Capacity rounds up to a power of two.
    /// Reconnecting closes the previous producer and discards its queued chunks.
    pub fn text_stream(&mut self, buffer: TextBuffer, capacity: usize) -> TextStreamSink {
        let wake = self.tasks.as_ref().and_then(|tasks| tasks.host_wake());
        let (sink, drain) = TextStreamSink::pair(capacity, wake);
        self.world.value_mut::<TextData>(buffer.0).stream = Some(drain);
        self.world.text_streams.insert(buffer.0);
        sink
    }

    #[track_caller]
    pub fn sample_text(&mut self, buffer: TextBuffer) -> String {
        ReadCx::new(self.world).text(buffer)
    }
}

impl ReadCx<'_> {
    #[track_caller]
    pub fn text(&mut self, buffer: TextBuffer) -> String {
        self.with_text(buffer, str::to_owned)
    }

    #[track_caller]
    pub fn with_text<R>(&mut self, buffer: TextBuffer, read: impl FnOnce(&str) -> R) -> R {
        self.track_id(buffer.0);
        read(&self.world.value::<TextData>(buffer.0).value)
    }

    #[track_caller]
    pub fn text_cursor(&mut self, buffer: TextBuffer) -> Cursor {
        self.track_id(buffer.0);
        self.world.value::<TextData>(buffer.0).log.cursor()
    }

    #[track_caller]
    pub fn text_changes(&mut self, buffer: TextBuffer, cursor: &mut Cursor) -> Changes<TextEdit> {
        self.track_id(buffer.0);
        self.world.value::<TextData>(buffer.0).log.take(cursor)
    }
}

impl WriteCx<'_> {
    #[track_caller]
    pub fn text(&mut self, buffer: TextBuffer) -> String {
        self.with_text(buffer, str::to_owned)
    }

    #[track_caller]
    pub fn with_text<R>(&mut self, buffer: TextBuffer, read: impl FnOnce(&str) -> R) -> R {
        read(&self.world.value::<TextData>(buffer.0).value)
    }

    #[track_caller]
    pub fn append_text(&mut self, buffer: TextBuffer, text: &str) {
        if !text.is_empty() {
            self.splice_text(buffer, TextEdit::Append(text.to_owned()));
        }
    }

    /// Applies an edit if its range is valid and it changes the text.
    #[track_caller]
    pub fn splice_text(&mut self, buffer: TextBuffer, edit: TextEdit) -> bool {
        let data = self.world.value_mut::<TextData>(buffer.0);
        let bytes = match &edit {
            TextEdit::Append(text) => {
                if text.is_empty() {
                    return false;
                }
                data.value.push_str(text);
                text.capacity()
            }
            TextEdit::Splice { range, insert } => {
                let Some(previous) = data.value.get(range.clone()) else { return false };
                if previous == insert {
                    return false;
                }
                data.value.replace_range(range.clone(), insert);
                insert.capacity()
            }
        };
        data.log.push_sized(edit, bytes);
        self.world.mark_changed(buffer.0);
        true
    }
}

impl ReactiveWorld {
    pub(crate) fn has_pending_text_stream(&self) -> bool {
        self.text_streams.iter().any(|&id| self.value::<TextData>(id).stream.as_ref().is_some_and(TextStreamDrain::pending))
    }

    pub(crate) fn drain_text_streams(&mut self) -> bool {
        let mut flushed = false;
        let mut streams = std::mem::take(&mut self.text_streams);
        streams.retain(|&id| {
            if !self.is_alive(id) {
                return false;
            }
            let data = self.value_mut::<TextData>(id);
            let Some(drain) = data.stream.as_mut() else { return false };
            let (text, open) = drain.take();
            if !open {
                data.stream = None;
            }
            if let Some(text) = text {
                let bytes = text.capacity();
                data.value.push_str(&text);
                data.log.push_sized(TextEdit::Append(text), bytes);
                self.mark_changed(id);
                flushed = true;
            }
            open
        });
        self.text_streams = streams;
        flushed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::ReactiveWorld;

    #[test]
    fn utf8_splices_and_appends_replay_in_order() {
        let mut world = ReactiveWorld::new();
        let scope = world.root_scope();
        let text = world.build(scope, None).text_buffer("a😀z");
        let mut cursor = world.read().text_cursor(text);
        let mut write = world.write(scope, None);
        let valid = TextEdit::Splice { range: 1..5, insert: "界".into() };
        assert!(!write.splice_text(text, TextEdit::Splice { range: 1..3, insert: "broken".into() }));
        assert!(!write.splice_text(text, TextEdit::Splice { range: 5..20, insert: "broken".into() }));
        assert!(write.splice_text(text, valid.clone()));
        assert!(!write.splice_text(text, TextEdit::Splice { range: 1..4, insert: "界".into() }));
        write.append_text(text, "");
        write.append_text(text, "🍉");
        assert_eq!(write.text(text), "a界z🍉");
        assert_eq!(world.read().text_changes(text, &mut cursor), Changes::Delta(vec![valid, TextEdit::Append("🍉".into())]));
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    use crate::graph::text_stream::TextStreamError;

    #[test]
    fn a_stream_burst_replays_as_one_edit_and_scope_drop_closes_the_sink() {
        let mut world = ReactiveWorld::new();
        let scope = world.root_scope();
        let mut build = world.build(scope, None);
        let buffer = build.text_buffer("A");
        let mut sink = build.text_stream(buffer, 2);
        let mut cursor = world.read().text_cursor(buffer);
        let mut sink = std::thread::spawn(move || {
            sink.try_send("é".into()).unwrap();
            sink.try_send("🍉".into()).unwrap();
            assert_eq!(sink.try_send("overflow".into()), Err(TextStreamError::Full("overflow".into())));
            sink
        })
        .join()
        .unwrap();
        world.drain_text_streams();
        assert_eq!(world.read().text(buffer), "Aé🍉");
        assert_eq!(world.read().text_changes(buffer, &mut cursor), Changes::Delta(vec![TextEdit::Append("é🍉".into())]));
        sink.try_send("B".into()).unwrap();
        world.drain_text_streams();
        assert_eq!(world.read().text(buffer), "Aé🍉B");
        drop(world);
        assert_eq!(sink.try_send("lost".into()), Err(TextStreamError::Closed("lost".into())));
    }
}

#[cfg(test)]
#[test]
fn app_turn_publishes_one_stream_batch_per_frame() {
    use crate::AppRuntime;

    let mut app = AppRuntime::new(|_, _| unreachable!("the stream uses the app scope"));
    let (buffer, mut sink) = app
        .build(|build| {
            let buffer = build.text_buffer("");
            (buffer, build.text_stream(buffer, 4))
        })
        .unwrap();
    sink.try_send("A".into()).unwrap();
    app.settle().unwrap();
    assert_eq!(app.world.read().text(buffer), "A");
    let mut cursor = app.world.read().text_cursor(buffer);

    sink.try_send("é".into()).unwrap();
    app.settle().unwrap();
    sink.try_send("B".into()).unwrap();
    app.settle().unwrap();
    assert_eq!(app.world.read().text(buffer), "A");
    app.advance_to(app.next_deadline().unwrap()).unwrap();
    assert_eq!(app.world.read().text_changes(buffer, &mut cursor), Changes::Delta(vec![TextEdit::Append("éB".into())]));
}
