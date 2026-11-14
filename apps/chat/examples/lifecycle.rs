//! The actual Chat UI with a controllable, cancellation-resistant transport for live checks.
//! Run via `python3 apps/chat/live_lifecycle.py` after building this example and the CLI.

#[path = "support/controlled.rs"]
mod controlled;

use controlled::Controlled;
use octara::prelude::*;
use octara_chat::engine::{Cancel, Model, Purpose, Request};
use octara_chat::model::Event;
use octara_chat::store::Store;
use octara_chat::{Options, root};
use std::path::PathBuf;
use std::rc::Rc;

/// Even cancellation itself can race with data already delivered by a transport.
struct InFlight(Controlled);

impl Model for InFlight {
    fn name(&self) -> &str {
        "Controlled lifecycle"
    }

    fn respond(&self, request: Request, send: Sender<Event>) -> Cancel {
        let reply = request.purpose == Purpose::Reply;
        let cancel = self.0.respond(request, send.clone());
        Box::new(move || {
            if reply {
                for (label, event) in [
                    ("text", Event::Text(" LATE_CANCEL_TEXT".into())),
                    ("done", Event::Done),
                    ("failed", Event::Failed("LATE_CANCEL_FAILURE".into())),
                ] {
                    let accepted = send.send(event).is_ok();
                    eprintln!("lifecycle: cancel {label} attempted accepted={accepted}");
                }
            }
            cancel();
        })
    }
}

/// These controls exist only in this verification executable. The product UI stays unchanged.
fn controls(model: &Controlled) {
    for ordinal in 0..8 {
        for (verb, event) in [
            ("text", Event::Text(format!("reply-{ordinal} prefix"))),
            ("late", Event::Text(format!(" LATE_REPLY_{ordinal}"))),
            ("done", Event::Done),
            ("failed", Event::Failed(format!("LATE_FAILURE_{ordinal}"))),
        ] {
            let model = model.clone();
            on_command(&format!("lifecycle.reply.{ordinal}.{verb}"), move || {
                let index = model.index(Purpose::Reply, ordinal).expect("the requested reply must have started");
                let accepted = model.emit(index, event.clone());
                eprintln!("lifecycle: reply {ordinal} {verb} attempted accepted={accepted}");
            });
        }
        let cancelled = model.clone();
        on_command(&format!("lifecycle.cancelled.{ordinal}"), move || {
            let index = cancelled.index(Purpose::Reply, ordinal).expect("the cancelled reply must exist");
            assert!(cancelled.cancelled(index), "reply {ordinal} was not cancelled");
            eprintln!("lifecycle: reply {ordinal} cancellation observed");
        });
    }
    for expected in 0..=8 {
        let model = model.clone();
        on_command(&format!("lifecycle.replies.{expected}"), move || {
            let actual = model.count(Purpose::Reply);
            assert_eq!(actual, expected, "unexpected number of model reply requests");
            assert_eq!(model.requests().iter().filter(|r| r.purpose == Purpose::Reply).count(), actual);
            eprintln!("lifecycle: reply requests {actual}");
        });
    }
    let model = model.clone();
    on_command("lifecycle.titles", move || {
        for ordinal in 0..model.count(Purpose::Title) {
            let index = model.index(Purpose::Title, ordinal).expect("the title request exists");
            let accepted = model.emit(index, Event::Text("Lifecycle kept".into()));
            let ended = model.emit(index, Event::Done);
            eprintln!("lifecycle: title {ordinal} attempted accepted={accepted} ended={ended}");
        }
    });
}

#[component]
fn lifecycle(model: Controlled, directory: PathBuf) -> View {
    root(move || {
        controls(&model);
        Options { models: vec![Rc::new(InFlight(model.clone()))], store: Some(Store::new(&directory)), ..Options::default() }
    })
}

fn main() {
    let directory =
        std::env::var_os("OCTARA_CHAT_DIR").map(PathBuf::from).expect("set an isolated OCTARA_CHAT_DIR for lifecycle verification");
    let model = Controlled::default();
    octara_shell::run_with(
        octara_shell::Options { title: "Octara Chat lifecycle".into(), size: (1000.0, 720.0), ..Default::default() },
        move || lifecycle(model.clone(), directory.clone()),
    );
}
