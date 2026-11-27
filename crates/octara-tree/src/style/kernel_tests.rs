use std::mem::size_of;
use std::sync::Arc;

use octara_model::style::{Direction, LayoutLength, LayoutStyle, State, StyleProp, Visual, When};
use octara_model::theme::{Breakpoint, ColorScheme, ColorToken, ResponsiveBreakpoint, Theme, ThemePatch, rgb};
use octara_model::window::SystemPreferences;

use crate::style::{ComputedStyle, sensitivity};
use crate::tables::AuthoredStyle;
use crate::{NodeChange, NodeContent, NodeFlags, NodeSpec, Retirement, RuntimeOp, Tree, TreeId};

fn authored(base: &[StyleProp], overrides: &[(When, Vec<StyleProp>)], group: Option<&'static str>) -> NodeChange {
    NodeChange::Style(Box::new(AuthoredStyle {
        base: base.iter().copied().collect(),
        overrides: overrides.iter().cloned().collect(),
        group,
    }))
}

#[test]
fn locale_script_overrides_the_language_default_and_survives_theme_changes() {
    let mut tree = Tree::new(TreeId::new([63, 91], 5), Theme::light());
    for (locale, direction) in [("ar", Direction::Rtl), ("ar-Latn", Direction::Ltr), ("en-Arab-u-nu-arab", Direction::Rtl)] {
        let prefs = SystemPreferences { locale: locale.into(), text_scale: 1.5, ..SystemPreferences::default() };
        let delta = tree.set_system_preferences(&prefs);
        assert!(delta.paint_dirty.contains(&tree.root()));
        assert_eq!(tree.visual(tree.root()).direction, direction);
        assert!(tree.set_system_preferences(&prefs).paint_dirty.is_empty());
    }
    tree.apply_runtime(RuntimeOp::ThemeChanged(Box::new(Theme::dark())));
    let visual = tree.visual(tree.root());
    assert_eq!(visual.direction, Direction::Rtl);
    assert_eq!(visual.font_size, 24.0);
}

#[test]
fn sparse_interaction_and_theme_changes_prune_over_ninety_percent_of_subtrees() {
    let mut tree = Tree::new(TreeId::new([63, 91], 1), Theme::light());
    let mut tx = tree.transaction();
    let group = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(group, authored(&[], &[], Some("sample")));
    tx.place(tree.node_ref(tree.root()), 0, group);
    for index in 0..100 {
        let branch = tx.create(NodeSpec::new(NodeContent::Box));
        tx.place(group, index, branch);
        for child in 0..10 {
            let leaf = tx.create(NodeSpec::new(NodeContent::Box));
            tx.place(branch, child, leaf);
        }
    }
    let target = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(target, NodeChange::Flags(NodeFlags::FOCUSABLE));
    tx.change(
        target,
        authored(
            &[StyleProp::Background(Some(ColorToken::Accent.into()))],
            &[
                (When::GroupHover("sample"), vec![StyleProp::Width(42.into())]),
                (When::Screen(Breakpoint::Medium), vec![StyleProp::Height(30.into())]),
                (When::Responsive(ResponsiveBreakpoint::Sm), vec![StyleProp::MinHeight(50.into())]),
                (When::Touch, vec![StyleProp::MaxHeight(90.into())]),
                (When::State(State::FOCUS_WITHIN), vec![StyleProp::Opacity(0.5)]),
                (When::Dark, vec![StyleProp::BorderColor(ColorToken::Danger.into())]),
            ],
            None,
        ),
    );
    tx.place(group, 100, target);
    let (receipt, _) = tree.apply(tx.finish()).unwrap();
    let [group, target] = [group, target].map(|node| receipt.node(node).unwrap());
    let full_visit = tree.subtree(group).count();
    let background = rgb(0x559123);
    let mut counts = Vec::new();

    for op in [
        RuntimeOp::SetState { node: group, state: State::HOVER, on: true },
        RuntimeOp::SetViewportContext { breakpoint: Breakpoint::Medium, width: 650.0, touch: true },
        RuntimeOp::SetFocus { node: Some(target), visible: true },
        RuntimeOp::ThemeChanged(Box::new(Theme::light().color(ColorToken::Accent, background))),
    ] {
        tree.style_visits = 0;
        tree.style_prunes = 0;
        tree.apply_runtime(op);
        assert!(tree.style_visits < full_visit / 10);
        counts.push((tree.style_visits, tree.style_prunes));
    }
    assert!(counts[0].1 > 90);
    assert!(counts[1].1 > 90);
    assert!(counts[3].1 > 90);
    assert_eq!(tree.layout_style(target).size.width, Some(LayoutLength::Px(42.0)));
    assert_eq!(tree.layout_style(target).size.height, Some(LayoutLength::Px(30.0)));
    assert_eq!(tree.layout_style(target).min_size.height, Some(LayoutLength::Px(50.0)));
    assert_eq!(tree.layout_style(target).max_size.height, Some(LayoutLength::Px(90.0)));
    assert_eq!(tree.visual(target).opacity, 0.5);
    assert_eq!(tree.visual(target).background, Some(background));

    tree.style_visits = 0;
    tree.style_prunes = 0;
    let mut tx = tree.transaction();
    tx.change(tree.node_ref(group), NodeChange::ThemePatch(Some(Box::new(ThemePatch::default().scheme(ColorScheme::Dark)))));
    tree.apply(tx.finish()).unwrap();
    counts.push((tree.style_visits, tree.style_prunes));
    assert!(tree.style_visits < full_visit / 10);
    assert!(tree.style_prunes > 90);
    assert_eq!(tree.visual(target).border, tree.theme().get(ColorToken::Danger));
    println!(
        "Style traversal baseline {full_visit} nodes in the group. Hover, viewport, focus, root token and scoped scheme (visited, pruned): {counts:?}"
    );
}

