//! Content ownership is enforced before mutating either the retained tree or Taffy.

use super::*;
use std::panic::{AssertUnwindSafe, catch_unwind};

fn rejected(f: impl FnOnce()) {
    let error = catch_unwind(AssertUnwindSafe(f)).expect_err("invalid content accepted");
    let message = error.downcast_ref::<String>().map(String::as_str).or_else(|| error.downcast_ref::<&str>().copied()).unwrap();
    assert!(message.contains("ELEMENT_CONTENT"), "{message}");
}

fn set(k: &mut Kernel, text: NodeId, value: rich::Rich) {
    k.set_rich(text, value.as_str(), value.runs());
}

#[test]
fn content_is_fixed_from_creation_and_only_boxes_accept_children() {
    let mut k = Kernel::default();
    k.enable_log(true);
    let image = crate::image::Image::rgba(1, 1, vec![255; 4]);
    let icon = crate::icon::Icon::stroked("empty", &[]);
    let atomic = [
        (k.create_text(), Kind::Text),
        (k.create_input(), Kind::TextInput),
        (k.create_image(image, crate::image::ObjectFit::Fill), Kind::Image),
        (k.create_icon(&icon, None), Kind::Icon),
        (k.create_canvas(), Kind::Canvas),
        (k.create_native("map", crate::native::Slot::Underlay), Kind::Native),
    ];
    let created: Vec<_> = k
        .take_log()
        .into_iter()
        .filter_map(|op| match op.kind {
            OpKind::Create(kind) => Some((op.node, kind)),
            _ => None,
        })
        .collect();
    assert_eq!(created, atomic);
    let child = k.create_box();
    for (parent, kind) in atomic {
        assert_eq!(k.kind(parent), kind);
        rejected(|| k.insert_child(parent, child, 0));
        rejected(|| k.append_child(parent, child));
        assert!(k.children(parent).is_empty());
        assert!(k.taffy.children(k.node(parent).taffy).unwrap().is_empty());
        assert_eq!(k.parent(child), None);
    }
    let container = k.create_box();
    k.append_child(container, child);
    rejected(|| k.set_canvas(container, Vec::new()));
    let native = atomic[5].0;
    assert_eq!(k.native_nodes(), [native]);
    assert_eq!(k.native(native).unwrap().kind, "map");
    k.destroy(native);
    assert!(k.native_nodes().is_empty());
    assert_eq!(k.children(container), &[child]);
    assert_eq!(k.taffy.children(k.node(container).taffy).unwrap(), vec![k.node(child).taffy]);
}

#[test]
fn links_are_text_owned_and_retire_with_their_owner() {
    let mut k = Kernel::default();
    let baseline = k.live_nodes();
    let text = k.create_text();
    set(&mut k, text, rich::rich().styled("guide", |s| s.link("/guide").id("guide")));
    let link = k.children(text)[0];
    assert_eq!(k.parent(link), Some(text));
    assert!(k.taffy.children(k.node(text).taffy).unwrap().is_empty());
    assert_eq!(k.find("guide"), None);
    k.append_child(k.root(), text);
    assert_eq!(k.find("guide"), Some(link));
    assert_eq!(k.kind(link), Kind::Link);
    rejected(|| k.append_child(k.root(), link));
    rejected(|| k.move_child(link, 0));
    rejected(|| k.remove_child(link));
    rejected(|| k.destroy(link));
    rejected(|| {
        k.begin_exit(link);
    });
    rejected(|| k.set_canvas(link, Vec::new()));
    assert_eq!(k.children(text), &[link]);
    k.remove_child(text);
    assert_eq!(k.find("guide"), None);
    k.append_child(k.root(), text);
    assert_eq!(k.find("guide"), Some(link));
    k.destroy(text);
    assert_eq!(k.find("guide"), None);
    assert!(!k.is_alive(link));
    assert_eq!(k.live_nodes(), baseline);
}

#[test]
fn link_identity_follows_unchanged_ranges_and_targets_not_run_positions_or_ids() {
    let mut k = Kernel::default();
    let text = k.create_text();
    let guide = || rich::rich().styled("指南", |s| s.link("/guide").id("same"));
    set(&mut k, text, guide());
    let original = k.children(text)[0];
    // A new earlier run/link must not steal the existing link's identity.
    set(&mut k, text, rich::rich().link("新😀", "/new").styled("指南", |s| s.link("/guide").id("same").bold()));
    let first = k.children(text)[0];
    assert_ne!(first, original);
    assert_eq!(k.children(text)[1], original);
    set(&mut k, text, guide());
    assert_eq!(k.children(text), &[original]);
    assert!(!k.is_alive(first));
    set(&mut k, text, guide().text(" 后续"));
    assert_eq!(k.children(text), &[original]);
    set(&mut k, text, rich::rich().styled("指南", |s| s.link("/other").id("same")).text(" 后续"));
    let changed_target = k.children(text)[0];
    assert_ne!(changed_target, original);
    assert!(!k.is_alive(original));
    // Identical byte ranges/runs with different text are replacement, not a no-op.
    set(&mut k, text, rich::rich().styled("说明", |s| s.link("/other").id("same")).text(" 后续"));
    assert_ne!(k.children(text)[0], changed_target);
    assert!(!k.is_alive(changed_target));
}

#[test]
fn plain_replacement_and_splice_retire_annotations_but_append_preserves_them() {
    let mut k = Kernel::default();
    let text = k.create_text();
    k.append_child(k.root(), text);
    let rich = || rich::rich().styled("hello", |s| s.link("/hello").id("hello"));
    set(&mut k, text, rich());
    let link = k.find("hello").unwrap();
    k.append_text(text, " world");
    assert_eq!(k.find("hello"), Some(link));
    assert!(k.set_text(text, "hello world"));
    assert!(!k.is_alive(link));
    assert!(k.runs(text).is_empty());
    assert!(k.children(text).is_empty());
    set(&mut k, text, rich());
    let link = k.find("hello").unwrap();
    k.splice_text(text, 0, 5, "x");
    assert_eq!(k.text(text), "x");
    assert!(k.runs(text).is_empty());
    assert_eq!(k.find("hello"), None);
    assert!(!k.is_alive(link));
}

#[test]
fn invalid_run_ranges_are_rejected_before_text_or_link_mutation() {
    let mut k = Kernel::default();
    let text = k.create_text();
    set(&mut k, text, rich::rich().link("old", "/old"));
    let link = k.children(text)[0];
    for ranges in [vec![(0, 4)], vec![(1, 3)], vec![(0, 3), (0, 3)], vec![(3, 2)]] {
        let runs: Vec<_> = ranges.into_iter().map(|(start, end)| (start..end, rich::Span::default().link("/new"))).collect();
        rejected(|| {
            k.set_rich(text, "新", &runs);
        });
        assert_eq!(k.text(text), "old");
        assert_eq!(k.children(text), &[link]);
        assert_eq!(k.live_nodes(), 4);
    }
}

#[test]
fn a_cycle_is_rejected_before_mutating_the_tree() {
    let mut k = Kernel::default();
    let parent = k.create_box();
    let child = k.create_box();
    k.append_child(parent, child);
    assert!(catch_unwind(AssertUnwindSafe(|| k.append_child(child, parent))).is_err());
    assert_eq!(k.parent(parent), None);
    assert_eq!(k.children(parent), &[child]);
    assert!(k.children(child).is_empty());
}
