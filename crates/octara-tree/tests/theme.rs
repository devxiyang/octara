use octara_model::style::{FontSize, Length, LineHeight, Radius, StyleProp, When};
use octara_model::theme::{Color, ColorScheme, ColorToken, Fill, FontSizeToken, RadiusToken, TextScale, Theme, ThemePatch, rgb};
use octara_tree::{NodeChange, NodeContent, NodeSpec, RuntimeOp, Tree, TreeId, tables::AuthoredStyle};

fn scope(patch: ThemePatch) -> NodeChange {
    NodeChange::ThemePatch(Some(Box::new(patch)))
}

fn accent_style() -> NodeChange {
    NodeChange::Style(Box::new(AuthoredStyle {
        base: [
            StyleProp::PaddingLeft(Length::Spacing(2.0)),
            StyleProp::RadiusTopLeft(Radius::Token(RadiusToken::Md)),
            StyleProp::RadiusTopRight(Radius::Token(RadiusToken::Md)),
            StyleProp::RadiusBottomRight(Radius::Token(RadiusToken::Md)),
            StyleProp::RadiusBottomLeft(Radius::Token(RadiusToken::Md)),
            StyleProp::Background(Some(Fill::Solid(ColorToken::Accent.alpha(0.5)))),
        ]
        .into_iter()
        .collect(),
        overrides: [
            (When::Dark, vec![StyleProp::TextColor(ColorToken::Info.into())]),
            (When::Light, vec![StyleProp::TextColor(ColorToken::Text.into())]),
        ]
        .into_iter()
        .collect(),
        ..AuthoredStyle::default()
    }))
}

#[test]
fn scope_changes_follow_subtrees_and_keep_replacement_themes_local() {
    let mut tree = Tree::new(TreeId::new([7, 1], 1), Theme::light());
    let root = tree.node_ref(tree.root());
    let mut tx = tree.transaction();
    let parent = tx.create(NodeSpec::new(NodeContent::Box));
    let child = tx.create(NodeSpec::new(NodeContent::Box));
    let nested = tx.create(NodeSpec::new(NodeContent::Box));
    let replacement = tx.create(NodeSpec::new(NodeContent::Box));
    let sibling = tx.create(NodeSpec::new(NodeContent::Box));
    let red = rgb(0xb62125);
    tx.change(
        parent,
        scope(ThemePatch::default().scheme(ColorScheme::Dark).color(ColorToken::Accent, red).spacing(8.0).radius(RadiusToken::Md, 12.0)),
    );
    tx.change(nested, scope(ThemePatch::default().spacing(2.0)));
    tx.change(replacement, scope(ThemePatch::scope(Theme::light().spacing(3.0))));
    for node in [child, nested, replacement, sibling] {
        tx.change(node, accent_style());
    }
    tx.place(root, 0, parent);
    tx.place(root, 1, sibling);
    tx.place(parent, 0, child);
    tx.place(parent, 1, nested);
    tx.place(parent, 2, replacement);
    let (receipt, _) = tree.apply(tx.finish()).unwrap();
    let [parent, child, nested, replacement, sibling] =
        [parent, child, nested, replacement, sibling].map(|node| receipt.node(node).unwrap());

    assert_eq!(tree.theme_at(child).scheme, ColorScheme::Dark);
    assert_eq!(tree.visual(child).text, tree.theme_at(parent).get(ColorToken::Info));
    assert_eq!(tree.visual(child).background, Some(red.with_alpha(0.5)));
    assert_eq!(tree.visual(child).radius, [12.0; 4]);
    assert_eq!(tree.layout_style(child).padding.left, Length::Px(16.0).resolved(tree.theme()).unwrap());
    assert_eq!(tree.theme_at(nested).get(ColorToken::Accent), red);
    assert_eq!(tree.theme_at(nested).spacing, 2.0);
    assert_eq!(tree.theme_at(replacement).scheme, ColorScheme::Light);
    assert_eq!(tree.theme_at(replacement).spacing, 3.0);
    assert!(std::ptr::eq(tree.theme_at(sibling), tree.theme()));

    let mut tx = tree.transaction();
    tx.change(tree.node_ref(parent), NodeChange::ThemePatch(None));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    assert!(std::ptr::eq(tree.theme_at(child), tree.theme()));
    assert_eq!(tree.visual(child).text, tree.theme().get(ColorToken::Text));
    assert_eq!(tree.visual(child).radius, [6.0; 4]);
    assert_eq!(tree.theme_at(nested).scheme, ColorScheme::Light);
    assert_eq!(tree.theme_at(nested).spacing, 2.0);
    assert!(delta.paint_dirty.contains(&child));
    assert!(delta.paint_dirty.contains(&nested));
    assert!(!delta.paint_dirty.contains(&replacement));
    assert!(!delta.paint_dirty.contains(&sibling));

    let mut tx = tree.transaction();
    tx.place(tree.node_ref(replacement), 0, tree.node_ref(nested));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    assert_eq!(tree.theme_at(nested).spacing, 2.0);
    assert_eq!(tree.theme_at(nested).scheme, ColorScheme::Light);
    assert!(!delta.paint_dirty.contains(&nested));

    let blue = rgb(0x2255aa);
    let dark = Theme::dark().color(ColorToken::Accent, blue);
    tree.apply_runtime(RuntimeOp::ThemeChanged(Box::new(dark)));
    assert_eq!(tree.visual(child).background, Some(blue.with_alpha(0.5)));
    assert_eq!(tree.visual(sibling).background, Some(blue.with_alpha(0.5)));
    assert_eq!(tree.visual(nested).background, Some(Theme::light().get(ColorToken::Accent).with_alpha(0.5)));
}