#[test]
fn bloom_tracks_reparenting_style_replacement_and_token_references_inside_patches() {
    const SOURCE: ColorToken = ColorToken::custom("source");
    const TARGET: ColorToken = ColorToken::custom("target");
    let mut tree = Tree::new(TreeId::new([63, 91], 2), Theme::light());
    let mut tx = tree.transaction();
    let before = tx.create(NodeSpec::new(NodeContent::Box));
    let after = tx.create(NodeSpec::new(NodeContent::Box));
    let styled = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(before, authored(&[], &[], Some("row")));
    tx.change(after, authored(&[], &[], Some("row")));
    tx.change(
        styled,
        authored(&[StyleProp::Background(Some(TARGET.into()))], &[(When::GroupHover("row"), vec![StyleProp::Width(63.into())])], None),
    );
    tx.change(after, NodeChange::ThemePatch(Some(Box::new(ThemePatch::default().color(TARGET, SOURCE)))));
    let root = tree.node_ref(tree.root());
    tx.place(root, 0, before);
    tx.place(root, 1, after);
    tx.place(before, 0, styled);
    let (receipt, _) = tree.apply(tx.finish()).unwrap();
    let [before, after, styled] = [before, after, styled].map(|node| receipt.node(node).unwrap());
    assert_ne!(tree.nodes[before].style_bloom & sensitivity::group("row"), 0);

    let mut tx = tree.transaction();
    tx.place(tree.node_ref(after), 0, tree.node_ref(styled));
    tree.apply(tx.finish()).unwrap();
    assert_eq!(tree.nodes[before].style_bloom & sensitivity::group("row"), 0);
    assert_ne!(tree.nodes[after].style_bloom & sensitivity::group("row"), 0);
    tree.apply_runtime(RuntimeOp::SetState { node: after, state: State::HOVER, on: true });
    assert_eq!(tree.layout_style(styled).size.width, Some(LayoutLength::Px(63.0)));

    let red = rgb(0xff0011);
    tree.apply_runtime(RuntimeOp::ThemeChanged(Box::new(Theme::light().color(SOURCE, red))));
    assert_eq!(tree.theme_at(styled).try_get(TARGET), Some(red));
    assert_eq!(tree.visual(styled).background, Some(red));

    let mut tx = tree.transaction();
    tx.change(tree.node_ref(styled), authored(&[], &[], None));
    tree.apply(tx.finish()).unwrap();
    assert_eq!(tree.nodes[after].style_bloom & sensitivity::group("row"), 0);
    assert_eq!(tree.visual(styled).background, None);
}

