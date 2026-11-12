//! Layout diagnostics: problems visible only after layout, as data.
//!
//! Like Flutter's "RenderFlex overflowed" but queryable, so tests and agents can assert there
//! are none. Each issue names the node (test id and path), where it was created, and the usual
//! fix in Tailwind terms.

use super::theme::{contrast, over};
use super::{Kernel, Kind, NodeId, Wrap};
use crate::diagnostics::{self as codes, Code};
use std::fmt::Write as _;
use std::panic::Location;

/// One layout problem.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutIssue {
    /// `OVERFLOW`, `COLLAPSED`, `TEXT_TRUNCATED`, `OVERLAY_CLAMPED` or `MISSING_TOKEN`; the
    /// code carries the severity and the cause.
    pub code: &'static Code,
    pub test_id: Option<String>,
    /// Ancestors by test id or role, e.g. `col > #todos > #todo-3 > Label`.
    pub path: String,
    /// Where the element was created (`file:line:column`), when known.
    pub at: Option<String>,
    pub message: String,
}

/// Overflow smaller than this is rounding.
const TOLERANCE: f32 = 0.5;

impl Kernel {
    /// Records where an element was created, for diagnostics.
    pub fn set_origin(&mut self, id: NodeId, at: &'static Location<'static>) {
        self.node_mut(id).origin = Some(at);
    }

    fn name(&self, id: NodeId) -> String {
        match &self.node(id).test_id {
            Some(t) => format!("#{t}"),
            None => format!("{:?}", self.role(id)),
        }
    }

    fn path(&self, id: NodeId) -> String {
        let mut chain = vec![self.name(id)];
        let mut p = self.node(id).parent;
        while let Some(n) = p {
            if n == self.root {
                break;
            }
            if n != self.overlay_layer {
                chain.push(self.name(n));
            }
            p = self.node(n).parent;
        }
        chain.reverse();
        chain.join(" > ")
    }

    /// The content box (inside padding and border) in absolute coordinates.
    fn content_box(&self, id: NodeId) -> (f32, f32, f32, f32) {
        let (x, y, w, h) = self.rect(id);
        let l = self.taffy.layout(self.node(id).taffy).expect("taffy");
        let (pl, pt) = (l.padding.left + l.border.left, l.padding.top + l.border.top);
        let (pr, pb) = (l.padding.right + l.border.right, l.padding.bottom + l.border.bottom);
        (x + pl, y + pt, (w - pl - pr).max(0.0), (h - pt - pb).max(0.0))
    }

    /// Content painted over an overlay native view (after it in paint order, overlapping it):
    /// the view, above Octara's surface, would hide it.
    fn native_issues(&self) -> Vec<LayoutIssue> {
        use crate::paint::{Item, Rect};
        if self.native_nodes.is_empty() {
            return Vec::new();
        }
        let dl = self.display_list();
        let mut overlays: Vec<(NodeId, Rect)> = Vec::new();
        let mut covered: Vec<(NodeId, NodeId)> = Vec::new();
        for item in &dl.items() {
            let drawn = match item {
                Item::Native { source, rect, slot: crate::native::Slot::Overlay, .. } => {
                    overlays.push((*source, *rect));
                    continue;
                }
                Item::Quad(q) => Some((q.source, q.rect)),
                Item::Shadow(s) => Some((s.source, s.rect)),
                Item::Image(i) => Some((i.source, i.rect.intersect(&i.bounds))),
                Item::Path(p) => Some((p.source, p.bounds)),
                Item::Glyphs(g) => g
                    .placed()
                    .map(|gl| Rect { x: gl.x, y: gl.y - g.size, w: g.size * 0.5, h: g.size })
                    .reduce(|a, b| {
                        let (x, y) = (a.x.min(b.x), a.y.min(b.y));
                        Rect { x, y, w: a.right().max(b.right()) - x, h: a.bottom().max(b.bottom()) - y }
                    })
                    .map(|r| (g.source, r)),
                _ => None,
            };
            let Some((src, r)) = drawn else { continue };
            for (view, vr) in &overlays {
                if src != *view && !r.intersect(vr).is_empty() && !covered.contains(&(src, *view)) {
                    covered.push((src, *view));
                }
            }
        }
        covered
            .into_iter()
            .map(|(src, view)| {
                self.issue(
                    src,
                    &codes::NATIVE_COVERED,
                    format!(
                        "{} is painted over {}, a native view above Octara's surface: the view hides it",
                        self.name(src),
                        self.name(view)
                    ),
                )
            })
            .collect()
    }

    fn issue(&self, id: NodeId, code: &'static Code, message: String) -> LayoutIssue {
        let n = self.node(id);
        LayoutIssue {
            code,
            test_id: n.test_id.clone(),
            path: self.path(id),
            at: n.origin.map(|l| format!("{}:{}:{}", crate::diagnostics::source_file(l), l.line(), l.column())),
            message,
        }
    }