#[test]
fn token_typography_inherits_and_resolves_inside_the_current_scope() {
    let mut tree = Tree::new(TreeId::new([7, 1], 2), Theme::light().line_height(1.5));
    let root = tree.node_ref(tree.root());
    let mut tx = tree.transaction();
    let parent = tx.create(NodeSpec::new(NodeContent::Box));
    let token = tx.create(NodeSpec::new(NodeContent::Text("token".into())));
    let numeric = tx.create(NodeSpec::new(NodeContent::Text("numeric".into())));
    let explicit = tx.create(NodeSpec::new(NodeContent::Text("explicit".into())));
    let scoped = tx.create(NodeSpec::new(NodeContent::Text("scoped".into())));
    let style = |declarations: &[StyleProp]| {
        NodeChange::Style(Box::new(AuthoredStyle { base: declarations.iter().copied().collect(), ..AuthoredStyle::default() }))
    };
    tx.change(parent, style(&[StyleProp::FontSize(FontSizeToken::Sm.into())]));
    tx.change(numeric, style(&[StyleProp::FontSize(FontSize::Len(Length::Px(14.0)))]));
    tx.change(explicit, style(&[StyleProp::LineHeight(LineHeight::Relative(2.0)), StyleProp::FontSize(FontSizeToken::Sm.into())]));
    tx.change(scoped, scope(ThemePatch::scope(Theme::dark().text_scale(FontSizeToken::Sm, TextScale::new(13.0, 19.0)))));
    tx.place(root, 0, parent);
    for (index, node) in [token, numeric, explicit, scoped].into_iter().enumerate() {
        tx.place(parent, index, node);
    }
    let (receipt, _) = tree.apply(tx.finish()).unwrap();
    let [token, numeric, explicit, scoped] = [token, numeric, explicit, scoped].map(|node| tree.visual(receipt.node(node).unwrap()));
    assert_eq!((token.font_size, token.line_height), (14.0, 20.0));
    assert_eq!((numeric.font_size, numeric.line_height), (14.0, 21.0));
    assert_eq!((explicit.font_size, explicit.line_height), (14.0, 28.0));
    assert_eq!((scoped.font_size, scoped.line_height), (13.0, 19.0));
    assert_ne!(token.text, Color::TRANSPARENT);
}
