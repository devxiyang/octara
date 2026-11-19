//! A real-window fixture for application-owned text and native IME delivery.

use octara::kernel::TextDocument;
use octara::prelude::*;
use octara_primitives::identity::NodeId;
use octara_primitives::text::TextEdit;

const PRIMARY: &str = "ime-controlled";
const OTHER: &str = "ime-controlled-other";
const SEED: &str = "A😀Z";
const OTHER_SEED: &str = "B😀Y";

#[derive(Default)]
struct Edits {
    proposals: u32,
    accepted: u32,
    last: String,
}

#[derive(Clone)]
struct Retired {
    owner: NodeId,
    document: TextDocument,
    insert: String,
}

fn capture(window: &Window, proposal: &str) -> Option<Retired> {
    let snapshot = window.inspect();
    if snapshot.focus.as_deref() != Some(PRIMARY) {
        return None;
    }
    let input = snapshot.text_input?;
    let marked = input.marked?;
    // In this fixture the candidate replaces the emoji in A😀Z. Reuse the
    // candidate that reached on_input in the previous native rejection case.
    let insert = proposal.strip_prefix('A')?.strip_suffix('Z')?;
    if insert.is_empty() || input.text != SEED {
        return None;
    }
    let [start, end] = input.selection;
    let (anchor, caret) = if input.reversed { (end, start) } else { (start, end) };
    let mut document = TextDocument::new(input.text, anchor, caret);
    document.apply(&TextEdit::Mark {
        text: input.composition,
        selection: input.composition_selection.map(|[start, end]| start..end),
        replacement: None,
    });
    if document.text() != input.display_text
        || document.marked() != Some(marked[0]..marked[1])
        || document.selection() != input.display_selection.map(|[start, end]| start..end)
    {
        return None;
    }
    Some(Retired { owner: window.kernel().find(PRIMARY)?, document, insert: insert.to_owned() })
}

#[ui]
pub(super) fn view() -> El {
    let id = current_window();
    let value = state("value", || SEED.to_owned());
    let other = state("other", || OTHER_SEED.to_owned());
    let refresh = state("refresh", || 0);
    let reject = state("reject", || false);
    let edits = state("edits", Edits::default);
    let retired = state("retired", || None::<Retired>);
    let report = state("delivery", || "Ready");

    on_command("ime-control.select-emoji", move || {
        let Some(mut window) = App::current().window(id) else { return };
        let snapshot = window.inspect();
        if snapshot.focus.as_deref() == Some(PRIMARY)
            && snapshot.text_input.is_some_and(|input| input.text == SEED && input.marked.is_none())
        {
            window.set_selection(5, 1);
        }
    });
    on_command("ime-control.refresh", move || refresh.update(|n| *n += 1));
    on_command("ime-control.replace", move || value.set("R😀S".to_owned()));
    on_command("ime-control.reject", move || reject.set(true));
    on_command("ime-control.reset", move || {
        value.set(SEED.to_owned());
        other.set(OTHER_SEED.to_owned());
        refresh.set(0);
        reject.set(false);
        edits.set(Edits::default());
        retired.set(None);
        report.set("Ready");
    });
    on_command("ime-control.capture", move || {
        let saved = App::current().window(id).and_then(|window| edits.with(|edits| capture(&window, &edits.last)));
        report.set(if saved.is_some() { "Captured" } else { "Capture unavailable" });
        retired.set(saved);
    });
    on_command("ime-control.deliver-retired", move || {
        let Some(mut window) = App::current().window(id) else { return };
        if window.inspect().focus.as_deref() != Some(OTHER) {
            report.set("Focus the other field");
            return;
        }
        let Some(saved) = retired.get() else {
            report.set("Capture unavailable");
            return;
        };
        let delivered = window.apply_text_edit(saved.owner, &saved.document, &TextEdit::Insert { text: saved.insert, replacement: None });
        report.set(if delivered { "Delivered" } else { "Refused" });
    });

    let body = col().size_full().p_6().gap_3().bg(Token::Surface);
    #[cfg(target_os = "ios")]
    let body = body.pt_16();
    body.child(text("Controlled Chinese input").text_xl().font_semibold())
        .child(
            text_input()
                .id(PRIMARY)
                .label("Controlled value")
                .w(320)
                .h(40)
                .px_3()
                .border()
                .value(move || {
                    let _ = refresh.get();
                    value.get()
                })
                .on_input(move |text| {
                    let accepted = !reject.get();
                    edits.update(|log| {
                        log.proposals += 1;
                        log.accepted += u32::from(accepted);
                        log.last = text.clone();
                    });
                    if accepted {
                        value.set(text);
                    }
                }),
        )
        .child(text_input().id(OTHER).label("Other value").w(320).h(40).px_3().border().value(other).on_input(move |text| other.set(text)))
        .child(text(move || format!("Refreshes {}", refresh.get())).id("ime-control-refreshes"))
        .child(
            text(move || edits.with(|log| format!("Proposals {} · Accepted {} · Last {:?}", log.proposals, log.accepted, log.last)))
                .id("ime-control-edits"),
        )
        .child(text(report).id("ime-control-delivery"))
}