    /// Problems in the last layout, in tree order.
    pub fn layout_issues(&self) -> Vec<LayoutIssue> {
        let mut out = self.native_issues();
        let mut stack = vec![self.root];
        while let Some(id) = stack.pop() {
            let n = self.node(id);
            if n.layout.display == taffy::Display::None {
                continue;
            }
            self.check(id, &mut out);
            stack.extend(n.children.iter().rev().copied());
        }
        out
    }

    fn check(&self, id: NodeId, out: &mut Vec<LayoutIssue>) {
        let n = self.node(id);
        let (.., w, h) = self.rect(id);

        if let Some(names) = self.missing_tokens.get(&id) {
            let mut list = String::new();
            for (i, name) in names.iter().enumerate() {
                let _ = write!(list, "{}`{name}`", if i > 0 { ", " } else { "" });
            }
            out.push(self.issue(
                id,
                &codes::MISSING_TOKEN,
                format!("{} uses {list}, which the theme does not define (drawn magenta)", self.name(id)),
            ));
        }

        if n.anchor.is_some() && n.placed.is_some_and(|p| p.clamped) {
            out.push(self.issue(
                id,
                &codes::OVERLAY_CLAMPED,
                format!("overlay {} ({w} × {h}) fits neither side of its anchor and was pushed into the window", self.name(id)),
            ));
        }

        if n.kind() == Kind::Text && !n.text().is_empty() {
            let m = self.text_metrics(id);
            if m.truncated {
                let how = if n.visual.wrap == Wrap::Truncate { "is truncated" } else { "is cut by `line_clamp`" };
                out.push(self.issue(id, &codes::TEXT_TRUNCATED, format!("text {} {how}: {} of its lines shown", self.name(id), m.lines)));
            }
        }

        // Collapsed: no area although it has content.
        if w <= TOLERANCE || h <= TOLERANCE {
            let explicit_zero = |d: taffy::Dimension| d == taffy::Dimension::length(0.0);
            let sized_zero = explicit_zero(n.layout.size.width) || explicit_zero(n.layout.size.height);
            let content = match n.kind() {
                Kind::Text => !n.text().is_empty(),
                _ => n.children.iter().any(|c| {
                    let (.., cw, ch) = self.rect(*c);
                    self.node(*c).layout.display != taffy::Display::None && cw > TOLERANCE && ch > TOLERANCE
                }),
            };
            if content && !sized_zero && n.anchor.is_none() && id != self.overlay_layer {
                out.push(self.issue(id, &codes::COLLAPSED, format!("{} is {w} × {h} px although it has content", self.name(id))));
            }
        }

        // Overflow: children past this node's content box (the window for the root).
        if id == self.overlay_layer || n.kind() == Kind::Text {
            return;
        }
        let (cx, cy, cw, ch) = if id == self.root { (0.0, 0.0, self.viewport.0, self.viewport.1) } else { self.content_box(id) };
        // One issue per container and edge, like Flutter's "RenderFlex overflowed by N px".
        let (mut right, mut bottom) = ((0.0f32, Vec::new()), (0.0f32, Vec::new()));
        let mut nowrap_text = false;
        for &c in &n.children {
            let cn = self.node(c);
            if c == self.overlay_layer || cn.anchor.is_some() || cn.hidden || cn.layout.display == taffy::Display::None {
                continue;
            }
            let (kx, ky, kw, kh) = self.rect(c);
            let r = (kx + kw) - (cx + cw);
            let b = (ky + kh) - (cy + ch);
            if r > TOLERANCE {
                right.0 = right.0.max(r);
                right.1.push(self.name(c));
                nowrap_text |= cn.kind() == Kind::Text && cn.visual.wrap == Wrap::None;
            }
            if b > TOLERANCE {
                bottom.0 = bottom.0.max(b);
                bottom.1.push(self.name(c));
            }
        }
        let what = if id == self.root { "the window".to_owned() } else { self.name(id) };
        let list = |names: &[String]| {
            let shown: Vec<&str> = names.iter().take(3).map(String::as_str).collect();
            let more = if names.len() > 3 { format!(" and {} more", names.len() - 3) } else { String::new() };
            format!("{}{more}", shown.join(", "))
        };
        // Scrolled or clipped axes are not overflowing.
        let o = n.layout.overflow;
        if o.x != taffy::Overflow::Visible {
            right.1.clear();
        }
        if o.y != taffy::Overflow::Visible {
            bottom.1.clear();
        }
        if !right.1.is_empty() {
            let nowrap = if nowrap_text { "; text set not to wrap (`whitespace_nowrap`)" } else { "" };
            out.push(self.issue(
                id,
                &codes::OVERFLOW,
                format!("content of {what} overflows its right edge by {} px ({}{nowrap})", right.0, list(&right.1)),
            ));
        }
        if !bottom.1.is_empty() {
            out.push(self.issue(
                id,
                &codes::OVERFLOW,
                format!("content of {what} overflows its bottom by {} px ({})", bottom.0, list(&bottom.1)),
            ));
        }
    }
}

/// A touch target's minimum side (Apple's 44 pt; Exact asks the same on touch screens).
pub const MIN_TOUCH_TARGET: f32 = 44.0;
/// `(x, y, width, height)` in logical pixels.
type Rect = (f32, f32, f32, f32);

