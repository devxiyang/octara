//! Consumer tests for the public value and wire formats, using only `octara-model`.

use octara_model::capture::{FrameReceipt, PaintSample};
use octara_model::diagnostics::{Diagnostic, Severity, codes};
use octara_model::identity::{FrameStamp, NodeId, TargetGuard, TreeId};
use octara_model::input::{DroppedFile, HostReply};
use octara_model::module::{AirspacePolicy, HostLifecycleEvent, ModuleError, ModuleId};
use octara_model::native::{NativeSnapshot, NativeSnapshotRequest};
use octara_model::paint::Picture;
use octara_model::text::TextDocument;
use serde_json::{json, to_value};
use slotmap::SlotMap;
use std::time::Duration;

#[test]
fn module_protocol_preserves_static_ids_and_owned_event_payloads() {
    const MODULE_WIRE: &str = r#""octara.example""#;
    let id = ModuleId("octara.example");
    assert_eq!(serde_json::to_string(&id).unwrap(), MODULE_WIRE);
    assert_eq!(serde_json::from_str::<ModuleId>(MODULE_WIRE).unwrap(), id);

    let event = HostLifecycleEvent::AppLaunched { urls: vec!["example://open".into()], files: vec!["/workspace/local/notes.md".into()] };
    let wire = to_value(&event).unwrap();
    assert_eq!(wire, json!({"app_launched": {"urls": ["example://open"], "files": ["/workspace/local/notes.md"]}}));
    assert_eq!(serde_json::from_value::<HostLifecycleEvent>(wire).unwrap(), event);

    let error = ModuleError::InvalidArgs("missing index".into());
    let wire = to_value(&error).unwrap();
    assert_eq!(wire, json!({"invalid_args": "missing index"}));
    assert_eq!(serde_json::from_value::<ModuleError>(wire).unwrap(), error);
    assert_eq!(to_value(AirspacePolicy::default()).unwrap(), "underlay_preferred");
}

#[test]
fn file_drop_preserves_its_guard_and_distinguishes_paths_from_bare_names() {
    let mut ids = SlotMap::<NodeId, ()>::with_key();
    let stale = ids.insert(());
    ids.remove(stale);
    let tree = TreeId::new([10, 20], 3).next_generation().unwrap();
    let target = TargetGuard::for_node(tree, ids.insert(()));
    let reply = HostReply::FilesDropped {
        target,
        files: vec![DroppedFile::Path("/workspace/local/notes.md".into()), DroppedFile::Name("図.png".into())],
    };
    let wire = to_value(&reply).unwrap();
    assert_eq!(wire["files_dropped"]["files"], json!([{ "path": "/workspace/local/notes.md" }, { "name": "図.png" }]));
    assert_eq!(serde_json::from_value::<HostReply>(wire).unwrap(), reply);
    assert_ne!(target, TargetGuard::for_node(tree, stale));
    let debug = format!("{reply:?}");
    assert!(!debug.contains("notes.md") && !debug.contains("図.png"));
}

#[test]
fn native_snapshot_rejects_truncated_pixels_on_the_wire() {
    let tree = TreeId::new([10, 20], 3);
    let node = SlotMap::<NodeId, ()>::with_key().insert(());
    let request = NativeSnapshotRequest { tree, id: node.into(), request: 5 };
    let picture = Picture::new(2, 1, vec![20, 30, 40, 50, 60, 70, 80, 90]).unwrap();
    let snapshot = HostReply::NativeSnapshot { request, snapshot: Ok(NativeSnapshot { picture, misses: vec![] }) };
    let mut wire = to_value(&snapshot).unwrap();
    assert_eq!(serde_json::from_value::<HostReply>(wire.clone()).unwrap(), snapshot);
    wire["native_snapshot"]["snapshot"]["Ok"]["picture"]["rgba"] = json!([20, 30, 40, 50]);
    assert!(serde_json::from_value::<HostReply>(wire).is_err());
}

#[test]
fn frame_receipts_preserve_the_sample_and_delivery_generations() {
    let tree = TreeId::new([10, 20], 3);
    let receipt = FrameReceipt {
        stamp: FrameStamp {
            tree,
            frame: 9,
            surface_generation: 8,
            device_generation: 7,
            native_rev: 6,
            sample: PaintSample { revision: 5, sampled_at: Duration::from_micros(4_321) },
        },
        gpu_complete: true,
        presented_at_ns: None,
    };
    let round_trip: FrameReceipt = serde_json::from_value(to_value(receipt).unwrap()).unwrap();
    assert_eq!(round_trip, receipt);
}

#[test]
fn document_and_diagnostic_decoders_enforce_their_public_values() {
    let document = TextDocument::new("é🎨".into(), 6, 0);
    let mut wire = to_value(&document).unwrap();
    assert_eq!(serde_json::from_value::<TextDocument>(wire.clone()).unwrap().selection_endpoints(), Some((6, 0)));
    wire["caret"] = json!(1);
    assert!(serde_json::from_value::<TextDocument>(wire).is_err());

    let mut wire = to_value(Diagnostic::new(&codes::INPUT_REJECTED, "stale receiver")).unwrap();
    let diagnostic: Diagnostic = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(diagnostic.code, &codes::INPUT_REJECTED);
    assert_eq!(diagnostic.severity(), Severity::Warning);
    wire["code"] = json!("UNREGISTERED_CODE");
    assert!(serde_json::from_value::<Diagnostic>(wire).is_err());
}
