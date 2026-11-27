use octara_model::geometry::{Size, WindowViewport};
use octara_model::identity::TreeId;
use octara_model::style::{Length, StyleProp};
use octara_pipeline::NativeEngine;
use octara_runtime::HostEngine;
use octara_tree::tables::{AuthoredStyle, EditorSlot};
use octara_tree::{NodeChange, NodeContent, NodeSpec, Tree, TreeDelta};
use std::sync::Arc;

#[test]
fn failed_projection_preserves_published_geometry_and_warm_caches() {
    let mut tree = Tree::new(TreeId::new([61, 32], 1), Default::default());
    let mut delta = tree.initial_delta();
    let mut tx = tree.transaction();
    let root = tree.node_ref(tree.root());
    let editor = tx.create(NodeSpec::new(NodeContent::TextInput(Box::new(EditorSlot::new("Published editor value")))));
    tx.change(
        editor,
        NodeChange::Style(Box::new(AuthoredStyle {
            base: [StyleProp::Width(Length::Px(280.0)), StyleProp::Height(Length::Px(32.0))].into_iter().collect(),
            ..Default::default()
        })),
    );
    tx.place(root, 0, editor);
    let mut untouched = None;
    for row in 1..=512 {
        let label = tx.create(NodeSpec::new(NodeContent::Text(format!("Independent document {row}").into())));
        tx.place(root, row, label);
        untouched = Some(label);
    }
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    delta.merge(mounted);
    let editor = receipt.node(editor).unwrap();
    let untouched = receipt.node(untouched.unwrap()).unwrap();
    let viewport = WindowViewport { size: Size::new(480.0, 16_000.0), scale: 1.0, safe_area: None };
    let mut engine = NativeEngine::new(Default::default());
    engine.register_font(Arc::from(&include_bytes!("../assets/fonts/Inter-Regular.ttf")[..]), &[*b"Latn"]);
    let publication = engine.publication();
    engine.project(&tree, &delta, viewport, 0.0).unwrap();
    engine.commit();

    let published = publication.latest().unwrap();
    let published_editor = engine.text_geometry(&tree, editor).unwrap();
    let published_layout = engine.geometry(editor).unwrap();
    let retained = engine.text_geometry(&tree, untouched).unwrap();
    let initial_text = engine.text_work();
    let initial_layout = engine.layout_work();
    assert_eq!(initial_text.font_generation, 1);
    assert!(initial_text.shapes_built >= 513);
    assert!(initial_layout.frames >= 515);

    let mut tx = tree.transaction();
    tx.change(tree.node_ref(editor), NodeChange::EditorValue("Candidate value with different geometry".into()));
    let (_, edit) = tree.apply(tx.finish()).unwrap();
    engine.project(&tree, &edit, viewport, 0.1).unwrap();
    let candidate = engine.text_geometry(&tree, editor).unwrap();
    assert_ne!(candidate, published_editor);
    let invalid = WindowViewport { scale: 0.0, ..viewport };
    assert!(engine.project(&tree, &TreeDelta::default(), invalid, 0.1).is_err());
    let candidate_text = engine.text_work();
    let candidate_layout = engine.layout_work();
    assert_eq!(candidate_text.shapes_built - initial_text.shapes_built, 1);

    engine.rollback();
    let rolled_back = engine.text_geometry(&tree, editor).unwrap();
    assert_eq!(rolled_back, published_editor);
    assert!(Arc::ptr_eq(&rolled_back.lines, &published_editor.lines));
    assert_eq!(engine.geometry(editor), Some(published_layout));
    assert!(Arc::ptr_eq(&publication.latest().unwrap(), &published));
    assert_eq!(engine.text_work(), candidate_text);
    assert_eq!(engine.layout_work(), candidate_layout);

    engine.project(&tree, &edit, viewport, 0.2).unwrap();
    engine.commit();
    assert_eq!(publication.latest().unwrap().number, published.number + 1);
    assert_eq!(engine.text_geometry(&tree, editor).unwrap(), candidate);
    assert!(Arc::ptr_eq(&engine.text_geometry(&tree, untouched).unwrap().lines, &retained.lines));
    let retried_text = engine.text_work();
    let retried_layout = engine.layout_work();
    assert_eq!(retried_text.font_generation, initial_text.font_generation);
    assert_eq!(retried_text.shapes_built, candidate_text.shapes_built);
    assert_eq!(retried_text.shape_cache_hits - candidate_text.shape_cache_hits, 1);
    assert_eq!(retried_text.documents_built - candidate_text.documents_built, 1);
    assert_eq!(retried_layout.computes, initial_layout.computes);
    assert_eq!(retried_layout.frames - candidate_layout.frames, 1);
    eprintln!(
        "rollback: {} initial frames, font generation {}, retry: {} frames, {} Taffy computes, {} new shapes, {} cache hits",
        initial_layout.frames,
        retried_text.font_generation,
        retried_layout.frames - candidate_layout.frames,
        retried_layout.computes - candidate_layout.computes,
        retried_text.shapes_built - candidate_text.shapes_built,
        retried_text.shape_cache_hits - candidate_text.shape_cache_hits,
    );
}
