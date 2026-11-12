//! What answers: a `Model` takes a request and sends `Event`s back through a channel into the UI
//! thread, from wherever it works (a network model from a background thread). The reference app
//! ships `Scripted`, an offline model that answers from a script on the app's clock, so tests,
//! recordings and replays see the same answer at the same times.

use crate::model::{Author, Event};
use octara::prelude::*;
use std::time::Duration;

/// What a model is asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Purpose {
    /// The next message of the conversation.
    Reply,
    /// A short title for the conversation (a few words, as text events).
    Title,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Request {
    pub purpose: Purpose,
    /// The conversation so far, oldest first; the last is the person's.
    pub history: Vec<(Author, String)>,
}

/// Stops an answer in progress.
pub type Cancel = Box<dyn Fn()>;

/// Something that answers.
pub trait Model {
    /// Its name, as the model picker shows it.
    fn name(&self) -> &str;
    /// Starts answering `request`, sending events to `send` until `Done` or `Failed`. Called on
    /// the UI thread, in a window's code; the answer may come from anywhere.
    fn respond(&self, request: Request, send: Sender<Event>) -> Cancel;
}

/// An offline model answering from a script: it thinks about the question, calls a tool when
/// asked about the weather, fails when asked to, and answers in Markdown, a few words every
/// `step` on the app's clock.
pub struct Scripted {
    pub name: String,
    pub step: Duration,
}

impl Default for Scripted {
    fn default() -> Self {
        Scripted { name: "Scripted".into(), step: Duration::from_millis(30) }
    }
}

impl Model for Scripted {
    fn name(&self) -> &str {
        &self.name
    }
    fn respond(&self, request: Request, send: Sender<Event>) -> Cancel {
        let script = script(&request);
        let step = self.step;
        let task = spawn(async move {
            for event in script {
                sleep(step).await;
                if send.send(event).is_err() {
                    return;
                }
            }
        });
        Box::new(move || task.cancel())
    }
}

/// Words in groups of `n`, each with the space after it.
fn chunks(text: &str, n: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_inclusive(' ').collect();
    words.chunks(n).map(|c| c.concat()).collect()
}

/// What the scripted model sends for `request`.
pub fn script(request: &Request) -> Vec<Event> {
    let question = request.history.iter().rev().find(|(a, _)| *a == Author::User).map(|(_, t)| t.trim().to_owned()).unwrap_or_default();
    if request.purpose == Purpose::Title {
        let title: Vec<&str> = question.split_whitespace().take(5).collect();
        let title = if title.is_empty() { "New chat".to_owned() } else { title.join(" ").trim_end_matches(['?', '.', '!']).to_owned() };
        return vec![Event::Text(title), Event::Done];
    }
    let lower = question.to_lowercase();
    let mut out = Vec::new();
    for c in chunks(&format!("The person asks: \"{question}\". I will answer briefly, with an example."), 3) {
        out.push(Event::Reasoning(c));
    }
    out.push(Event::ReasoningEnd);
    if lower.contains("weather") {
        let id = "call-1".to_owned();
        out.push(Event::ToolStart { id: id.clone(), name: "weather".into() });
        for d in ["{\"city\": ", "\"Oslo\"}"] {
            out.push(Event::ToolInput { id: id.clone(), delta: d.into() });
        }
        out.push(Event::ToolRun { id: id.clone() });
        out.push(Event::ToolDone { id, output: "{\"temperature\": 4, \"sky\": \"cloudy\"}".into() });
    }
    let answer = if lower.contains("weather") {
        "It is **4 °C** and cloudy in Oslo.\n\n- Take a coat.\n- Rain is likely later.".to_owned()
    } else {
        format!(
            "## About \"{question}\"\n\nHere is a short answer, streamed a few words at a time.\n\n1. The first point.\n2. The second point, with `inline code`.\n\n```rust\nfn main() {{\n    println!(\"hello\");\n}}\n```\n\nAsk again to see it stream once more."
        )
    };
    let words = chunks(&answer, 2);
    if lower.contains("fail") {
        out.extend(words.into_iter().take(4).map(Event::Text));
        out.push(Event::Failed("The connection to the model closed.".into()));
        return out;
    }
    out.extend(words.into_iter().map(Event::Text));
    out.push(Event::Done);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(q: &str, purpose: Purpose) -> Vec<Event> {
        script(&Request { purpose, history: vec![(Author::User, q.into())] })
    }

    #[test]
    fn scripts() {
        let weather = ask("What is the weather?", Purpose::Reply);
        assert!(weather.iter().any(|e| matches!(e, Event::ToolStart { name, .. } if name == "weather")));
        assert_eq!(weather.last(), Some(&Event::Done));
        assert!(matches!(ask("please fail", Purpose::Reply).last(), Some(Event::Failed(_))));
        assert_eq!(
            ask("What is the weather in Oslo today, please?", Purpose::Title),
            vec![Event::Text("What is the weather in".into()), Event::Done]
        );
    }
}