#[test]
fn inherited_mutations_cross_unstyled_nodes_and_stop_at_explicit_overrides() {
    let mut tree = Tree::new(TreeId::new([63, 91], 4), Theme::light());
    let mut tx = tree.transaction();
    let parent = tx.create(NodeSpec::new(NodeContent::Box));
    let passthrough = tx.create(NodeSpec::new(NodeContent::Box));
    let blocked = tx.create(NodeSpec::new(NodeContent::Box));
    let inherited = tx.create(NodeSpec::new(NodeContent::Text("inherited".into())));
    let fixed = tx.create(NodeSpec::new(NodeContent::Text("fixed".into())));
    let active = rgb(0x995522);
    let permanent = rgb(0x446688);
    tx.change(parent, authored(&[], &[(When::State(State::HOVER), vec![StyleProp::TextColor(active.into())])], None));
    tx.change(blocked, authored(&[StyleProp::TextColor(permanent.into())], &[], None));
    tx.place(tree.node_ref(tree.root()), 0, parent);
    tx.place(parent, 0, passthrough);
    tx.place(parent, 1, blocked);
    tx.place(passthrough, 0, inherited);
    tx.place(blocked, 0, fixed);
    let (receipt, _) = tree.apply(tx.finish()).unwrap();
    let [parent, inherited, fixed] = [parent, inherited, fixed].map(|node| receipt.node(node).unwrap());

    let delta = tree.apply_runtime(RuntimeOp::SetState { node: parent, state: State::HOVER, on: true });
    assert_eq!(tree.visual(inherited).text, active);
    assert_eq!(tree.visual(fixed).text, permanent);
    assert!(delta.paint_dirty.contains(&inherited));
    assert!(!delta.paint_dirty.contains(&fixed));
}

#[test]
fn equivalent_computed_pairs_share_and_differing_inherited_tokens_remain_local() {
    const ROWS: usize = 1_000;
    const TOKEN: ColorToken = ColorToken::custom("row-text");
    let original = rgb(0x777799);
    let mut tree = Tree::new(TreeId::new([63, 91], 3), Theme::light().color(TOKEN, original));
    let mut tx = tree.transaction();
    let root = tree.node_ref(tree.root());
    let mut rows = Vec::with_capacity(ROWS);
    for index in 0..ROWS {
        let row = tx.create(NodeSpec::new(NodeContent::Box));
        let text = if index == 0 { TOKEN.into() } else { original.into() };
        tx.change(row, authored(&[StyleProp::TextColor(text), StyleProp::Width(36.into())], &[], None));
        tx.place(root, index, row);
        rows.push(row);
    }
    let (receipt, _) = tree.apply(tx.finish()).unwrap();
    let rows: Vec<_> = rows.into_iter().map(|node| receipt.node(node).unwrap()).collect();
    let first = tree.style(rows[0]).unwrap();
    assert_ne!(first.inherited.text, tree.style(rows[1]).unwrap().inherited.text);
    assert!(rows.iter().all(|&row| Arc::ptr_eq(&first.resolved, &tree.style(row).unwrap().resolved)));
    assert_eq!(tree.style_interner.len(), 2, "one row pair and the initial tree pair");

    let before = ROWS * size_of::<(LayoutStyle, Visual)>();
    let after = ROWS * size_of::<Arc<ComputedStyle>>() + size_of::<ComputedStyle>() + 2 * size_of::<usize>();
    println!("Computed style payload for {ROWS} rows: {ROWS} pairs/{before} bytes inline versus 1 pair and handles/{after} bytes");
    assert!(before > after * 10);

    let changed = rgb(0x000012);
    let delta = tree.apply_runtime(RuntimeOp::ThemeChanged(Box::new(Theme::light().color(TOKEN, changed))));
    assert_eq!(tree.visual(rows[0]).text, changed);
    assert_eq!(tree.visual(rows[1]).text, original);
    assert!(!Arc::ptr_eq(&tree.style(rows[0]).unwrap().resolved, &tree.style(rows[1]).unwrap().resolved));
    assert!(delta.paint_dirty.contains(&rows[0]));
    assert!(!delta.paint_dirty.contains(&rows[1]));
    assert_eq!(tree.style_interner.len(), 3);

    let mut tx = tree.transaction();
    tx.retire(tree.node_ref(rows[0]), Retirement::Destroy);
    tree.apply(tx.finish()).unwrap();
    assert_eq!(tree.style_interner.len(), 2);
}
