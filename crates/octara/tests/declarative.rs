use octara::declarative::{SCENE_FORMAT, ScenePacket};
use octara::model::identity::TreeId;
use octara::model::style::FlexDirection;
use octara::model::theme::Color;
use octara::tree::tables::{HandlerId, HandlerKind};
use octara::tree::{Tree, TreeRev, transaction::TreeError};
use octara_release::{Policy, ReleaseTarget, Store, public_key, signing};
use serde_json::json;
use std::io;

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
            "test_id": "card"
        }]
    });
    let input = serde_json::to_vec(&json!({"format": "octara.resource.v1", "sequence": 2, "target": target, "payload": payload})).unwrap();
    let bytes = signing::sign(&key, input.as_slice()).unwrap();
    let store = Store::new(directory.path(), Policy { target, public_key: public_key(&public).unwrap(), embedded_sequence: 1 });
    let decode = |payload: &str| ScenePacket::parse(payload).map_err(io::Error::other);
    let bind = |name: &str| (name == "select").then_some(HandlerId(10));

    store
        .stage(bytes.as_slice(), |payload| {
            let mut validation = tree();
            let compiled = decode(payload)?.compile(&validation, validation.root(), bind).map_err(io::Error::other)?;
            validation.apply(compiled).map_err(|error| io::Error::other(format!("scene preflight rejected {:?}", error.reason)))?;
            Ok(())
        })
        .unwrap();
    let startup = store.start(decode).unwrap();
    let (version, packet) = startup.selected.unwrap();
    let mut root = tree();
    let compiled = packet.compile(&root, root.root(), bind).unwrap();
    let (receipt, delta) = root.apply(compiled).unwrap();
    assert_eq!(receipt.revision, TreeRev(1));
    assert_eq!(delta.created.len(), 2);
    let card = root.test_ids("card")[0];
    assert_eq!(root.layout_style(card).flex_direction, FlexDirection::Column);
    assert_eq!(root.visual(card).background, Some(Color::new([0.0, 0.6, 1.0, 1.0])));
    assert_eq!(root.handler(card, HandlerKind::Press), Some(HandlerId(10)));
    assert_eq!(root.text(root.test_ids("message")[0]).unwrap().text.as_ref(), "Hello");
    assert_eq!(startup.session.confirm().unwrap(), Some(version));
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
