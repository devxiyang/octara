#![cfg(all(feature = "native", not(target_arch = "wasm32")))]

use octara::{
    prelude::*,
    runtime::Input,
    tree::{NodeChange, NodeContent, NodeFlags, NodeSpec},
};
use octara_inspect::{
    AgentCommand, AgentReceipt, AgentRequest, AuditFinding, AuditNode, AuditRule, HeadlessDriver, InspectTransport, Inspector, TargetRef,
    conformance,
    replay::{Channel, Event, Replay},
};
use octara_model::{
    ask::{ClipboardItem, ServiceId, ServiceOutcome},
    diagnostics::{Severity, codes},
    input::HostReply,
    overlay::LayerPolicy,
    style::Display,
};
use std::{
    cell::RefCell,
    hint::black_box,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

fn driver() -> HeadlessDriver {
    HeadlessDriver::with_namespace([14, 52], Default::default())
}

fn options() -> WindowOptions {
    WindowOptions {
        viewport: WindowViewport { size: Size::new(360.0, 260.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 },
        ..Default::default()
    }
}

fn audit(driver: &mut HeadlessDriver, window: WindowId) -> Vec<AuditFinding> {
    let request = AgentRequest::new(Some(window), AgentCommand::Audit);
    let receipt = driver.execute(request.clone());
    conformance::verify_receipt(&request, &receipt).unwrap();
    assert!(receipt.ok, "{receipt:?}");
    receipt.findings.unwrap()
}

fn target(driver: &HeadlessDriver, window: WindowId, test_id: &str) -> TargetRef {
    let tree = driver.app().runtime().window(window).unwrap().tree();
    let [id] = tree.test_ids(test_id) else { panic!("expected one node for {test_id}") };
    tree.guard(*id)
}

#[test]
fn built_ins_report_host_clips_missing_names_and_unreachable_focus_targets() {
    let mut driver = driver();
    let window = driver
        .app_mut()
        .runtime_mut()
        .open_window(options(), |mount| {
            let parent = mount.root();
            let view = col()
                .child(text("An ellipsis should appear in this long single line").w(39).truncate().test_id("ellipsis"))
                .child(text("This much longer phrase wraps over several of its lines").w(58).max_lines(2).test_id("clamp"))
                .child(el().size(20).role(Role::Button).label(" ").test_id("unnamed"))
                .child(el().size(20).role(Role::Button).child(text("Text content")).test_id("content-name"))
                .child(text("Connected label").id("caption"))
                .child(input().labelled_by("caption").test_id("related-name"))
                .child(
                    el().size(20).role(Role::Button).labelled_by("missing").child(text("Unusable implicit caption")).test_id("broken-name"),
                )
                .child(el().display(Display::None).child(el().size(20).focusable().test_id("display-hidden")))
                .child(el().a11y_hidden().child(el().size(20).focusable().test_id("a11y-hidden")))
                .child(el().w(0).h(20).label("Empty button").focusable().autofocus().test_id("empty"));
            view.into_view().mount(mount, parent, 0);

            let inert = mount.create(NodeSpec::new(NodeContent::Box));
            mount.change(inert, NodeChange::Flags(NodeFlags::INERT));
            mount.place(parent, 1, inert);
            el().size(20).focusable().test_id("inert-child").into_view().mount(mount, inert, 0);
        })
        .unwrap();
    let findings = audit(&mut driver, window);
    assert_eq!(audit(&mut driver, window), findings);

    let expected = [
        ("ellipsis", &codes::CLIPPED_TEXT, "ellipsis", Severity::Warning),
        ("clamp", &codes::CLIPPED_TEXT, "2-line", Severity::Warning),
        ("unnamed", &codes::MISSING_SEMANTICS, "accessible name", Severity::Error),
        ("broken-name", &codes::MISSING_SEMANTICS, "accessible name", Severity::Error),
        ("display-hidden", &codes::DEAD_FOCUS_TARGET, "hidden", Severity::Warning),
        ("a11y-hidden", &codes::DEAD_FOCUS_TARGET, "hidden", Severity::Warning),
        ("empty", &codes::DEAD_FOCUS_TARGET, "empty bounds", Severity::Error),
        ("inert-child", &codes::DEAD_FOCUS_TARGET, "inert", Severity::Warning),
    ];
    assert_eq!(findings.len(), expected.len(), "{findings:?}");
    for (finding, (name, code, message, severity)) in findings.iter().zip(expected) {
        assert_eq!(finding.node, target(&driver, window, name));
        assert_eq!(finding.code, code.id);
        assert_eq!(finding.severity, severity);
        assert!(finding.message.contains(message), "{finding:?}");
        let provenance = finding.provenance.as_ref().unwrap();
        assert_eq!(provenance.file, file!());
        assert!(provenance.line > 0 && provenance.column > 0);
        assert_eq!(provenance.to_string(), format!("{}:{}", file!(), provenance.line));
    }

    let payload = serde_json::to_vec(&AgentRequest::new(Some(window), AgentCommand::Audit)).unwrap();
    let response = driver.exchange(&payload);
    assert!(response.ends_with(b"\n"));
    let receipt: AgentReceipt = serde_json::from_slice(&response).unwrap();
    assert_eq!(receipt.findings, Some(findings));
}

#[test]
fn effective_modal_excludes_background_but_includes_portals_owned_by_the_modal() {
    let mut driver = driver();
    let window = driver
        .app_mut()
        .open_window(options(), |_| {
            col().child(el().size(40).label("Background").focusable().test_id("background")).child(
                col().layer(LayerPolicy::modal(), |_| {}).child(el().size(40).label("Modal anchor").focusable().overlay(
                    Placement::BELOW,
                    |_| true,
                    |_| el().size(40).label("Owned portal").focusable().test_id("portal"),
                )),
            )
        })
        .unwrap();
    let findings = audit(&mut driver, window);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].code, codes::DEAD_FOCUS_TARGET.id);
    assert_eq!(findings[0].node, target(&driver, window, "background"));
    assert!(findings[0].message.contains("outside the active modal"));
    assert_ne!(findings[0].node, target(&driver, window, "portal"));
}

