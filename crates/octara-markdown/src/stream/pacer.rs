use octara::{ColorValue, TextReveal, TextRevealKind};
use std::time::Duration;

const INITIAL_GAP: Duration = Duration::from_millis(40);
const MIN_WAIT: Duration = Duration::from_millis(8);
const MAX_WAIT: Duration = Duration::from_millis(250);
const MAX_LAG: Duration = Duration::from_secs(1);
const FINISH_DRAIN: Duration = Duration::from_millis(192);

/// Options passed to the paint-time glyph reveal for an active streaming tail.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum StreamingEffects {
    #[default]
    None,
    Fade {
        duration_secs: f32,
    },
    Gradient {
        trail_bytes: u32,
        tint: Option<ColorValue>,
        glide_bytes_per_sec: f32,
    },
}

impl StreamingEffects {
    pub fn reveal(self, revealed_bytes: usize, finished: bool) -> Option<TextReveal> {
        let kind = match self {
            Self::None => return None,
            Self::Fade { duration_secs } => TextRevealKind::Fade { duration_secs },
            Self::Gradient { trail_bytes, tint, glide_bytes_per_sec } => {
                TextRevealKind::Gradient { trail_bytes, tint, glide_bytes_per_sec }
            }
        };
        Some(TextReveal::new(kind, revealed_bytes, finished))
    }
}

#[derive(Clone, Copy, Debug, Default)]
enum Line {
    /// Waiting to see the line's first non-whitespace byte.
    #[default]
    Start,
    Text,
    Table,
}

#[derive(Debug)]
struct Drain {
    started: Duration,
    bytes: usize,
    emitted: usize,
}

/// Batches incoming tokens using an exponentially weighted estimate of their arrival gap.
///
/// Supply timestamps from the same monotonic clock. `tick` yields append-only UTF-8 fragments,
/// waiting for complete words and pipe-led table rows when possible. A stalled fragment can
/// remain pending for at most one second between ticks.
#[derive(Debug)]
pub struct StreamingPacer {
    pending: String,
    oldest: Option<Duration>,
    last_arrival: Option<Duration>,
    gap: Duration,
    line: Line,
    drain: Option<Drain>,
}

impl Default for StreamingPacer {
    fn default() -> Self {
        Self { pending: String::new(), oldest: None, last_arrival: None, gap: INITIAL_GAP, line: Line::Start, drain: None }
    }
}

impl StreamingPacer {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn from_visible(text: &str) -> Self {
        let tail = text.rsplit(['\r', '\n']).next().unwrap_or("");
        let line = tail.trim_start().chars().next().map_or(Line::Start, |ch| next_line(Line::Start, ch));
        Self { line, ..Self::default() }
    }

    /// Adds token text. A new token resumes pacing if a drain was in progress.
    pub fn push(&mut self, chunk: &str, now: Duration) {
        if chunk.is_empty() {
            return;
        }
        self.cancel_finish();
        self.oldest.get_or_insert(now);
        if let Some(last) = self.last_arrival
            && now > last
        {
            let sample = (now - last).min(MAX_LAG);
            self.gap = self.gap.mul_f64(0.75) + sample.mul_f64(0.25);
        }
        self.last_arrival = Some(now);
        self.pending.push_str(chunk);
    }

    /// Starts a bounded drain, allowing the final word or last row through.
    pub fn finish(&mut self, now: Duration) {
        self.drain.get_or_insert(Drain { started: now, bytes: self.pending.len(), emitted: 0 });
    }

