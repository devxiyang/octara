//! Building the display list from the laid-out tree.

use super::text::{self, Wrap};
use super::{Align, Color, Kernel, Kind, NodeId, Token};
use crate::paint::{DisplayList, GlyphRun, Item, PathPaint, Quad, Rect};
use crate::path::Transform;

fn fade(c: Color, opacity: f32) -> Color {
    if opacity >= 1.0 { c } else { c.multiply_alpha(opacity) }
}

impl Kernel {
    /// What the last layout looks like, in drawing order (children over parents, overlays last).
    pub fn display_list(&self) -> DisplayList {
        let mut items = Vec::new();
        let window = Rect { x: 0.0, y: 0.0, w: self.viewport.0, h: self.viewport.1 };
        self.paint(self.root, 1.0, window, &mut items);
        DisplayList { size: self.viewport, background: self.theme.get(Token::Background), items }
    }

    /// Where the node is shown now (moving layout included).
    fn rect_of(&self, id: NodeId) -> Rect {
        let (x, y, w, h) = self.shown_rect(id);
        Rect { x, y, w, h }
    }

    /// Paints `id` and its subtree. Below full opacity, a subtree that draws more than one thing
    /// goes into a layer composited at that opacity (Rune's group opacity); a single thing is
    /// faded directly.
    fn paint(&self, id: NodeId, opacity: f32, clip: Rect, out: &mut Vec<Item>) {
        let n = self.node(id);
        if n.layout.display == taffy::Display::None {
            return;
        }
        let own = self.presented(id).opacity;
        if own >= 1.0 {
            return self.paint_node(id, opacity, clip, out);
        }
        let k = opacity * own;
        if k <= 0.0 {
            return;
        }
        let mut inner = Vec::new();
        self.paint_node(id, 1.0, clip, &mut inner);
        if inner.iter().filter(|i| i.draws()).count() <= 1 {
            for mut i in inner {
                i.fade(k);
                out.push(i);
            }
        } else {
            out.push(Item::PushLayer { source: id, opacity: k });
            out.extend(inner);
            out.push(Item::PopLayer);
        }
    }

