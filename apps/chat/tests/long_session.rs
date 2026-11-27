//! The product transcript mounts only the visible window of a long stored session.

#[path = "../examples/support/headless.rs"]
mod headless;
#[path = "../examples/support/long_session.rs"]
mod long_session;

use headless::Fixture;
use long_session::Payload;
use octara::prelude::{A11yAction, Modifiers, Point, Size, Vec2, WindowViewport};
use octara::{HostReply, Input, InteractionEvent, WheelPhase};
use octara_inspect::{AgentCommand, AgentRequest};

#[test]
fn thousand_and_ten_thousand_message_histories_use_the_same_virtual_transcript() {
    for history in [1_000, 10_000] {
        let mut fixture = Fixture::new(history, Payload::Mixed, 64).unwrap();
        let sample = fixture.sample(1).unwrap();
        assert!(sample.mounted_messages > 0 && sample.mounted_messages < 100, "{history} history rows");
        let readback = fixture.driver.app_mut().update_window(fixture.window, |w| fixture.model.readback(w).unwrap()).unwrap();
        assert_eq!(readback.consumed_text_bytes, readback.sent_bytes);
        assert!(readback.answering);
        fixture.command("long.read").unwrap();
        let read = fixture.sample(1).unwrap();
        assert!(read.mounted_messages < 100);
        fixture
            .driver
            .dispatch(
                fixture.window,
                Input::Host(HostReply::ViewportChanged(WindowViewport {
                    size: Size::new(800.0, 650.0),
                    scale: 1.0,
                    safe_area: None,
                    keyboard_inset: 0.0,
                })),
            )
            .unwrap();
        let snapshot = fixture.driver.execute(AgentRequest::new(Some(fixture.window), AgentCommand::Observe)).snapshot.unwrap();
        assert!(snapshot.nodes.iter().any(|node| node.test_id.as_deref() == Some("composer")));
        assert!(fixture.sample(1).unwrap().mounted_messages < 100);
    }
}

#[test]
fn scrolling_back_holds_the_reading_position_and_jump_resumes_following() {
    let mut fixture = Fixture::new(1_000, Payload::Code, 512).unwrap();
    fixture.sample(4).unwrap();
    let offset = |fixture: &Fixture| {
        let window = fixture.driver.app().window(fixture.window).unwrap();
        let tree = window.runtime().tree();
        let node = tree.test_ids("transcript")[0];
        tree.scroll(node).unwrap().offset.y
    };
    let initial = offset(&fixture);
    assert!(initial > 1000.0);
    let receipt = fixture
        .driver
        .dispatch(
            fixture.window,
            Input::Interaction(InteractionEvent::Wheel {
                pos: Point::new(600.0, 300.0),
                delta: Vec2::new(0.0, -1200.0),
                phase: WheelPhase::Discrete,
                mods: Modifiers::empty(),
            }),
        )
        .unwrap();
    let reading = offset(&fixture);
    let state = fixture.driver.app().window(fixture.window).unwrap();
    let runtime = state.runtime();
    let tree = runtime.tree();
    let hit = runtime.hit_test(Point::new(600.0, 300.0)).and_then(|target| target.hit.node());
    let hit_chain: Vec<_> = std::iter::successors(hit, |&node| tree.logical_parent(node))
        .map(|node| (tree.provenance(node).and_then(|p| p.test_id.as_deref()), tree.scroll(node), runtime.geometry(node)))
        .collect();
    assert!(reading < initial - 500.0, "wheel failed: {initial} to {reading}, {receipt:?}, {hit_chain:?}");
    fixture.sample(16).unwrap();
    assert!((offset(&fixture) - reading).abs() < 1.0, "new output must not snap the reader to the end");
    let snapshot = fixture.driver.execute(AgentRequest::new(Some(fixture.window), AgentCommand::Observe)).snapshot.unwrap();
    let target = snapshot.nodes.iter().find(|node| node.test_id.as_deref() == Some("jump")).unwrap().target;
    fixture.driver.dispatch(fixture.window, Input::Host(HostReply::AccessibilityAction { target, action: A11yAction::Press })).unwrap();
    let jumped = offset(&fixture);
    assert!(jumped > reading + 500.0);
    fixture.sample(8).unwrap();
    assert!(offset(&fixture) > jumped, "later output should move the transcript after Jump to latest");
}