struct DesignRule(&'static str);

impl AuditRule for DesignRule {
    fn check(&self, nodes: &[AuditNode<'_>], findings: &mut Vec<AuditFinding>) {
        findings.extend(nodes.iter().filter(|node| node.test_id.is_some()).map(|node| node.finding(self.0, Severity::Info, "Design rule")));
    }
}

#[test]
fn external_rules_run_in_registration_order_and_in_their_live_build_scopes() {
    let inspector = Inspector::default().with_audit_rule(Arc::new(DesignRule("APP_RULE")));
    let mut driver = driver().with_inspector(inspector);
    let shown = driver.app_mut().build(|cx| cx.signal(true)).unwrap();
    let window = driver
        .app_mut()
        .open_window(options(), move |_| {
            col().child(text("Outside").test_id("outside")).child(show(
                move |r| r.get(shown),
                |_| {
                    scoped(|cx| {
                        cx.provide::<Arc<dyn AuditRule>>(Arc::new(DesignRule("PANE_RULE")));
                        col().child(text("Pane").test_id("pane")).child(scoped(|cx| {
                            cx.provide::<Arc<dyn AuditRule>>(Arc::new(DesignRule("INNER_RULE")));
                            text("Inner").test_id("inner")
                        }))
                    })
                },
            ))
        })
        .unwrap();

    let outside = target(&driver, window, "outside");
    let pane = target(&driver, window, "pane");
    let inner = target(&driver, window, "inner");
    let first = audit(&mut driver, window);
    let emitted: Vec<_> = first.iter().map(|finding| (finding.code.as_str(), finding.node)).collect();
    assert_eq!(emitted, [("APP_RULE", outside), ("APP_RULE", pane), ("APP_RULE", inner), ("PANE_RULE", pane), ("INNER_RULE", inner)]);
    driver.app_mut().update(|w| w.set(shown, false)).unwrap();
    let remaining = audit(&mut driver, window);
    assert_eq!(remaining.len(), 1, "{remaining:?}");
    assert_eq!(remaining[0].node, outside);
    driver.app_mut().update(|w| w.set(shown, true)).unwrap();
    assert_eq!(audit(&mut driver, window).len(), first.len());
}

struct BinaryAudit(AgentRequest);

impl InspectTransport for BinaryAudit {
    fn exchange(&mut self, payload: &[u8], execute: &mut dyn FnMut(AgentRequest) -> AgentReceipt) -> Vec<u8> {
        assert_eq!(payload, [255, 0, 128]);
        let receipt = execute(self.0.clone());
        assert!(receipt.ok && receipt.progress.settled);
        vec![254, u8::try_from(receipt.findings.unwrap().len()).unwrap()]
    }
}

#[test]
fn custom_codec_dispatches_ordinary_requests_and_trace_records_redacted_entries() {
    let recording = Rc::new(RefCell::new(Replay::new([14, 52])));
    let inspector = Inspector::default()
        .with_transport(BinaryAudit(AgentRequest::new(None, AgentCommand::Audit)))
        .with_trace_recorder(recording.clone());
    let mut driver = driver().with_inspector(inspector);
    let window = driver.app_mut().open_window(options(), |_| el().size(20).role(Role::Button)).unwrap();
    assert_eq!(driver.exchange(&[255, 0, 128]), [254, 1]);
    driver.advance_to(Duration::from_millis(17)).unwrap();
    driver
        .dispatch(
            window,
            Input::Host(HostReply::ServiceReply {
                id: ServiceId(42),
                outcome: ServiceOutcome::Clipboard(vec![ClipboardItem::Files(vec!["/private/protected.txt".into()])]),
            }),
        )
        .unwrap();
    driver.execute(AgentRequest::new(Some(window), AgentCommand::Observe));

    let mut fresh = self::driver().with_inspector(Inspector::default().with_trace_recorder(recording.clone()));
    fresh.app_mut().open_window(options(), |_| el().size(20).role(Role::Button)).unwrap();
    let recording = recording.borrow();
    assert_eq!(fresh.replay(&recording).unwrap().len(), 3);
    let recorded = recording.entries();
    assert_eq!(recorded.len(), 3, "{recorded:?}");
    assert_eq!(recorded[0].at, Duration::ZERO);
    assert!(matches!(&recorded[0].event, Event::Agent(request) if request.command == AgentCommand::Audit));
    assert_eq!(recorded[1].at, Duration::from_millis(17));
    assert_eq!(recorded[1].event, Event::Redacted(Channel::Host));
    assert_eq!(recorded[2].at, recorded[1].at);
    assert!(matches!(&recorded[2].event, Event::Agent(request) if request.command == AgentCommand::Observe));
}

#[test]
fn full_built_in_sweep_stays_under_ten_milliseconds() {
    const ROWS: usize = 512;
    let mut driver = driver();
    let window = driver
        .app_mut()
        .open_window(options(), |_| {
            col()
                .children((0..ROWS).map(|index| {
                    row().child(text(format!("Row {index}"))).child(el().w(40).h(16).role(Role::Button).label("Details").focusable())
                }))
                .child(text("This label is long enough to be clipped").w(35).truncate())
                .child(el().size(18).role(Role::Button))
                .child(el().w(0).h(18).label("Empty").focusable())
        })
        .unwrap();
    let tree = driver.app().runtime().window(window).unwrap().tree();
    let count = tree.subtree(tree.root()).filter(|&node| tree.is_live(node)).count();
    assert_eq!(count, ROWS * 3 + 6);
    let expected = audit(&mut driver, window);
    assert_eq!(expected.len(), 3, "{expected:?}");
    for _ in 0..4 {
        black_box(audit(&mut driver, window));
    }

    let mut samples = [Duration::ZERO; 21];
    for sample in &mut samples {
        let start = Instant::now();
        let actual = black_box(audit(&mut driver, window));
        *sample = start.elapsed();
        assert_eq!(actual, expected);
    }
    samples.sort_unstable();
    let median = samples[samples.len() / 2];
    eprintln!(
        "built-in audit: {count} live nodes, 3 findings, 21 warm samples, median {median:?}, range {:?}–{:?}",
        samples[0], samples[20]
    );
    assert!(median < Duration::from_millis(10), "full audit took {median:?}");
}