    /// Paints `id` and its subtree, fading colors by `opacity` (the node's own opacity is the
    /// caller's to apply).
    fn paint_node(&self, id: NodeId, opacity: f32, clip: Rect, out: &mut Vec<Item>) {
        let n = self.node(id);
        let rect = self.rect_of(id);
        let l = self.taffy.layout(n.taffy).expect("taffy");
        let visible = !rect.intersect(&clip).is_empty();
        // What is shown now: moving properties sampled at the kernel's time.
        let shown = self.presented(id);
        let v = &shown;

        // Shadows under the box, the first listed on top.
        let radius = v.radius.min(rect.w / 2.0).min(rect.h / 2.0).max(0.0);
        for s in self.presented_shadows(id).iter().rev() {
            let grow = s.spread_radius;
            let r = Rect { x: rect.x + s.offset.0 - grow, y: rect.y + s.offset.1 - grow, w: rect.w + 2.0 * grow, h: rect.h + 2.0 * grow };
            if r.w <= 0.0 || r.h <= 0.0 {
                continue;
            }
            let shadow = crate::paint::Shadow {
                source: id,
                rect: r,
                radius: (radius + grow).max(0.0),
                color: fade(s.color, opacity),
                blur: s.blur_radius,
            };
            let reach = shadow.reach();
            let bounds = Rect { x: r.x - reach, y: r.y - reach, w: r.w + 2.0 * reach, h: r.h + 2.0 * reach };
            if !bounds.intersect(&clip).is_empty() {
                out.push(Item::Shadow(shadow));
            }
        }

        // Background and border.
        let border = [l.border.top, l.border.right, l.border.bottom, l.border.left];
        let has_border = border.iter().any(|b| *b > 0.0);
        if visible && (v.background.is_some() || v.gradient.is_some() || has_border) {
            let gradient = v.gradient.map(|g| crate::paint::Gradient {
                from: (fade(g.from.0, opacity), g.from.1),
                to: (fade(g.to.0, opacity), g.to.1),
                ..g
            });
            out.push(Item::Quad(Quad {
                source: id,
                rect,
                radius: v.radius.min(rect.w / 2.0).min(rect.h / 2.0).max(0.0),
                fill: v.background.map(|c| fade(c, opacity)),
                gradient,
                border,
                border_color: fade(v.border, opacity),
            }));
        }

        // An image, fitted into the padding box and clipped to the box's corners.
        if let super::Content::Image(img, fit) = &n.content
            && visible
        {
            let (iw, ih) = img.size();
            let inner = Rect {
                x: rect.x + l.border.left,
                y: rect.y + l.border.top,
                w: (rect.w - l.border.left - l.border.right).max(0.0),
                h: (rect.h - l.border.top - l.border.bottom).max(0.0),
            };
            let (x, y, w, h) = fit.place((iw as f32, ih as f32), (inner.x, inner.y, inner.w, inner.h));
            if w > 0.0 && h > 0.0 {
                out.push(Item::Image(crate::paint::ImagePaint {
                    source: id,
                    image: img.clone(),
                    rect: Rect { x, y, w, h },
                    bounds: inner,
                    radius: (v.radius - l.border.left).max(0.0).min(inner.w / 2.0).min(inner.h / 2.0),
                    opacity,
                }));
            }
        }

        // An icon, fitted into the content box (centered, aspect kept), in the text color.
        if let super::Content::Icon(icon) = &n.content
            && visible
        {
            let content = Rect {
                x: rect.x + l.border.left + l.padding.left,
                y: rect.y + l.border.top + l.padding.top,
                w: (rect.w - l.border.left - l.border.right - l.padding.left - l.padding.right).max(0.0),
                h: (rect.h - l.border.top - l.border.bottom - l.padding.top - l.padding.bottom).max(0.0),
            };
            let vb = icon.view_box;
            let k = (content.w / vb.w).min(content.h / vb.h);
            if k > 0.0 && k.is_finite() {
                let (ox, oy) = (content.x + (content.w - vb.w * k) * 0.5, content.y + (content.h - vb.h * k) * 0.5);
                let transform = Transform::translate(-vb.x, -vb.y).then(&Transform::scale(k, k)).then(&Transform::translate(ox, oy));
                let color = fade(v.text, opacity);
                for s in &icon.shapes {
                    out.push(Item::Path(PathPaint {
                        source: id,
                        path: s.path.clone(),
                        transform,
                        style: s.style.clone(),
                        color,
                        gradient: None,
                        bounds: content,
                    }));
                }
            }
        }

        // A canvas's recording, in its content box, clipped to it.
        if let super::Content::Canvas(ops) = &n.content
            && visible
        {
            let content = Rect {
                x: rect.x + l.border.left + l.padding.left,
                y: rect.y + l.border.top + l.padding.top,
                w: (rect.w - l.border.left - l.border.right - l.padding.left - l.padding.right).max(0.0),
                h: (rect.h - l.border.top - l.border.bottom - l.padding.top - l.padding.bottom).max(0.0),
            };
            self.paint_canvas(id, ops, content, (v.text_style(), v.text), opacity, out);
        }

        // A text input: its line, selection, composition and caret.
        if n.kind() == Kind::TextInput && visible {
            self.paint_input(id, opacity, out);
        }
        // A label's text.
        if n.kind() == Kind::Text && visible {
            let (s, style) = (n.text(), v.text_style());
            let cx = rect.x + l.border.left + l.padding.left;
            let cy = rect.y + l.border.top + l.padding.top;
            let inner = (l.size.width - l.border.left - l.border.right - l.padding.left - l.padding.right).max(0.0);
            let limit = match style.wrap {
                Wrap::Words => Some(inner),
                Wrap::None | Wrap::Truncate => None,
            };
            if !s.is_empty() {
                let mut engine = self.text.borrow_mut();
                let clean = n.text_content().map_or(0, |t| t.clean.replace(s.len()));
                text::measure(&mut **engine, id.into(), s, clean, &style, limit);
                // Cut text (one line, or `line_clamp`) ends in an ellipsis inside the box.
                let fit = (style.wrap == Wrap::Truncate || style.max_lines.is_some()).then_some(inner);
                let runs = engine.glyph_runs(id.into(), &style, limit, fit);
                drop(engine);
                // A truncated line is clipped to the box (an ellipsis comes with the renderer).
                let clip_text = style.wrap == Wrap::Truncate;
                if clip_text {
                    out.push(Item::PushClip { source: id, rect: Rect { x: cx, y: cy, w: inner, h: rect.h }, radius: 0.0 });
                }
                for r in runs {
                    let glyphs: Vec<_> = r.glyphs.into_iter().map(|g| crate::paint::Glyph { id: g.id, x: g.x + cx, y: g.y + cy }).collect();
                    out.push(Item::Glyphs(GlyphRun {
                        source: id,
                        font: r.font,
                        size: r.size,
                        coords: r.coords,
                        embolden: r.embolden,
                        color: fade(v.text, opacity),
                        glyphs,
                    }));
                }
                if clip_text {
                    out.push(Item::PopClip);
                }
            }
        }

        // Children, clipped to the padding box when the node scrolls or clips.
        if n.children.is_empty() && self.ghosts_of(id).is_empty() {
            return;
        }
        let clips = n.layout.overflow.x != taffy::Overflow::Visible || n.layout.overflow.y != taffy::Overflow::Visible;
        let mut child_clip = clip;
        if clips {
            let inner = Rect {
                x: rect.x + l.border.left,
                y: rect.y + l.border.top,
                w: (rect.w - l.border.left - l.border.right).max(0.0),
                h: (rect.h - l.border.top - l.border.bottom).max(0.0),
            };
            child_clip = clip.intersect(&inner);
            if child_clip.is_empty() {
                return;
            }
            out.push(Item::PushClip { source: id, rect: inner, radius: (v.radius - l.border.left).max(0.0) });
        }
        for &c in &n.children {
            self.paint(c, opacity, child_clip, out);
        }
        // Subtrees on their way out, where they were.
        for g in self.ghosts_of(id) {
            self.paint(g, opacity, child_clip, out);
        }
        if clips {
            out.push(Item::PopClip);
        }
    }
}

