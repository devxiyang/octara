//! What people and agents point at ([core](../../../../docs/design/core.md)): numbered marks,
//! the element at a point, and why an element is the size it is.

use super::{Kernel, Kind, NodeId};
use crate::inspect::{Ancestor, AxisExplanation, LayoutExplanation, ParentLayout, Picked, snake, tenth};

impl Kernel {
    /// Where a node was created (`file:line:column`).
    pub(crate) fn source_of(&self, id: NodeId) -> Option<String> {
        self.node(id).origin.map(|l| format!("{}:{}:{}", crate::diagnostics::source_file(l), l.line(), l.column()))
    }

    /// Whether a marked capture numbers the node: something can be done to it, or a component
    /// starts at it.
    fn markable(&self, id: NodeId) -> bool {
        id != self.root && (!self.node_actions(id).is_empty() || self.starts_component(id))
    }

    /// Whether a `#[component]` starts at the node: its parent was built by another one.
    pub(crate) fn starts_component(&self, id: NodeId) -> bool {
        let n = self.node(id);
        n.component.is_some() && n.parent.is_none_or(|p| self.node(p).component != n.component)
    }

    /// The marked elements in number order (`marks()[0]` is #1): depth first through the tree
    /// the inspector shows.
    pub fn marks(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![self.root];
        while let Some(id) = stack.pop() {
            if self.markable(id) {
                out.push(id);
            }
            stack.extend(self.node(id).children.iter().rev().copied().filter(|c| self.shown_to_a11y(*c)));
        }
        out
    }

    /// Each marked node's number.
    pub(crate) fn mark_numbers(&self) -> rustc_hash::FxHashMap<NodeId, u32> {
        self.marks().into_iter().enumerate().map(|(i, n)| (n, i as u32 + 1)).collect()
    }

    /// The topmost element at `(x, y)` (logical window pixels), with what surrounds it.
    pub fn pick(&self, x: f32, y: f32) -> Option<Picked> {
        let hit = self.hit_test(x, y)?;
        let marks = self.mark_numbers();
        let node = self.inspect_one(hit, marks.get(&hit).copied());
        let mut ancestors = Vec::new();
        let mut p = self.node(hit).parent;
        while let Some(id) = p {
            if id == self.root {
                break;
            }
            if id != self.overlay_layer {
                let one = self.inspect_one(id, marks.get(&id).copied());
                ancestors.push(Ancestor { role: one.role, id: one.id, mark: one.mark, source: one.source, component: one.component });
            }
            p = self.node(id).parent;
        }
        let describe = |mark: Option<u32>, role: &str, id: &Option<String>, source: &Option<String>| {
            let mut s = String::new();
            if let Some(m) = mark {
                s.push_str(&format!("#{m} "));
            }
            s.push_str(role);
            if let Some(id) = id {
                s.push_str(&format!(" (id {id})"));
            }
            if let Some(src) = source {
                s.push_str(&format!(" at {src}"));
            }
            s
        };
        let mut text = describe(node.mark, &node.role, &node.id, &node.source);
        if let Some(name) = &node.name {
            let name: String = name.chars().take(40).collect();
            text = text.replacen(&node.role, &format!("{} {name:?}", node.role), 1);
        }
        // The nearest ancestors that say where it is: an id, or another source line.
        let mut last = node.source.clone();
        let mut shown = 0;
        for a in &ancestors {
            if shown == 3 {
                break;
            }
            if a.id.is_some() || (a.source.is_some() && a.source != last) {
                text.push_str(&format!(", in {}", describe(a.mark, &a.role, &a.id, &a.source)));
                last = a.source.clone();
                shown += 1;
            }
        }
        // The components it is part of, innermost first.
        let mut made: Vec<&crate::inspect::ComponentInfo> = Vec::new();
        for c in std::iter::once(&node.component).chain(ancestors.iter().map(|a| &a.component)).flatten() {
            if made.last() != Some(&c) {
                made.push(c);
            }
        }
        for (i, c) in made.iter().take(3).enumerate() {
            text.push_str(if i == 0 { "; made by " } else { " in " });
            text.push_str(&format!("{}() called at {}", c.name, c.called_at));
        }
        Some(Picked { node, ancestors, text })
    }