    pub(crate) fn cancel_finish(&mut self) {
        self.drain = None;
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn is_finished(&self) -> bool {
        self.drain.is_some() && self.pending.is_empty()
    }

    /// Returns the next append for the preview, if its playback time has arrived.
    pub fn tick(&mut self, now: Duration) -> Option<String> {
        let age = now.saturating_sub(self.oldest?);
        let mut end = if let Some(drain) = &self.drain {
            let elapsed = now.saturating_sub(drain.started);
            if elapsed >= FINISH_DRAIN || age >= MAX_LAG {
                self.pending.len()
            } else {
                let fraction = elapsed.as_secs_f64() / FINISH_DRAIN.as_secs_f64();
                ((drain.bytes as f64 * fraction) as usize).saturating_sub(drain.emitted)
            }
        } else if age >= MAX_LAG {
            self.pending.len()
        } else {
            if age < self.gap.clamp(MIN_WAIT, MAX_WAIT) {
                return None;
            }
            self.complete_prefix()
        };
        end = end.min(self.pending.len());
        while !self.pending.is_char_boundary(end) {
            end -= 1;
        }
        if end == 0 {
            return None;
        }
        let output: String = self.pending.drain(..end).collect();
        for ch in output.chars() {
            self.line = next_line(self.line, ch);
        }
        if let Some(drain) = &mut self.drain {
            drain.emitted += end;
        }
        if self.pending.is_empty() {
            self.oldest = None;
        }
        Some(output)
    }

    fn complete_prefix(&self) -> usize {
        let mut line = self.line;
        let mut end = 0;
        let mut before_line = 0;
        for (at, ch) in self.pending.char_indices() {
            line = next_line(line, ch);
            match (line, ch) {
                (Line::Start, '\r' | '\n') => {
                    end = at + 1;
                    before_line = end;
                }
                (Line::Table, _) => end = before_line,
                (Line::Text, _) if ch.is_whitespace() => end = at + ch.len_utf8(),
                _ => {}
            }
        }
        end
    }
}

fn next_line(line: Line, ch: char) -> Line {
    match (line, ch) {
        (_, '\r' | '\n') => Line::Start,
        (Line::Start, '|') => Line::Table,
        (Line::Start, ch) if !ch.is_whitespace() => Line::Text,
        _ => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paces_complete_words_and_holds_table_rows() {
        let at = Duration::from_millis;
        let mut pacer = StreamingPacer::new();
        pacer.push("Hello wor", at(0));
        assert_eq!(pacer.tick(at(10)), None);
        assert_eq!(pacer.tick(at(80)).as_deref(), Some("Hello "));
        pacer.push("ld\n  | left |", at(95));
        assert_eq!(pacer.tick(at(125)).as_deref(), Some("world\n"));
        pacer.push(" right |\n翻", at(130));
        assert_eq!(pacer.tick(at(150)).as_deref(), Some("  | left | right |\n"));
        assert_eq!(pacer.tick(at(1000)).as_deref(), Some("翻"));
        assert!(!pacer.has_pending());
    }

    #[test]
    fn completion_drains_utf8_within_two_hundred_milliseconds() {
        let at = Duration::from_millis;
        let mut pacer = StreamingPacer::new();
        pacer.push("a翻訳", at(50));
        pacer.finish(at(60));
        let first = pacer.tick(at(160)).unwrap();
        let rest = pacer.tick(at(260)).unwrap();
        assert_eq!(first + &rest, "a翻訳");
        assert!(pacer.is_finished());
    }

    #[test]
    fn new_tokens_resume_from_a_pending_drain() {
        let at = Duration::from_millis;
        let mut pacer = StreamingPacer::new();
        pacer.push("unfinished", at(10));
        pacer.finish(at(15));
        pacer.push(" text ", at(20));
        assert!(!pacer.is_finished());
        assert_eq!(pacer.tick(at(90)).as_deref(), Some("unfinished text "));
    }

    #[test]
    fn an_existing_visible_table_row_is_held_until_its_line_ends() {
        let at = Duration::from_millis;
        let mut pacer = StreamingPacer::from_visible("Intro\n| first");
        pacer.push(" cell ", at(0));
        assert_eq!(pacer.tick(at(80)), None);
        pacer.push("|\n", at(90));
        assert_eq!(pacer.tick(at(100)).as_deref(), Some(" cell |\n"));
    }
}