impl Kernel {
    /// A canvas's ops as display-list items: offset to `content`, paints resolved in the theme.
    fn paint_canvas(
        &self,
        id: NodeId,
        ops: &[crate::canvas::CanvasOp],
        content: Rect,
        (base, text_color): (text::TextStyle, Color),
        opacity: f32,
        out: &mut Vec<Item>,
    ) {
        use crate::canvas::CanvasOp;
        use crate::kernel::Fill;
        let origin = Transform::translate(content.x, content.y);
        let at = |r: Rect| Rect { x: r.x + content.x, y: r.y + content.y, ..r };
        let color = |p: crate::kernel::Paint| fade(p.resolve(&self.theme).0, opacity);
        out.push(Item::PushClip { source: id, rect: content, radius: 0.0 });
        // A canvas's texts are cached by the text engine as slots of its node, in order.
        let mut slot = 0u32;
        for op in ops {
            match op {
                CanvasOp::Path { path, style, fill, transform } => {
                    let t = transform.then(&origin);
                    // A gradient spans the path's box.
                    let bounds = path.bounds().map_or(content, |b| {
                        let pts = [(b.x, b.y), (b.right(), b.y), (b.x, b.bottom()), (b.right(), b.bottom())]
                            .map(|(x, y)| t.apply(crate::path::point(x, y)));
                        let x0 = pts.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
                        let y0 = pts.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
                        let x1 = pts.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max);
                        let y1 = pts.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
                        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
                    });
                    let (c, gradient) = match *fill {
                        Fill::Solid(p) => (color(p), None),
                        Fill::Linear(g) => (
                            crate::kernel::Color::TRANSPARENT,
                            Some(crate::paint::Gradient {
                                angle: g.angle,
                                from: (color(g.from.color), g.from.percentage),
                                to: (color(g.to.color), g.to.percentage),
                            }),
                        ),
                    };
                    out.push(Item::Path(PathPaint {
                        source: id,
                        path: path.clone(),
                        transform: t,
                        style: style.clone(),
                        color: c,
                        gradient,
                        bounds,
                    }));
                }
                CanvasOp::Image { image, rect } => {
                    let r = at(*rect);
                    out.push(Item::Image(crate::paint::ImagePaint { source: id, image: image.clone(), rect: r, bounds: r, radius: 0.0, opacity }));
                }
                CanvasOp::Text(t) => {
                    slot += 1;
                    let size = t.size.unwrap_or(base.font_size) * t.scale;
                    let style = text::TextStyle {
                        font_size: size,
                        line_height: base.line_height * size / base.font_size.max(1.0),
                        weight: t.weight.unwrap_or(base.weight),
                        wrap: if t.max_width.is_some() { Wrap::Words } else { Wrap::None },
                        max_lines: None,
                        ..base
                    };
                    let limit = t.max_width.map(|w| w * t.scale);
                    let key = text::TextKey { node: id, slot };
                    let mut engine = self.text.borrow_mut();
                    let m = text::measure(&mut **engine, key, &t.text, 0, &style, limit);
                    let runs = engine.glyph_runs(key, &style, limit, None);
                    drop(engine);
                    let along = |a: Align, len: f32| match a {
                        Align::Start => 0.0,
                        Align::Center => len / 2.0,
                        Align::End => len,
                    };
                    let x = content.x + t.at.x - along(t.align, m.width);
                    let y = content.y + t.at.y - along(t.valign, m.height);
                    let color = t.color.map_or(fade(text_color, opacity), color);
                    for r in runs {
                        let glyphs = r.glyphs.into_iter().map(|g| crate::paint::Glyph { id: g.id, x: g.x + x, y: g.y + y }).collect();
                        out.push(Item::Glyphs(GlyphRun {
                            source: id,
                            font: r.font,
                            size: r.size,
                            coords: r.coords,
                            embolden: r.embolden,
                            color,
                            glyphs,
                        }));
                    }
                }
                CanvasOp::PushClip { rect, radius } => out.push(Item::PushClip { source: id, rect: at(*rect), radius: *radius }),
                CanvasOp::PopClip => out.push(Item::PopClip),
            }
        }
        out.push(Item::PopClip);
    }
}

impl Kernel {
    /// The topmost node under `(x, y)` (logical pixels): drawn last, inside every clip around it.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<NodeId> {
        let window = Rect { x: 0.0, y: 0.0, w: self.viewport.0, h: self.viewport.1 };
        let mut hit = None;
        self.hit(self.root, x, y, window, &mut hit);
        hit
    }

    fn hit(&self, id: NodeId, x: f32, y: f32, clip: Rect, hit: &mut Option<NodeId>) {
        let n = self.node(id);
        if n.layout.display == taffy::Display::None {
            return;
        }
        let inside = |r: &Rect| x >= r.x && x < r.right() && y >= r.y && y < r.bottom();
        let rect = self.rect_of(id);
        if inside(&rect) && inside(&clip) && id != self.overlay_layer {
            *hit = Some(id);
        }
        let mut child_clip = clip;
        if n.layout.overflow.x != taffy::Overflow::Visible || n.layout.overflow.y != taffy::Overflow::Visible {
            child_clip = clip.intersect(&rect);
        }
        for &c in &n.children {
            self.hit(c, x, y, child_clip, hit);
        }
    }
}