    /// Why `id` is the size it is: per axis the constraints written on it, what its content
    /// asks, and what decided the size.
    pub fn explain_layout(&self, id: NodeId) -> LayoutExplanation {
        let n = self.node(id);
        let l = *self.taffy.layout(n.taffy).expect("taffy");
        let (x, y, _, _) = self.rect(id);
        let edges = |r: taffy::Rect<f32>| [tenth(r.top), tenth(r.right), tenth(r.bottom), tenth(r.left)];
        let parent = n.parent.filter(|_| id != self.root);
        let pl = parent.map(|p| *self.taffy.layout(self.node(p).taffy).expect("taffy"));
        let content_box = pl.map(|pl| {
            (
                pl.size.width - pl.padding.left - pl.padding.right - pl.border.left - pl.border.right,
                pl.size.height - pl.padding.top - pl.padding.bottom - pl.border.top - pl.border.bottom,
            )
        });
        let content = self.content_extent(id, &l);
        let role = snake(&format!("{:?}", if id == self.root { super::Role::Window } else { self.role(id) }));
        let ctx = |horizontal: bool| Axis {
            horizontal,
            size: if horizontal { l.size.width } else { l.size.height },
            room: content_box.map(|c| if horizontal { c.0 } else { c.1 }),
            content: if horizontal { content.0 } else { content.1 },
        };
        LayoutExplanation {
            id: n.test_id.clone(),
            role,
            source: self.source_of(id),
            component: n.component.map(crate::inspect::ComponentInfo::of),
            bounds: [tenth(x), tenth(y), tenth(l.size.width), tenth(l.size.height)],
            width: self.explain_axis(id, ctx(true)),
            height: self.explain_axis(id, ctx(false)),
            padding: edges(l.padding),
            border: edges(l.border),
            parent: parent.map(|p| {
                let pn = self.node(p);
                let s = &pn.layout;
                let gap = |g: taffy::LengthPercentage| tenth(g.into_raw().value());
                ParentLayout {
                    id: pn.test_id.clone(),
                    role: snake(&format!("{:?}", if p == self.root { super::Role::Window } else { self.role(p) })),
                    source: self.source_of(p),
                    layout: parent_layout(s),
                    content_box: content_box.map(|(w, h)| [tenth(w), tenth(h)]).unwrap_or_default(),
                    gap: [gap(s.gap.width), gap(s.gap.height)],
                    align_items: align_name(s.align_items),
                    justify_content: snake(&format!("{:?}", s.justify_content.keyword)),
                    wrap: s.flex_wrap != taffy::FlexWrap::NoWrap,
                }
            }),
        }
    }

    /// What a node's content asks, `(width, height)` with padding and border: a text's natural
    /// width and its height at its width, else its children's extent.
    fn content_extent(&self, id: NodeId, l: &taffy::Layout) -> (Option<f32>, Option<f32>) {
        let n = self.node(id);
        let frame_w = l.padding.left + l.padding.right + l.border.left + l.border.right;
        let frame_h = l.padding.top + l.padding.bottom + l.border.top + l.border.bottom;
        if n.kind() == Kind::Text
            && let Some(t) = n.text_content()
        {
            let natural =
                super::text_metrics(&mut **self.res.text.borrow_mut(), self.key(id), &t.text, &t.clean, &n.visual.text_style(), None);
            let here = self.text_metrics(id);
            return (Some(natural.width + frame_w), Some(here.height + frame_h));
        }
        if n.children.is_empty() {
            return (None, None);
        }
        let o = l.scrollable_overflow_rect;
        (Some(o.right + l.border.left + l.border.right), Some(o.bottom + l.border.top + l.border.bottom))
    }

