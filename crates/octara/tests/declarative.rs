use octara::declarative::{SCENE_FORMAT, ScenePacket};
use octara::model::geometry::{LayoutBox, Point, WindowViewport};
use octara::model::identity::TreeId;
use octara::model::input::ContactTarget;
use octara::model::semantics::A11yAction;
use octara::model::style::FlexDirection;
use octara::model::theme::Color;
use octara::prelude::{App, AppDefinition, WindowDefinition, WindowOptions, WriteCx};
use octara::tree::tables::{HandlerId, HandlerKind};
use octara::tree::{NodeId, Tree, TreeDelta, TreeRev, transaction::TreeError};
use octara::{CommitOutput, HostEngine, HostReply, Input, ProjectError, ProjectFeedback};
use octara_release::{Check, Policy, ReleaseProvider, ReleaseTarget, Store, Version, public_key, signing};
use serde_json::json;
use std::io::{self, Read};

struct TestHost;

impl HostEngine for TestHost {
    fn project(&mut self, _: &Tree, _: &TreeDelta, _: WindowViewport, _: f64) -> Result<ProjectFeedback, ProjectError> {
        Ok(ProjectFeedback::default())
    }

    fn commit(&mut self) -> CommitOutput {
        CommitOutput { frame: Some(1), ..Default::default() }
    }

    fn rollback(&mut self) {}

    fn hit_test(&self, _: &Tree, _: Point) -> Option<ContactTarget> {
        None
    }

    fn geometry(&self, _: NodeId) -> Option<LayoutBox> {
        None
    }
}

struct Delivery<'a>(Version, &'a [u8]);

impl ReleaseProvider for Delivery<'_> {
    fn current(&self, _: &str) -> io::Result<Option<Version>> {
        Ok(Some(self.0))
    }

    fn fetch(&self, _: &str, version: Version) -> io::Result<Box<dyn Read + '_>> {
        assert_eq!(version, self.0);
        Ok(Box::new(self.1))
    }
}

fn tree() -> Tree {
    Tree::new(TreeId::new([13, 17], 1), Default::default())
}

#[test]
fn verified_release_payload_compiles_to_an_authored_transaction() {
    let directory = tempfile::tempdir().unwrap();
    let (key, public) = signing::generate_key().unwrap();
    let target = ReleaseTarget {
        app: "octara-scene-test".into(),
        channel: "local".into(),
        platform: "test".into(),
        runtime: "octara/2".into(),
        resource_schema: SCENE_FORMAT.into(),
        state_schema: "test-state/1".into(),
    };
    let payload = json!({
        "format": SCENE_FORMAT,
        "palette": {"accent": [0, 153, 255, 255]},
        "nodes": [{
            "content": {"kind": "box"},
            "style": {"width": {"fraction": 0.5}, "direction": "column", "background": "accent"},
            "children": [{"content": {"kind": "text", "value": "Hello"}, "test_id": "message"}],
            "press": "select",
            "test_id": "card",
            "role": "button"
        }]
    });
    let input = serde_json::to_vec(&json!({"format": "octara.resource.v1", "sequence": 2, "target": target, "payload": payload})).unwrap();
    let bytes = signing::sign(&key, input.as_slice()).unwrap();
    let public_key = public_key(&public).unwrap();
    let published = signing::publish(&directory.path().join("origin"), public_key, bytes.as_slice()).unwrap();
    let store = Store::new(directory.path().join("store"), Policy { target, public_key, embedded_sequence: 1 });
    let decode = |payload: &str| ScenePacket::parse(payload).map_err(io::Error::other);
    let bind = |name: &str| (name == "select").then_some(HandlerId(10));

    let checked = store
        .check(&Delivery(published.version, &bytes), |payload| {
            let mut validation = tree();
            let compiled = decode(payload)?.compile(&validation, validation.root(), bind).map_err(io::Error::other)?;
            validation.apply(compiled).map_err(|error| io::Error::other(format!("scene preflight rejected {:?}", error.reason)))?;
            Ok(())
        })
        .unwrap();
    assert_eq!(checked, Check::Staged(published.version));
    let startup = store.start(decode).unwrap();
    let (version, packet) = startup.selected.unwrap();

    let mut app = App::new(|_, _| Box::new(TestHost));
    let selected = app.build(|build| build.signal(0)).unwrap();
    let view = packet
        .into_view(|name| (name == "select").then_some(move |write: &mut WriteCx<'_>| write.update(selected, |count| *count += 1)))
        .unwrap();
    let windows =
        AppDefinition::new().window(WindowDefinition::new(WindowOptions::default(), move |_| view)).launch(&mut app, None).unwrap();
    let window = app.window(windows[0]).unwrap();
    let runtime = window.runtime();
    let root = runtime.tree();
    assert_eq!(runtime.transaction_count(), 1);
    assert!(runtime.validate_handler_integrity(app.runtime().world()));
    let card = root.test_ids("card")[0];
    assert_eq!(root.layout_style(card).flex_direction, FlexDirection::Column);
    assert_eq!(root.visual(card).background, Some(Color::new([0.0, 0.6, 1.0, 1.0])));
    assert!(root.handler(card, HandlerKind::Press).is_some());
    assert_eq!(root.commands(card).unwrap().commands[0].0.as_ref(), "select");
    assert_eq!(root.text(root.test_ids("message")[0]).unwrap().text.as_ref(), "Hello");
    let target = root.guard(card);
    assert_eq!(runtime.last_commit().unwrap().frame, Some(1));
    assert_eq!(startup.session.confirm().unwrap(), Some(version));
    assert!(app.dispatch(windows[0], Input::Host(HostReply::AccessibilityAction { target, action: A11yAction::Press })).unwrap().handled);
    assert_eq!(app.update(|write| write.get(selected)).unwrap(), 1);
}

#[test]
fn packet_uses_the_retained_trees_content_preflight() {
    let packet = ScenePacket::parse(
        r#"{
        "format": "octara.scene/2",
        "nodes": [{
            "content": {"kind": "text", "value": "cannot contain children"},
            "children": [{"content": {"kind": "box"}}]
        }]
    }"#,
    )
    .unwrap();
    let mut tree = tree();
    let invalid = packet.compile(&tree, tree.root(), |_| None).unwrap();
    let rejected = tree.apply(invalid).unwrap_err();
    assert!(matches!(rejected.reason, TreeError::WrongContent(_)));
    assert_eq!(tree.revision(), TreeRev(0));
    assert_eq!(tree.nodes().count(), 2);
}
