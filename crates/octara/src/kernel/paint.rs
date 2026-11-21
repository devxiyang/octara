//! Building the display list from the laid-out tree: each node's own drawing is built in its
//! own coordinates and kept until what it is drawn from changes; a frame walks the shown nodes
//! and places their drawings, with the clips and layers between them.

use super::text::{self, Wrap};
use super::{Align, BoxShadow, Color, Dirty, Kernel, Kind, NodeId, Token};
use crate::image::{Image, Sample};
use crate::paint::{CustomPaint, DisplayList, GlyphRun, Item, PaintMount, PaintProgram, Part, PathPaint, Quad, Rect};
use crate::path::Transform;
use std::sync::{Arc, Weak};

/// What a node's own drawing was built from: while it stays the same, the drawing is kept.
#[derive(PartialEq)]
pub(crate) struct DrawKey {
    ver: u64,
    exiting_paint: bool,
    size: (u32, u32),
    /// Text wrapping and content placement use target layout geometry, which can change
    /// before the animated size shown above starts moving.
    layout_size: taffy::Size<f32>,
    padding: taffy::Rect<f32>,
    border: taffy::Rect<f32>,
    /// What it shows now (opacity aside: that is the frame's to apply).
    visual: super::Visual,
    shadows: Vec<BoxShadow>,
    theme: u64,
    fonts: u64,
}

/// A node's kept drawing.
pub(crate) type Drawn = (DrawKey, Arc<[Item]>);

pub(crate) struct CustomPaintBinding {
    program: PaintProgram,
    params: [f32; 4],
    mount: PaintMount,
}

#[derive(Clone, Copy)]
enum PaintMode {
    Live { keep: bool, motion_at: f64 },
    Capture,
}

impl PaintMode {
    fn keeps_drawings(self) -> bool {
        matches!(self, Self::Live { keep: true, .. })
    }

    fn motion_at(self, now: f64) -> f64 {
        match self {
            Self::Live { motion_at, .. } => motion_at,
            Self::Capture => now,
        }
    }
}

fn fade(c: Color, opacity: f32) -> Color {
    if opacity >= 1.0 { c } else { c.multiply_alpha(opacity) }
}

impl Kernel {
    /// What the last layout looks like, in drawing order (children over parents, overlays last).
    /// While nothing moves and nothing changed since, the last list is returned again.
    pub fn display_list(&self) -> Arc<DisplayList> {
        self.display_list_at(self.now)
    }