/// A pointer target's minimum side (WCAG 2.5.8), unless it is spaced that far from the others.
pub const MIN_POINTER_TARGET: f32 = 24.0;

impl Kernel {
    /// Accessibility problems in the last layout (as Exact judges them): controls without a name,
    /// canvases without a label, text without enough contrast, and targets under the platform's
    /// minimum — 44 × 44 on touch, 24 × 24 with a pointer unless no other control's center is
    /// within 24 (WCAG 2.5.8).
    pub fn accessibility_issues(&self) -> Vec<LayoutIssue> {
        let mut out = Vec::new();
        let mut controls = Vec::new();
        let page = self.theme().get(crate::kernel::Token::Background);
        self.a11y_check(self.root, Some(page), &mut controls, &mut out);
        let (min, spaced_ok) = if self.res.touch.get() { (MIN_TOUCH_TARGET, false) } else { (MIN_POINTER_TARGET, true) };
        let center = |(x, y, w, h): (f32, f32, f32, f32)| (x + w / 2.0, y + h / 2.0);
        for &(id, rect) in &controls {
            let (.., w, h) = rect;
            if w >= min - TOLERANCE && h >= min - TOLERANCE {
                continue;
            }
            let (cx, cy) = center(rect);
            let spaced = controls.iter().all(|&(o, r)| {
                let (ox, oy) = center(r);
                o == id || (ox - cx).hypot(oy - cy) >= min
            });
            if spaced_ok && spaced {
                continue;
            }
            let how = if self.res.touch.get() { "touch targets" } else { "pointer targets this close to others" };
            out.push(self.issue(id, &codes::SMALL_TARGET, format!("{} is {w} × {h} px; {how} need {min} × {min}", self.name(id))));
        }
        out
    }

    /// `under`: the solid color behind this node, `None` where it is a gradient or an image
    /// (contrast is not judged there).
    fn a11y_check(&self, id: NodeId, under: Option<crate::kernel::Color>, controls: &mut Vec<(NodeId, Rect)>, out: &mut Vec<LayoutIssue>) {
        let n = self.node(id);
        if n.hidden || n.layout.display == taffy::Display::None {
            return;
        }
        let bg = if n.visual.gradient.is_some() || matches!(n.content, super::Content::Image(..)) {
            None
        } else {
            match n.visual.background {
                Some(b) if b.components[3] >= 1.0 => Some(b),
                Some(b) if b.components[3] > 0.0 => under.map(|u| over(b, u)),
                _ => under,
            }
        };
        if matches!(n.content, super::Content::Native(_)) && n.label.is_none() {
            let (.., w, h) = self.shown_rect(id);
            if w > 0.0 && h > 0.0 {
                out.push(self.issue(id, &codes::UNLABELED_NATIVE_VIEW, format!("native view {} has no label", self.name(id))));
            }
        }
        if matches!(n.content, super::Content::Canvas(_)) && n.label.is_none() {
            let (.., w, h) = self.shown_rect(id);
            if w > 0.0 && h > 0.0 {
                out.push(self.issue(id, &codes::UNLABELED_CANVAS, format!("canvas {} has no label", self.name(id))));
            }
        }
        let disabled = n.state.contains(super::State::DISABLED);
        let control = n.kind() == Kind::TextInput || super::HandlerKind::POINTER.iter().any(|k| n.handlers[*k as usize].is_some());
        let (.., w, h) = self.shown_rect(id);
        if control && w > 0.0 && h > 0.0 {
            if n.label.is_none() && !self.has_text(id) && n.kind() != Kind::TextInput {
                out.push(self.issue(
                    id,
                    &codes::UNNAMED_CONTROL,
                    format!("{} can be operated but has no name for assistive technology", self.name(id)),
                ));
            }
            if n.kind() == Kind::TextInput && n.label.is_none() {
                out.push(self.issue(
                    id,
                    &codes::UNNAMED_CONTROL,
                    format!("text field {} has no label (a placeholder is not a name)", self.name(id)),
                ));
            }
            if !disabled {
                controls.push((id, self.shown_rect(id)));
            }
        }
        if let Some(bg) = bg
            && n.kind() == Kind::Text
            && !n.text().trim().is_empty()
            && !disabled
            && n.visual.opacity > 0.0
        {
            let fg = over(n.visual.text, bg);
            let ratio = contrast(fg, bg);
            let large = n.visual.font_size >= 24.0 || (n.visual.font_size >= 18.66 && n.visual.font_weight >= 700);
            let need = if large { 3.0 } else { 4.5 };
            if ratio < need {
                out.push(self.issue(
                    id,
                    &codes::LOW_CONTRAST,
                    format!("text {} has contrast {ratio:.2}:1 against its background; needs {need}:1", self.name(id)),
                ));
            }
        }
        for &c in &n.children {
            self.a11y_check(c, bg, controls, out);
        }
    }

    /// Whether any shown text lies inside `id`.
    fn has_text(&self, id: NodeId) -> bool {
        let n = self.node(id);
        (n.kind() == Kind::Text && !n.text().trim().is_empty()) || n.children.iter().any(|c| !self.node(*c).hidden && self.has_text(*c))
    }
}
