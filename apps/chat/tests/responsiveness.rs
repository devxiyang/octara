//! Bounded long-answer transport and retained editor behavior during projected updates.

#[path = "../examples/support/headless.rs"]
mod headless;
#[path = "../examples/support/long_session.rs"]
mod long_session;

use headless::Fixture;
use long_session::{MAX_REPLY_BYTES, MAX_REPLY_CHUNKS, Payload};
use octara::{EditorEvent, Input, TextEdit};
use octara_chat::model::Status;
use octara_inspect::{AgentCommand, AgentRequest};
use std::time::{Duration, Instant};

#[test]
fn long_paragraphs_and_fences_keep_the_same_focused_composer_and_selection() {
    for payload in [Payload::Paragraph, Payload::Code] {
        let mut fixture = Fixture::new(1_000, payload, 1024).unwrap();
        fixture.command("chat.focus_composer").unwrap();
        let observe = |fixture: &mut Fixture| {
            fixture.driver.execute(AgentRequest::new(Some(fixture.window), AgentCommand::Observe)).snapshot.unwrap()
        };
        let editor = observe(&mut fixture).editor.unwrap();
        let draft = "A draft with 中文 and 😀";
        fixture
            .driver
            .dispatch(
                fixture.window,
                Input::Editor(EditorEvent::Edit {
                    session: editor.session,
                    expected_rev: editor.document_rev,
                    edit: TextEdit::Insert { text: draft.into(), replacement: None },
                }),
            )
            .unwrap();
        let editor = observe(&mut fixture).editor.unwrap();
        fixture
            .driver
            .dispatch(
                fixture.window,
                Input::Editor(EditorEvent::Select {
                    session: editor.session,
                    expected_rev: editor.document_rev,
                    selection: (draft.len(), 2),
                }),
            )
            .unwrap();
        for _ in 0..4 {
            fixture.sample(16).unwrap();
        }
        let after = observe(&mut fixture).editor.unwrap();
        assert_eq!(after.session, editor.session);
        assert_eq!(after.value.as_deref(), Some(draft));
        assert_eq!(after.selection, Some((draft.len(), 2)));
    }
}

#[test]
fn four_mib_of_streamed_text_keeps_the_editor_and_virtual_transcript_responsive() {
    let mut fixture = Fixture::new(10_000, Payload::Paragraph, 16 * 1024).unwrap();
    fixture.command("chat.focus_composer").unwrap();
    let observe =
        |fixture: &mut Fixture| fixture.driver.execute(AgentRequest::new(Some(fixture.window), AgentCommand::Observe)).snapshot.unwrap();
    let editor = observe(&mut fixture).editor.unwrap();
    let draft = "Keep drafting 😀";
    fixture
        .driver
        .dispatch(
            fixture.window,
            Input::Editor(EditorEvent::Edit {
                session: editor.session,
                expected_rev: editor.document_rev,
                edit: TextEdit::Insert { text: draft.into(), replacement: None },
            }),
        )
        .unwrap();

    let stream = fixture.sample(256).unwrap();
    assert_eq!(stream.bytes, 4 * 1024 * 1024);
    assert!((1..100).contains(&stream.mounted_messages), "{}", stream.mounted_messages);
    let readback = fixture.driver.app_mut().update_window(fixture.window, |w| fixture.model.readback(w).unwrap()).unwrap();
    assert_eq!(readback.consumed_text_bytes, stream.bytes);
    assert!(readback.answering);
    let after = observe(&mut fixture).editor.unwrap();
    assert_eq!(after.session, editor.session);
    assert_eq!(after.value.as_deref(), Some(draft));
}

#[test]
fn stop_freezes_consumed_source_and_rejects_late_long_stream_events() {
    let mut fixture = Fixture::new(1_000, Payload::Code, 4096).unwrap();
    fixture.sample(32).unwrap();
    fixture.command("long.stop").unwrap();
    let after = fixture.driver.app_mut().update_window(fixture.window, |w| fixture.model.readback(w).unwrap()).unwrap();
    assert_eq!(after.status, Some(Status::Stopped));
    assert_eq!(after.consumed_text_bytes, after.sent_bytes);
    assert_eq!(fixture.model.attempt_late(), (false, false));
    let late = fixture.driver.app_mut().update_window(fixture.window, |w| fixture.model.readback(w).unwrap()).unwrap();
    assert_eq!(late.consumed_text_bytes, after.consumed_text_bytes);
    assert_eq!(late.late_attempts, 2);
    assert_eq!(late.late_accepted, 0);
}

#[test]
fn fixture_caps_queued_bytes_before_presentation() {
    let fixture = Fixture::new(1_000, Payload::Paragraph, 16_384).unwrap();
    while fixture.model.emit_chunk() {}
    assert!(fixture.model.sent_bytes() <= MAX_REPLY_BYTES);
    assert!(fixture.model.sent() <= MAX_REPLY_CHUNKS);
    assert!(fixture.model.sent_bytes() > MAX_REPLY_BYTES - 16_384);
}

#[test]
fn wall_clock_producer_works_at_fixed_virtual_time_and_retires_when_paused() {
    let mut fixture = Fixture::new(1_000, Payload::Mixed, 256).unwrap();
    fixture.model.set_rate(200).unwrap();
    let time = fixture.driver.app().runtime().now();
    fixture.command("long.run.short").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while fixture.model.sent() < 3 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    fixture.command("long.pause").unwrap();
    while fixture.model.workers() != 0 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(fixture.driver.app().runtime().now(), time);
}