    /// Sample analytic paint tracks while keeping the settled geometry and text state.
    pub(crate) fn display_list_at(&self, motion_at: f64) -> Arc<DisplayList> {
        let still = self.at_rest();
        if still
            && let Some((generation, viewport, dl)) = &*self.last_frame.borrow()
            && *generation == self.paint_gen
            && *viewport == self.viewport
        {
            // Debug builds check that reusing it is right: every change must bump the count.
            #[cfg(debug_assertions)]
            debug_assert_eq!(
                format!("{:?}", self.build_display_list(PaintMode::Live { keep: false, motion_at }).items()),
                format!("{:?}", dl.items()),
                "a change did not invalidate the last frame"
            );
            return dl.clone();
        }
        let dl = Arc::new(self.build_display_list(PaintMode::Live { keep: true, motion_at }));
        // Debug builds check the kept drawings: the frame built without them is the same.
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            format!("{:?}", self.build_display_list(PaintMode::Live { keep: false, motion_at }).items()),
            format!("{:?}", dl.items()),
            "a change did not reach a kept drawing"
        );
        self.frames_built.set(self.frames_built.get() + 1);
        *self.last_frame.borrow_mut() = still.then(|| (self.paint_gen, self.viewport, dl.clone()));
        dl
    }

    pub(crate) fn discard_cached_frame(&mut self) {
        self.last_frame.get_mut().take();
    }

    /// Samples the prepared frame with native placeholders.
    pub(crate) fn capture_display_list(&self) -> DisplayList {
        self.build_display_list(PaintMode::Capture)
    }

    fn sampled_image(&self, image: &Image) -> Sample {
        self.image_frames.get(&image.id()).cloned().unwrap_or_else(|| image.sample())
    }

    /// Display lists built rather than reused.
    pub fn frames_built(&self) -> u64 {
        self.frames_built.get()
    }

    /// Own drawings built and kept ones used, and glyph runs built, since the kernel started.
    pub fn drawings(&self) -> (u64, u64, u64) {
        (self.drawings_built.get(), self.drawings_reused.get(), self.glyph_runs_built.get())
    }

    pub(crate) fn set_custom_paint(&mut self, id: NodeId, program: &PaintProgram, params: [f32; 4]) -> bool {
        let n = self.node_mut(id);
        if let Some(binding) = &mut n.custom_paint
            && Weak::ptr_eq(&binding.program.downgrade(), &program.downgrade())
        {
            if binding.params.map(f32::to_bits) == params.map(f32::to_bits) {
                return false;
            }
            binding.params = params;
        } else {
            n.custom_paint = Some(CustomPaintBinding { program: program.clone(), params, mount: PaintMount::default() });
        }
        self.changed(id, Dirty::PAINT, "paint", || format!("{params:?}"));
        true
    }

    /// The frame: kept drawings used where nothing they are drawn from changed (`keep`), else
    /// every drawing built again (without keeping it, and without counting it: a check).
    fn build_display_list(&self, mode: PaintMode) -> DisplayList {
        let runs = self.glyph_runs_built.get();
        let dl = self.assemble(mode);
        if !mode.keeps_drawings() {
            self.glyph_runs_built.set(runs);
        }
        dl
    }

    fn assemble(&self, mode: PaintMode) -> DisplayList {
        let mut parts = Vec::new();
        self.update_ink();
        self.update_motion();
        let window = Rect { x: 0.0, y: 0.0, w: self.viewport.0, h: self.viewport.1 };
        self.paint(self.root, (0.0, 0.0), 1.0, window, &mut parts, mode);
        DisplayList { size: self.viewport, background: self.res.theme.borrow().get(Token::Background), parts }
    }

    /// Notes the nodes moving now (their frames, colors or shadows) and their ancestors, and the
    /// parents of nodes on their way out: their subtrees do not reach where they rest.
    pub(crate) fn update_motion(&self) {
        let mut set = self.in_motion.borrow_mut();
        set.clear();
        let t = self.now;
        let mut mark = |mut n: Option<NodeId>| {
            while let Some(id) = n {
                if !set.insert(id) {
                    break;
                }
                n = self.node(id).parent;
            }
        };
        for id in &self.moving {
            if self.is_alive(*id) && self.node(*id).anim.as_ref().is_some_and(|a| a.rest_at(t).is_some()) {
                mark(Some(*id));
            }
        }
        for &id in self.anchor_offsets.keys() {
            if self.is_mounted(id) {
                mark(Some(id));
            }
        }
        for g in &self.ghosts {
            if self.is_alive(*g) {
                mark(Some(*g));
            }
        }
    }

    /// Where the node is shown now (moving layout included), given where its parent's content
    /// starts in the window (`origin`: the parent's position less its scroll). Walking down the
    /// tree, each node's place costs the same however deep it is.
    fn rect_at(&self, id: NodeId, origin: (f32, f32)) -> Rect {
        let (x, y, w, h) = self.shown_frame(id);
        Rect { x: origin.0 + x, y: origin.1 + y, w, h }
    }

    /// Where the children of a node shown at `rect` start (its scroll applied).
    fn content_origin(&self, id: NodeId, rect: Rect) -> (f32, f32) {
        let (sx, sy) = self.node(id).scroll;
        (rect.x - sx, rect.y - sy)
    }

    pub(super) fn clips_children(&self, id: NodeId) -> bool {
        let overflow = self.node(id).layout.overflow;
        overflow.x != taffy::Overflow::Visible || overflow.y != taffy::Overflow::Visible
    }

    /// The rectangular clip used for children of an overflow box, in the space of `rect`.
    pub(super) fn padding_box(&self, id: NodeId, rect: Rect) -> Rect {
        let border = self.taffy.layout(self.node(id).taffy).expect("taffy").border;
        Rect {
            x: rect.x + border.left,
            y: rect.y + border.top,
            w: (rect.w - border.left - border.right).max(0.0),
            h: (rect.h - border.top - border.bottom).max(0.0),
        }
    }

    /// The row's presented rectangle in its parent's viewport, when some of the box can paint.
    pub(super) fn visible_exit_rect(&self, id: NodeId, parent: NodeId) -> Option<Rect> {
        let (x, y, w, h) = self.shown_rect(id);
        let window = Rect::new(0.0, 0.0, self.viewport.0, self.viewport.1);
        let mut visible = window.intersect(&Rect::new(x, y, w, h));
        let mut current = Some(id);
        while let Some(node) = current {
            let n = self.node(node);
            if visible.is_empty() || n.layout.display == taffy::Display::None || self.presented(node).opacity <= 0.0 {
                return None;
            }
            if node != id && self.clips_children(node) {
                let (x, y, w, h) = self.shown_rect(node);
                visible = visible.intersect(&self.padding_box(node, Rect::new(x, y, w, h)));
            }
            current = n.parent;
        }
        (!visible.is_empty()).then(|| self.rect_in_parent_viewport(id, parent))
    }

    /// Whether `id`'s subtree can be skipped by its ink: not while something in it moves (its
    /// presented positions and shadows differ from the resting ones the reach is computed from).
    fn culls(&self, id: NodeId) -> bool {
        self.culling && !self.in_motion.borrow().contains(&id)
    }

    /// Whether painting and hit testing skip subtrees that cannot show (on by default; off
    /// checks that skipping changes nothing).
    pub fn set_culling(&mut self, on: bool) {
        self.culling = on;
        self.paint_gen += 1;
    }

    /// Whether `id`'s drawing (its subtree's included), shown at `r`, can show inside `clip`.
    fn ink_meets(&self, id: NodeId, r: Rect, clip: &Rect) -> bool {
        if !self.culls(id) {
            return true;
        }
        let [l, t, rr, b] = self.node(id).ink.get();
        !Rect { x: r.x - l, y: r.y - t, w: r.w + l + rr, h: r.h + t + b }.intersect(clip).is_empty()
    }

    /// Computes every node's ink reach again if a layout or a shadow changed since.
    pub(crate) fn update_ink(&self) {
        if self.ink_dirty.replace(false) {
            self.ink_of(self.root);
        }
    }

    /// Where a node's own drawing (not its shadows or children) can be: its box, grown by what
    /// its text draws past it.
    fn content_rect(&self, id: NodeId, rect: Rect) -> Rect {
        let n = self.node(id);
        if n.kind() != Kind::Text {
            return rect;
        }
        let l = self.taffy.layout(n.taffy).expect("taffy");
        let o = l.scrollable_overflow_rect;
        let m = Self::INK_MARGIN;
        let grow = |size: f32, reach: f32| if reach > size { reach + m } else { size };
        Rect { w: grow(rect.w, o.right), h: grow(rect.h, o.bottom), ..rect }
    }

    /// Margin for antialiasing, glyph overhang and strokes on a box's edge.
    const INK_MARGIN: f32 = 2.0;

    fn ink_of(&self, id: NodeId) -> [f32; 4] {
        let n = self.node(id);
        let mut e = [Self::INK_MARGIN; 4];
        if n.layout.display == taffy::Display::None {
            n.ink.set(e);
            return e;
        }
        let l = self.taffy.layout(n.taffy).expect("taffy");
        for s in n.visual.shadow.resolve(&self.res.theme.borrow()) {
            let reach = 1.5 * s.blur_radius + s.spread_radius;
            e[0] = e[0].max(reach - s.offset.0);
            e[1] = e[1].max(reach - s.offset.1);
            e[2] = e[2].max(reach + s.offset.0);
            e[3] = e[3].max(reach + s.offset.1);
        }
        // Text wider or taller than its box (a line that does not wrap) draws past it.
        if n.kind() == Kind::Text {
            let o = l.scrollable_overflow_rect;
            e[2] = e[2].max(o.right - l.size.width + Self::INK_MARGIN);
            e[3] = e[3].max(o.bottom - l.size.height + Self::INK_MARGIN);
        }
        let clips = self.clips_children(id);
        for &c in n.children() {
            let ce = self.ink_of(c);
            if clips {
                continue;
            }
            let cl = self.taffy.layout(self.node(c).taffy).expect("taffy");
            let (cx, cy) = (cl.location.x, cl.location.y);
            e[0] = e[0].max(ce[0] - cx);
            e[1] = e[1].max(ce[1] - cy);
            e[2] = e[2].max(cx + cl.size.width + ce[2] - l.size.width);
            e[3] = e[3].max(cy + cl.size.height + ce[3] - l.size.height);
        }
        n.ink.set(e);
        e
    }

    /// Paints `id` and its subtree. Below full opacity, a subtree that draws more than one thing
    /// goes into a layer composited at that opacity (Rune's group opacity); a single thing is
    /// faded directly.
    fn paint(&self, id: NodeId, origin: (f32, f32), opacity: f32, clip: Rect, out: &mut Vec<Part>, mode: PaintMode) {
        if self.node(id).layout.display == taffy::Display::None {
            return;
        }
        self.paint_rect(id, self.rect_at(id, origin), opacity, clip, out, mode);
    }

    fn paint_rect(&self, id: NodeId, rect: Rect, opacity: f32, clip: Rect, out: &mut Vec<Part>, mode: PaintMode) {
        if !self.ink_meets(id, rect, &clip) {
            return;
        }
        let own = self.presented_at(id, mode.motion_at(self.now)).opacity;
        if own >= 1.0 {
            return self.paint_node(id, rect, opacity, clip, out, mode);
        }
        let k = opacity * own;
        if k <= 0.0 {
            return;
        }
        let mut inner = Vec::new();
        self.paint_node(id, rect, 1.0, clip, &mut inner, mode);
        let draws: usize = inner
            .iter()
            .map(|p| match p {
                Part::Drawing { items, .. } => items.iter().filter(|i| i.draws()).count(),
                Part::Item(i) => usize::from(i.draws()),
            })
            .sum();
        if draws <= 1 {
            for mut p in inner {
                match &mut p {
                    Part::Drawing { fade, .. } => *fade *= k,
                    Part::Item(i) => i.fade(k),
                }
                out.push(p);
            }
        } else {
            out.push(Part::Item(Item::PushLayer { source: id, opacity: k }));
            out.extend(inner);
            out.push(Part::Item(Item::PopLayer));
        }
    }

    /// Paints `id` (its own drawing, kept or built) and its subtree, fading colors by `opacity`
    /// (the node's own opacity is the caller's to apply).
    fn paint_node(&self, id: NodeId, rect: Rect, opacity: f32, clip: Rect, out: &mut Vec<Part>, mode: PaintMode) {
        let n = self.node(id);
        let l = self.taffy.layout(n.taffy).expect("taffy");
        let motion_at = mode.motion_at(self.now);
        // Its own drawing shows: the box, a line of text running past it, or a shadow.
        let shadows_show = || {
            (n.visual.shadow != super::style::Shadows::None || n.anim.is_some())
                && self.presented_shadows_at(id, motion_at).iter().any(|s| {
                    let reach = 1.5 * s.blur_radius + s.spread_radius;
                    let r = Rect {
                        x: rect.x + s.offset.0 - reach,
                        y: rect.y + s.offset.1 - reach,
                        w: rect.w + 2.0 * reach,
                        h: rect.h + 2.0 * reach,
                    };
                    !r.intersect(&clip).is_empty()
                })
        };
        let visible = !self.content_rect(id, rect).intersect(&clip).is_empty() || shadows_show();
        if visible {
            // Fields, canvases and native views draw where they are (what they show moves with
            // more than their own changes); the others in their own coordinates, kept.
            let local = n.kind() != Kind::TextInput && !matches!(n.content, super::Content::Canvas(_) | super::Content::Native(_));
            let items = if local { self.own_drawing(id, rect, mode) } else { self.draw_own(id, rect, mode).into() };
            if !items.is_empty() {
                let offset = if local { (rect.x, rect.y) } else { (0.0, 0.0) };
                out.push(Part::Drawing { offset, fade: opacity, items });
            }
        }

        // Children, clipped to the padding box when the node scrolls or clips.
        let mut ghosts = self.ghosts_of(id);
        if n.children().is_empty() && ghosts.is_empty() {
            return;
        }
        let clips = self.clips_children(id);
        let mut child_clip = clip;
        if clips {
            let v = self.presented_at(id, motion_at);
            let inner = self.padding_box(id, rect);
            child_clip = clip.intersect(&inner);
            if child_clip.is_empty() {
                return;
            }
            out.push(Part::Item(Item::PushClip { source: id, rect: inner, radius: (v.radius - l.border.left).max(0.0) }));
        }
        let origin = self.content_origin(id, rect);
        let paint_exit = |exit, out: &mut Vec<Part>| {
            if let Some(bounds) = self.frozen_exit_rect(exit) {
                // The exit keeps its presented place when the live content moves.
                let bounds = Rect { x: rect.x + bounds.x, y: rect.y + bounds.y, ..bounds };
                self.paint_rect(exit, bounds, opacity, child_clip, out, mode);
            } else {
                self.paint(exit, origin, opacity, child_clip, out, mode);
            }
        };
        if id == self.overlay_layer {
            ghosts.sort_by_key(|&exit| {
                let order = self.node(exit).overlay_mount.map(|mount| mount.order);
                (order.is_none(), order)
            });
        }
        let mut exits = ghosts.into_iter().peekable();
        for &child in n.children() {
            // Keep a retired overlay in its original region, below a reopening of that region.
            if let Some(mount) = self.node(child).overlay_mount {
                while let Some(&exit) = exits.peek()
                    && self.node(exit).overlay_mount.is_some_and(|old| old.order <= mount.order)
                {
                    paint_exit(exit, out);
                    exits.next();
                }
            }
            self.paint(child, origin, opacity, child_clip, out, mode);
        }
        for exit in exits {
            paint_exit(exit, out);
        }
        if clips {
            out.push(Part::Item(Item::PopClip));
        }
    }

    /// `id`'s own drawing in its own coordinates: the kept one while what it is drawn from is the
    /// same, else built (and kept, with `keep`).
    fn own_drawing(&self, id: NodeId, rect: Rect, mode: PaintMode) -> Arc<[Item]> {
        let local = Rect { x: 0.0, y: 0.0, ..rect };
        if !mode.keeps_drawings() {
            return self.draw_own(id, local, mode).into();
        }
        let n = self.node(id);
        let layout = self.taffy.layout(n.taffy).expect("taffy");
        let motion_at = mode.motion_at(self.now);
        let mut visual = self.presented_at(id, motion_at);
        visual.opacity = 1.0;
        let key = DrawKey {
            ver: n.paint_ver,
            exiting_paint: n.custom_paint.is_some() && !n.attached,
            size: (rect.w.to_bits(), rect.h.to_bits()),
            layout_size: layout.size,
            padding: layout.padding,
            border: layout.border,
            visual,
            shadows: if n.visual.shadow == super::style::Shadows::None && n.anim.is_none() {
                Vec::new()
            } else {
                self.presented_shadows_at(id, motion_at)
            },
            theme: self.theme_gen,
            fonts: self.fonts_generation,
        };
        if let Some((k, items)) = &*n.drawn.borrow()
            && *k == key
        {
            self.drawings_reused.set(self.drawings_reused.get() + 1);
            return items.clone();
        }
        let items: Arc<[Item]> = self.draw_own(id, local, mode).into();
        self.drawings_built.set(self.drawings_built.get() + 1);
        *n.drawn.borrow_mut() = Some((key, items.clone()));
        items
    }

    /// `id`'s own drawing with its box at `rect`: shadows, background and border, its image,
    /// icon, native view, canvas, field or text.
    fn draw_own(&self, id: NodeId, rect: Rect, mode: PaintMode) -> Vec<Item> {
        let mut items = Vec::new();
        let out = &mut items;
        let opacity = 1.0;
        let n = self.node(id);
        let l = self.taffy.layout(n.taffy).expect("taffy");
        let visible = true;
        let motion_at = mode.motion_at(self.now);
        let shown = self.presented_at(id, motion_at);
        let v = &shown;

        // Shadows under the box, the first listed on top.
        let radius = v.radius.min(rect.w / 2.0).min(rect.h / 2.0).max(0.0);
        for s in self.presented_shadows_at(id, motion_at).iter().rev() {
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
            out.push(Item::Shadow(shadow));
        }

        // Background and border.
        let border = [l.border.top, l.border.right, l.border.bottom, l.border.left];
        let has_border = border.iter().any(|b| *b > 0.0);
        if visible && (n.custom_paint.is_some() || v.background.is_some() || v.gradient.is_some() || has_border) {
            let gradient = v.gradient.map(|g| crate::paint::Gradient {
                from: (fade(g.from.0, opacity), g.from.1),
                to: (fade(g.to.0, opacity), g.to.1),
                ..g
            });
            let quad = Quad {
                source: id,
                rect,
                radius,
                fill: v.background.map(|c| fade(c, opacity)),
                gradient,
                border,
                border_color: fade(v.border, opacity),
            };
            out.push(match &n.custom_paint {
                Some(binding) => Item::Custom(CustomPaint {
                    fallback: quad,
                    program: binding.program.clone(),
                    mount: binding.mount.clone(),
                    exiting: !n.attached,
                    params: binding.params,
                    opacity,
                }),
                None => Item::Quad(quad),
            });
        }

        // An image, fitted into the padding box and clipped to the box's corners.
        if let super::Content::Image(img, fit) = &n.content
            && visible
        {
            let image = self.sampled_image(img);
            let frame = image.frame();
            let inner = Rect {
                x: rect.x + l.border.left,
                y: rect.y + l.border.top,
                w: (rect.w - l.border.left - l.border.right).max(0.0),
                h: (rect.h - l.border.top - l.border.bottom).max(0.0),
            };
            let (x, y, w, h) = fit.place((frame.width as f32, frame.height as f32), (inner.x, inner.y, inner.w, inner.h));
            if w > 0.0 && h > 0.0 {
                out.push(Item::Image(crate::paint::ImagePaint {
                    source: id,
                    image,
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

        // A native view's drawing and its place in paint order.
        if let super::Content::Native(nv) = &n.content
            && visible
        {
            let live = matches!(mode, PaintMode::Live { .. }) && nv.live;
            if !live {
                out.push(Item::Quad(Quad {
                    source: id,
                    rect,
                    radius,
                    fill: Some(fade(self.res.theme.borrow().get(Token::Surface), opacity)),
                    gradient: None,
                    border: [1.0; 4],
                    border_color: fade(self.res.theme.borrow().get(Token::Border), opacity),
                }));
            }
            let cut = live && nv.slot == crate::native::Slot::Underlay;
            // Capture replaces the placeholder immediately before this marker, without changing
            // the part's offset, fade or surrounding clips and opacity layers.
            out.push(Item::Native { source: id, rect, radius, slot: nv.slot, cut });
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
            self.paint_canvas(id, ops, content, (v.text_style(), v.text), out);
        }

        // A text input: its line, selection, composition and caret.
        if n.kind() == Kind::TextInput && visible {
            self.paint_input(id, opacity, out);
        }
        // A label's text.
        if n.kind() == Kind::Text && visible {
            let (s, style) = (n.text(), v.text_style());
            let geometry = self.text_box(id);
            let cx = rect.x + geometry.origin.0;
            let cy = rect.y + geometry.origin.1;
            if !s.is_empty() {
                // Cut text (one line, or `line_clamp`) ends in an ellipsis inside the box.
                let presentation = self.text_presentation(id);
                let runs = &presentation.glyphs;
                // A truncated line is clipped to the box (an ellipsis comes with the renderer).
                let clip_text = style.wrap == Wrap::Truncate;
                if clip_text {
                    out.push(Item::PushClip { source: id, rect: Rect { x: cx, y: cy, w: geometry.width, h: rect.h }, radius: 0.0 });
                }
                // A document selection, behind the glyphs (as a field draws its own).
                if n.selected.is_some() {
                    let fill = fade(self.res.theme.borrow().get(Token::Accent).multiply_alpha(0.3), opacity);
                    for r in self.selected_rects(id) {
                        out.push(Item::Quad(crate::paint::Quad {
                            source: id,
                            rect: Rect { x: rect.x + r.x, y: rect.y + r.y, w: r.w, h: r.h },
                            radius: 0.0,
                            fill: Some(fill),
                            gradient: None,
                            border: [0.0; 4],
                            border_color: fill,
                        }));
                    }
                }
                self.paint_runs(id, runs, (cx, cy), v.text, opacity, out);
                if clip_text {
                    out.push(Item::PopClip);
                }
            }
        }

        items
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
        out: &mut Vec<Item>,
    ) {
        use crate::canvas::CanvasOp;
        use crate::kernel::Fill;
        let opacity = 1.0;
        let origin = Transform::translate(content.x, content.y);
        let at = |r: Rect| Rect { x: r.x + content.x, y: r.y + content.y, ..r };
        let color = |p: crate::kernel::Paint| fade(p.resolve(&self.res.theme.borrow()).0, opacity);
        out.push(Item::PushClip { source: id, rect: content, radius: 0.0 });
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
                    out.push(Item::Image(crate::paint::ImagePaint {
                        source: id,
                        image: self.sampled_image(image),
                        rect: r,
                        bounds: r,
                        radius: 0.0,
                        opacity,
                    }));
                }
                CanvasOp::Text(t) => {
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
                    // Found by content in the engine: a canvas redrawing the same text shapes nothing.
                    let layout = self.res.text.borrow_mut().layout(&t.text, &style, &[], None, 0);
                    let m = text::metrics(&layout, &style, limit);
                    let runs = layout.present(limit, None, None, None).glyphs;
                    let along = |a: Align, len: f32| match a {
                        Align::Start => 0.0,
                        Align::Center => len / 2.0,
                        Align::End => len,
                    };
                    let x = content.x + t.at.x - along(t.align, m.width);
                    let y = content.y + t.at.y - along(t.valign, m.height);
                    let color = t.color.map_or(fade(text_color, opacity), color);
                    for r in runs {
                        let glyphs = r.glyphs.into_iter().map(|g| crate::paint::Glyph { id: g.id, x: g.x, y: g.y }).collect();
                        out.push(Item::Glyphs(GlyphRun {
                            source: id,
                            font: r.font,
                            size: r.size,
                            coords: r.coords,
                            embolden: r.embolden,
                            color,
                            origin: (x, y),
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
        self.update_ink();
        let window = Rect { x: 0.0, y: 0.0, w: self.viewport.0, h: self.viewport.1 };
        let mut hit = None;
        self.hit(self.root, (0.0, 0.0), x, y, window, &mut hit);
        hit
    }

    fn hit(&self, id: NodeId, origin: (f32, f32), x: f32, y: f32, clip: Rect, hit: &mut Option<NodeId>) {
        let n = self.node(id);
        if n.layout.display == taffy::Display::None {
            return;
        }
        let inside = |r: &Rect| x >= r.x && x < r.right() && y >= r.y && y < r.bottom();
        let rect = self.rect_at(id, origin);
        // Nothing in a subtree whose reach misses the point can be hit.
        if !self.ink_meets(id, rect, &Rect { x, y, w: 1.0, h: 1.0 }) {
            return;
        }
        // A link takes the pointer on the lines it covers, not its whole box.
        let on_link = || self.link_rects(id).iter().any(|r| inside(&Rect { x: origin.0 + r.x, y: origin.1 + r.y, w: r.w, h: r.h }));
        let is_link = matches!(n.content, super::Content::Link(_));
        if inside(&rect) && inside(&clip) && id != self.overlay_layer && (!is_link || on_link()) {
            *hit = Some(id);
        }
        let mut child_clip = clip;
        if n.layout.overflow.x != taffy::Overflow::Visible || n.layout.overflow.y != taffy::Overflow::Visible {
            child_clip = clip.intersect(&rect);
        }
        let origin = self.content_origin(id, rect);
        for &c in n.children() {
            self.hit(c, origin, x, y, child_clip, hit);
        }
    }
}

impl Kernel {
    /// What is hit before native view `view` inside `area`: the boxes (clipped) of the nodes
    /// after it in hit order (painted later) that overlap it, as [`hit_test`](Self::hit_test)
    /// would find them topmost there.
    fn covering(&self, view: NodeId, area: Rect) -> Vec<Rect> {
        let window = Rect { x: 0.0, y: 0.0, w: self.viewport.0, h: self.viewport.1 };
        let mut out = Vec::new();
        let mut passed = false;
        self.cover(self.root, (0.0, 0.0), window, view, &area, &mut passed, &mut out);
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn cover(&self, id: NodeId, origin: (f32, f32), clip: Rect, view: NodeId, area: &Rect, passed: &mut bool, out: &mut Vec<Rect>) {
        let n = self.node(id);
        if n.layout.display == taffy::Display::None {
            return;
        }
        let rect = self.rect_at(id, origin);
        if id == view {
            *passed = true;
            return;
        }
        if *passed && id != self.overlay_layer {
            let r = rect.intersect(&clip).intersect(area);
            if !r.is_empty() {
                out.push(r);
            }
        }
        // A later subtree that cannot reach the view's area adds nothing; an earlier one may
        // still hold the view.
        if *passed && !self.ink_meets(id, rect, area) {
            return;
        }
        let mut child_clip = clip;
        if n.layout.overflow.x != taffy::Overflow::Visible || n.layout.overflow.y != taffy::Overflow::Visible {
            child_clip = clip.intersect(&rect);
        }
        let origin = self.content_origin(id, rect);
        for &c in n.children() {
            self.cover(c, origin, child_clip, view, area, passed, out);
        }
    }

    /// Where each native view goes now: its box, corner radius, the clip around it (the
    /// innermost clipping ancestor's padding box, intersected with the outer ones, rounded by
    /// the innermost), its opacity and whether any of it shows.
    pub fn native_placements(&self) -> Vec<(NodeId, crate::native::Placement)> {
        let window = Rect { x: 0.0, y: 0.0, w: self.viewport.0, h: self.viewport.1 };
        let mut out = Vec::new();
        for id in self.native_nodes() {
            let n = self.node(id);
            let Some(nv) = self.native(id) else { continue };
            let (x, y, w, h) = self.shown_rect(id);
            let rect = Rect { x, y, w, h };
            let own = self.presented(id);
            let radius = own.radius.min(w / 2.0).min(h / 2.0).max(0.0);
            let (mut clip, mut clip_radius, mut opacity) = (window, 0.0f32, own.opacity);
            let mut shown = n.layout.display != taffy::Display::None && n.attached;
            let mut inner = true;
            let mut p = n.parent;
            while let Some(a) = p {
                let an = self.node(a);
                shown &= an.layout.display != taffy::Display::None;
                let v = self.presented(a);
                opacity *= v.opacity;
                if self.clips_children(a) {
                    let (ax, ay, aw, ah) = self.shown_rect(a);
                    let l = self.taffy.layout(an.taffy).expect("taffy");
                    let pad = self.padding_box(a, Rect::new(ax, ay, aw, ah));
                    clip = clip.intersect(&pad);
                    if inner {
                        clip_radius = (v.radius - l.border.left).max(0.0);
                        inner = false;
                    }
                }
                p = an.parent;
            }
            let visible = shown && opacity > 0.0 && !rect.intersect(&clip).is_empty();
            let covered =
                if nv.slot == crate::native::Slot::Underlay && visible { self.covering(id, rect.intersect(&clip)) } else { Vec::new() };
            out.push((
                id,
                crate::native::Placement {
                    id: crate::native::NativeId(super::a11y::a11y_id(id).0),
                    slot: nv.slot,
                    rect,
                    radius,
                    clip,
                    clip_radius,
                    opacity,
                    visible,
                    covered,
                },
            ));
        }
        out
    }
}