    fn explain_axis(&self, id: NodeId, a: Axis) -> AxisExplanation {
        let n = self.node(id);
        let s = &n.layout;
        let (w, minw, maxw) = if a.horizontal { ("w", "min_w", "max_w") } else { ("h", "min_h", "max_h") };
        let axis = |sz: taffy::Size<Len>| if a.horizontal { sz.width } else { sz.height };
        let set = axis(s.size.map(Len::from));
        let min = axis(s.min_size.map(Len::from));
        let max = axis(s.max_size.map(Len::from));
        let mut rules: Vec<String> = [(w, &set), (minw, &min), (maxw, &max)].into_iter().filter_map(|(name, d)| rule(name, d)).collect();
        let resolve = |d: &Len| match d {
            Len::Px(v) => Some(*v),
            Len::Pct(p) => a.room.map(|r| p * r),
            _ => None,
        };
        let near = |v: f32| (a.size - v).abs() < 0.5;
        let num = |v: f32| fmt(tenth(v));
        let done =
            |because: String, rules: Vec<String>| AxisExplanation { size: tenth(a.size), because, rules, content: a.content.map(tenth) };

        if id == self.root {
            return done("the window's size".into(), rules);
        }
        let parent = n.parent.expect("a node below the root");
        let ps = &self.node(parent).layout;
        let grid = ps.display == taffy::Display::Grid;
        let row = matches!(ps.flex_direction, taffy::FlexDirection::Row | taffy::FlexDirection::RowReverse);
        let main = !grid && row == a.horizontal;
        if main {
            if s.flex_grow > 0.0 {
                rules.push(format!("flex_grow({})", fmt(s.flex_grow)));
            }
            if s.flex_shrink > 0.0 {
                rules.push(format!("flex_shrink({})", fmt(s.flex_shrink)));
            }
            if let Some(r) = rule("basis", &Len::from(s.flex_basis)) {
                rules.push(r);
            }
        }
        if let Some(r) = s.aspect_ratio {
            rules.push(format!("aspect_ratio({})", fmt(r)));
        }

        // Limits first: a size held at one is decided by it.
        if let Some(v) = resolve(&max)
            && near(v)
        {
            return done(format!("held at its maximum, {} ({maxw})", num(v)), rules);
        }
        if let Some(v) = resolve(&min)
            && near(v)
            && a.content.is_none_or(|c| c < v - 0.5)
        {
            return done(format!("held at its minimum, {} ({minw})", num(v)), rules);
        }
        if let Some(v) = resolve(&set)
            && near(v)
        {
            let how = match set {
                Len::Pct(p) => format!("{} of the parent's {}", fmt(p), num(a.room.unwrap_or(0.0))),
                _ => "set".into(),
            };
            return done(format!("{} ({how}, {w})", num(v)), rules);
        }
        if s.aspect_ratio.is_some() && resolve(&Len::from(if a.horizontal { s.size.height } else { s.size.width })).is_some() {
            return done("follows its aspect ratio from the other side".into(), rules);
        }
        if s.position == taffy::Position::Absolute {
            return done("positioned on its own (an overlay or absolute box): its content".into(), rules);
        }
        let room = a.room.map(num).unwrap_or_default();
        let content = a.content;
        if grid {
            let stretched = stretches(s.align_self.unwrap_or(ps.align_items));
            return done(if stretched { "fills its grid cell".into() } else { "its content, inside its grid cell".into() }, rules);
        }
        if main {
            let base = resolve(&Len::from(s.flex_basis)).or(content).unwrap_or(0.0);
            if s.flex_grow > 0.0 && a.size > base + 0.5 {
                return done(format!("grows into the free space of the parent's {room} (flex_grow)"), rules);
            }
            if a.size < base - 0.5 {
                return done(
                    format!("shrinks from {} to fit the parent's {room} (flex_shrink); content below it is cut or wraps", num(base)),
                    rules,
                );
            }
            return match content {
                Some(c) if c > a.size + 0.5 => {
                    done(format!("its content's {} does not fit: it overflows (boxes do not shrink without flex_shrink)", num(c)), rules)
                }
                Some(_) => done(if n.kind() == Kind::Text { "its text".into() } else { "its content".into() }, rules),
                None => done("nothing gives it a size on this axis (no content, no size set)".into(), rules),
            };
        }
        // The cross axis.
        let align = s.align_self.unwrap_or(ps.align_items);
        if stretches(align) {
            return done(format!("stretched to the parent's {room} (align_items stretch)"), rules);
        }
        let why = match content {
            Some(_) if n.kind() == Kind::Text && a.horizontal => "its text".to_owned(),
            Some(_) if n.kind() == Kind::Text => "its text's lines at this width".to_owned(),
            Some(_) => "its content".to_owned(),
            None => "nothing gives it a size on this axis (no content, no size set)".to_owned(),
        };
        done(format!("{why} (align_items {}, so not stretched)", align_name(align)), rules)
    }
}

/// One axis being explained.
struct Axis {
    horizontal: bool,
    size: f32,
    /// The parent's content box on this axis.
    room: Option<f32>,
    content: Option<f32>,
}

/// A size as written: logical pixels, a fraction of the parent, automatic, or a keyword.
enum Len {
    Px(f32),
    Pct(f32),
    Auto,
    Other(String),
}

impl From<taffy::Dimension> for Len {
    fn from(d: taffy::Dimension) -> Len {
        use taffy::style::ExpandedDimension as D;
        match d.expand() {
            D::Length(v) => Len::Px(v),
            D::Percent(p) => Len::Pct(p),
            D::Auto => Len::Auto,
            other => Len::Other(snake(&format!("{other:?}"))),
        }
    }
}

impl From<taffy::LengthPercentageAuto> for Len {
    fn from(d: taffy::LengthPercentageAuto) -> Len {
        use taffy::style::ExpandedLengthPercentageAuto as D;
        match d.expand() {
            D::Length(v) => Len::Px(v),
            D::Percent(p) => Len::Pct(p),
            D::Auto => Len::Auto,
            #[allow(unreachable_patterns)]
            _ => Len::Other("calc".into()),
        }
    }
}

/// A size constraint as the style method writes it (`w(320)`, `w(relative(0.5))`).
fn rule(name: &str, d: &Len) -> Option<String> {
    match d {
        Len::Auto => None,
        Len::Px(v) => Some(format!("{name}({})", fmt(*v))),
        Len::Pct(p) => Some(format!("{name}(relative({}))", fmt(*p))),
        Len::Other(k) => Some(format!("{name}({k})")),
    }
}

/// Whether a flex or grid item with this alignment fills its line's cross size.
fn stretches(a: taffy::AlignItems) -> bool {
    matches!(a.keyword, taffy::style::AlignItemsKeyword::Normal | taffy::style::AlignItemsKeyword::Stretch)
}

fn align_name(a: taffy::AlignItems) -> String {
    if a.keyword == taffy::style::AlignItemsKeyword::Normal { "stretch".into() } else { snake(&format!("{:?}", a.keyword)) }
}

fn fmt(v: f32) -> String {
    let t = (v * 10.0).round() / 10.0;
    if t.fract() == 0.0 { format!("{}", t as i64) } else { format!("{t}") }
}

fn parent_layout(s: &taffy::Style) -> String {
    if s.display == taffy::Display::Grid {
        return "grid".into();
    }
    snake(&format!("{:?}", s.flex_direction))
}
