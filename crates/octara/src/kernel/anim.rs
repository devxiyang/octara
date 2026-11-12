//! Property motion in the kernel: when a node with `motion` resolves to new visual values, its
//! tracks move from what is shown toward them; painting samples the tracks at the kernel's time.
//! Views and bindings never rerun for motion.

use super::{BoxShadow, Color, Kernel, NodeId, Visual};
use crate::motion::{Curve, PropClass, Track};
use color::{AlphaColor, Oklab, Srgb};

/// A color as four tracks: premultiplied OKLab (L·α, a·α, b·α, α), so a fade to or from
/// transparent does not pass through gray (Rune).
#[derive(Clone, Copy, Debug)]
struct ColorTrack([Track; 4]);

fn lab(c: Option<Color>) -> [f64; 4] {
    match c {
        None => [0.0; 4],
        Some(c) => {
            let [l, a, b, al] = c.convert::<Oklab>().components;
            [(l * al) as f64, (a * al) as f64, (b * al) as f64, al as f64]
        }
    }
}

fn from_lab(v: [f64; 4]) -> Option<Color> {
    let al = v[3].clamp(0.0, 1.0);
    if al <= 1e-6 {
        return None;
    }
    let c = AlphaColor::<Oklab>::new([(v[0] / al) as f32, (v[1] / al) as f32, (v[2] / al) as f32, al as f32]);
    let mut s = c.convert::<Srgb>();
    s.components = [s.components[0].clamp(0.0, 1.0), s.components[1].clamp(0.0, 1.0), s.components[2].clamp(0.0, 1.0), al as f32];
    Some(s)
}

impl ColorTrack {
    fn new(c: Option<Color>) -> ColorTrack {
        ColorTrack(lab(c).map(Track::new))
    }
    fn retarget(&mut self, c: Option<Color>, t: f64, curve: Curve) {
        for (track, v) in self.0.iter_mut().zip(lab(c)) {
            track.retarget(v, t, curve, PropClass::Color);
        }
    }
    fn sample(&self, t: f64) -> Option<Color> {
        from_lab(self.0.map(|tr| tr.value(t)))
    }
    fn rest_at(&self, t: f64) -> Option<f64> {
        self.0.iter().filter_map(|tr| tr.rest_at(t)).reduce(f64::max)
    }
    fn land(&mut self) {
        for tr in &mut self.0 {
            tr.set(tr.target());
        }
    }
}

/// A node's moving properties.
#[derive(Clone, Debug)]
pub(crate) struct Anim {
    background: ColorTrack,
    text: ColorTrack,
    border: ColorTrack,
    opacity: Track,
    /// Shadows move from `shadow_from` to `shadow_to` as `shadow_phase` goes 0 → 1.
    shadow_from: Vec<BoxShadow>,
    shadow_to: Vec<BoxShadow>,
    shadow_phase: Track,
    /// Position (relative to the parent) and size: x, y, width, height. Set at the node's first
    /// layout; moves on later ones.
    frame: Option<[Track; 4]>,
}

fn lerp(a: f32, b: f32, k: f32) -> f32 {
    a + (b - a) * k
}

/// The shadows between two lists, pairwise (a missing one is a transparent copy of the other).
fn mix_shadows(from: &[BoxShadow], to: &[BoxShadow], k: f32) -> Vec<BoxShadow> {
    let n = from.len().max(to.len());
    let clear = |s: &BoxShadow| BoxShadow { color: s.color.with_alpha(0.0), ..*s };
    (0..n)
        .map(|i| {
            let (a, b) = match (from.get(i), to.get(i)) {
                (Some(a), Some(b)) => (*a, *b),
                (Some(a), None) => (*a, clear(a)),
                (None, Some(b)) => (clear(b), *b),
                (None, None) => unreachable!(),
            };
            let pa = lab(Some(a.color));
            let pb = lab(Some(b.color));
            let c: [f64; 4] = std::array::from_fn(|j| pa[j] + (pb[j] - pa[j]) * k as f64);
            BoxShadow {
                color: from_lab(c).unwrap_or(Color::TRANSPARENT),
                offset: (lerp(a.offset.0, b.offset.0, k), lerp(a.offset.1, b.offset.1, k)),
                blur_radius: lerp(a.blur_radius, b.blur_radius, k).max(0.0),
                spread_radius: lerp(a.spread_radius, b.spread_radius, k),
            }
        })
        .collect()
}

impl Anim {
    fn at(v: &Visual, shadows: Vec<BoxShadow>) -> Anim {
        Anim {
            background: ColorTrack::new(v.background),
            text: ColorTrack::new(Some(v.text)),
            border: ColorTrack::new(Some(v.border)),
            opacity: Track::new(v.opacity as f64),
            shadow_from: shadows.clone(),
            shadow_to: shadows,
            shadow_phase: Track::new(1.0),
            frame: None,
        }
    }
    fn shadows(&self, t: f64) -> Vec<BoxShadow> {
        let k = self.shadow_phase.value(t) as f32;
        if k >= 1.0 { self.shadow_to.clone() } else { mix_shadows(&self.shadow_from, &self.shadow_to, k) }
    }
    /// When everything has come to rest, if anything moves at `t`.
    pub(crate) fn rest_at(&self, t: f64) -> Option<f64> {
        let frame = self.frame.iter().flatten().filter_map(|tr| tr.rest_at(t)).reduce(f64::max);
        [
            self.background.rest_at(t),
            self.text.rest_at(t),
            self.border.rest_at(t),
            self.opacity.rest_at(t),
            self.shadow_phase.rest_at(t),
            frame,
        ]
        .into_iter()
        .flatten()
        .reduce(f64::max)
    }
    fn land(&mut self) {
        self.background.land();
        self.text.land();
        self.border.land();
        self.opacity.set(self.opacity.target());
        self.shadow_phase.set(1.0);
        self.shadow_from = self.shadow_to.clone();
        if let Some(f) = &mut self.frame {
            for tr in f {
                tr.set(tr.target());
            }
        }
    }
}

impl Kernel {
    /// The time the kernel shows (seconds on the app's clock).
    pub fn set_time(&mut self, seconds: f64) {
        // A secure field's shown character hides now: what it shows changes.
        if let Some((id, until)) = self.revealing
            && self.now < until
            && seconds >= until
        {
            self.revealing = None;
            if self.is_alive(id) {
                self.hide_typed(id);
            }
        }
        // The focused field's caret shows or hides now: the frame changes.
        if let Some(t) = self.caret_deadline()
            && seconds >= t
        {
            self.paint_gen += 1;
            if let Some((f, _)) = self.caret {
                self.node_mut(f).paint_ver += 1;
            }
        }
        self.now = seconds;
    }
    pub fn time(&self) -> f64 {
        self.now
    }
    /// Changes in the current frame land at once (an `instant` transaction, direct manipulation).
    pub fn set_instant(&mut self, on: bool) {
        self.instant = on;
    }
    /// Reduce Motion: spatial motion lands at once, opacity and colors fade briefly.
    pub fn set_reduced_motion(&mut self, on: bool) {
        self.res.reduced_motion.set(on);
    }
    pub fn reduced_motion(&self) -> bool {
        self.res.reduced_motion.get()
    }

    fn curve(&self, c: Curve, class: PropClass) -> Curve {
        if self.instant {
            Curve::Instant
        } else if self.res.reduced_motion.get() {
            class.reduced(c)
        } else {
            c
        }
    }

    /// A node resolved from `old` to `new` (`first`: its first resolve): its tracks move toward
    /// the new values, from what is shown now.
    pub(crate) fn animate(&mut self, id: NodeId, old: &Visual, new: &Visual, first: bool) {
        let m = new.motion;
        if m.is_none() && self.node(id).anim.is_none() {
            return;
        }
        let new_shadows = new.shadow.resolve(&self.res.theme.borrow());
        if !matches!(m.layout, Curve::Instant) {
            self.framed.insert(id);
        }
        if first {
            if !m.is_none() {
                self.node_mut(id).anim = Some(Box::new(Anim::at(new, new_shadows)));
            }
            return;
        }
        let t = self.now;
        let (color, opacity) = (self.curve(m.color, PropClass::Color), self.curve(m.opacity, PropClass::Opacity));
        let old_shadows = old.shadow.resolve(&self.res.theme.borrow());
        let n = self.node_mut(id);
        let a = n.anim.get_or_insert_with(|| Box::new(Anim::at(old, old_shadows)));
        a.background.retarget(new.background, t, color);
        a.text.retarget(Some(new.text), t, color);
        a.border.retarget(Some(new.border), t, color);
        a.opacity.retarget(new.opacity as f64, t, opacity, PropClass::Opacity);
        if new_shadows != a.shadow_to {
            a.shadow_from = a.shadows(t);
            a.shadow_to = new_shadows;
            a.shadow_phase.set(0.0);
            a.shadow_phase.retarget(1.0, t, color, PropClass::Opacity);
        }
        if a.rest_at(t).is_some() {
            self.moving.insert(id);
        }
    }

    /// After a layout: nodes whose layout moves start toward their new frame (position relative
    /// to the parent, and size) from where they are shown; a first layout sets it.
    pub(crate) fn track_frames(&mut self) {
        let t = self.now;
        let ids: Vec<NodeId> = self.framed.iter().copied().collect();
        for id in ids {
            if !self.is_alive(id) {
                self.framed.remove(&id);
                continue;
            }
            let l = *self.taffy.layout(self.node(id).taffy).expect("taffy");
            let target = [l.location.x as f64, l.location.y as f64, l.size.width as f64, l.size.height as f64];
            let curve = self.curve(self.node(id).visual.motion.layout, PropClass::Spatial);
            let Some(a) = self.node_mut(id).anim.as_mut() else { continue };
            match &mut a.frame {
                None => a.frame = Some(target.map(Track::new)),
                Some(f) => {
                    for (tr, v) in f.iter_mut().zip(target) {
                        tr.retarget(v, t, curve, PropClass::Spatial);
                    }
                }
            }
            if a.rest_at(t).is_some() {
                self.moving.insert(id);
            }
        }
    }

    /// Scrolls `id` to `(x, y)` as a program asks (not as a finger or a wheel moves it): where its
    /// layout moves (`layout_motion`), it glides there; a new request while gliding keeps the
    /// speed (Rune). Elsewhere it jumps.
    pub fn glide_scroll(&mut self, id: NodeId, x: f32, y: f32) {
        let (cw, ch) = self.client_size(id);
        let (sw, sh) = self.scroll_size(id);
        let to = (x.clamp(0.0, (sw - cw).max(0.0)) as f64, y.clamp(0.0, (sh - ch).max(0.0)) as f64);
        let curve = self.curve(self.node(id).visual.motion.layout, PropClass::Spatial);
        if matches!(curve, Curve::Instant) {
            self.glides.remove(&id);
            self.scroll_to(id, to.0 as f32, to.1 as f32);
            return;
        }
        // Where it is going is where someone put it: the tail is followed if that is the end.
        if self.node(id).follow.is_some() {
            self.node_mut(id).follow = Some(to.1 >= (sh - ch - 1.0) as f64);
        }
        let (t, now) = (self.now, self.node(id).scroll);
        let tracks = self.glides.entry(id).or_insert_with(|| [Track::new(now.0 as f64), Track::new(now.1 as f64)]);
        tracks[0].retarget(to.0, t, curve, PropClass::Spatial);
        tracks[1].retarget(to.1, t, curve, PropClass::Spatial);
    }

    /// The user scrolled `id` (direct manipulation): a glide in progress stops where it is.
    pub fn stop_glide(&mut self, id: NodeId) {
        self.glides.remove(&id);
    }

    /// Moves gliding scroll containers to where they are at the kernel's time. Returns whether
    /// any moved.
    pub fn step_glides(&mut self) -> bool {
        let t = self.now;
        let mut moved = false;
        let ids: Vec<NodeId> = self.glides.keys().copied().collect();
        for id in ids {
            if !self.is_alive(id) {
                self.glides.remove(&id);
                continue;
            }
            let [x, y] = self.glides[&id];
            moved |= self.move_scroll(id, x.value(t) as f32, y.value(t) as f32);
            if !x.moving(t) && !y.moving(t) {
                self.glides.remove(&id);
            }
        }
        moved
    }

    /// Whether any layout is moving at the kernel's time (hover holds still meanwhile).
    pub fn layout_in_motion(&self) -> bool {
        let t = self.now;
        self.framed.iter().any(|id| {
            self.is_alive(*id)
                && self.node(*id).anim.as_ref().and_then(|a| a.frame.as_ref()).is_some_and(|f| f.iter().any(|tr| tr.moving(t)))
        })
    }

    pub fn gliding_scrolls(&self) -> bool {
        !self.glides.is_empty()
    }

    /// Structural changes neither enter nor exit while on (a hot reload rebuilding, a virtual
    /// list moving its window).
    pub fn set_quiet(&mut self, on: bool) {
        self.quiet = on;
    }

    /// Whether `id` is on its way out (it, or a subtree it is in, has been removed and is fading).
    pub fn is_exiting(&self, mut id: NodeId) -> bool {
        loop {
            let n = self.node(id);
            if n.exiting {
                return true;
            }
            match n.parent {
                Some(p) => id = p,
                None => return false,
            }
        }
    }

    /// A subtree is being removed: where its opacity moves (a `motion` subtree) and it has been
    /// shown, it leaves the tree now — layout, hit testing, focus, assistive technology and test
    /// ids no longer see it — while it fades out where it was; it is destroyed when it has. Its
    /// destroy calls until then do nothing. Returns whether it is leaving this way.
    pub fn begin_exit(&mut self, id: NodeId) -> bool {
        if self.quiet || !self.is_alive(id) {
            return false;
        }
        let n = self.node(id);
        let Some(parent) = n.parent else { return false };
        let curve = self.curve(n.visual.motion.opacity, PropClass::Opacity);
        if !n.attached || n.born == self.layouts || n.anim.is_none() || matches!(curve, Curve::Instant) {
            return false;
        }
        let pos = self.node(parent).children.iter().position(|c| *c == id).expect("child of parent");
        self.node_mut(parent).children.remove(pos);
        let pt = self.node(parent).taffy;
        self.taffy.remove_child_at_index(pt, pos).expect("taffy");
        if self.node(parent).children.is_empty() && self.node(id).layout.position == taffy::Position::Absolute {
            self.refresh_taffy_node(parent);
        }
        self.set_attached(id, false);
        let t = self.now;
        let n = self.node_mut(id);
        n.exiting = true;
        if let Some(a) = n.anim.as_mut() {
            a.opacity.retarget(0.0, t, curve, PropClass::Opacity);
        }
        self.ghosts.push(id);
        self.moving.insert(id);
        self.counters.structural_ops += 1;
        self.a11y_touch(parent);
        self.a11y_forget(id);
        self.mark(super::Dirty::LAYOUT | super::Dirty::SEMANTICS);
        self.record(id, super::OpKind::Remove { parent });
        true
    }

    /// A node new this frame was put under a node already shown: where its opacity moves, it
    /// fades in.
    pub(crate) fn begin_enter(&mut self, id: NodeId) {
        if self.quiet || self.instant {
            return;
        }
        let n = self.node(id);
        let Some(parent) = n.parent else { return };
        let p = self.node(parent);
        if n.born != self.layouts || !p.attached || p.born == self.layouts {
            return;
        }
        let curve = self.curve(n.visual.motion.opacity, PropClass::Opacity);
        if matches!(curve, Curve::Instant) {
            return;
        }
        let (t, target) = (self.now, n.visual.opacity as f64);
        if let Some(a) = self.node_mut(id).anim.as_mut() {
            a.opacity.set(0.0);
            a.opacity.retarget(target, t, curve, PropClass::Opacity);
            self.moving.insert(id);
        }
    }

    /// Subtrees that have faded out are destroyed.
    fn reap_ghosts(&mut self) {
        let t = self.now;
        let done: Vec<NodeId> =
            self.ghosts.iter().copied().filter(|g| self.node(*g).anim.as_ref().is_none_or(|a| !a.opacity.moving(t))).collect();
        for g in done {
            self.destroy_ghost(g);
        }
    }

    fn destroy_ghost(&mut self, g: NodeId) {
        self.ghosts.retain(|x| *x != g);
        if self.is_alive(g) {
            self.node_mut(g).parent = None;
            self.destroy_rec(g);
        }
    }

    /// Subtrees leaving `parent`, to be drawn after its children.
    pub(crate) fn ghosts_of(&self, parent: NodeId) -> Vec<NodeId> {
        self.ghosts.iter().copied().filter(|g| self.node(*g).parent == Some(parent)).collect()
    }

    /// A node's position relative to its parent and its size, as shown now.
    pub(crate) fn shown_frame(&self, id: NodeId) -> (f32, f32, f32, f32) {
        let n = self.node(id);
        // A link is where its range is in its text.
        if matches!(n.content, super::Content::Link(_)) {
            return self.link_frame(id);
        }
        if let Some(f) = n.anim.as_ref().and_then(|a| a.frame.as_ref())
            && f.iter().any(|tr| tr.moving(self.now))
        {
            let v = f.map(|tr| tr.value(self.now) as f32);
            return (v[0], v[1], v[2].max(0.0), v[3].max(0.0));
        }
        let l = self.taffy.layout(n.taffy).expect("taffy");
        (l.location.x, l.location.y, l.size.width, l.size.height)
    }

    /// Where the node is shown now (logical pixels): its frame and its ancestors', as they move,
    /// with scroll offsets applied. What paint, hit testing and assistive technology use; layout
    /// itself works on the targets (`rect`).
    pub fn shown_rect(&self, id: NodeId) -> (f32, f32, f32, f32) {
        let (mut x, mut y, w, h) = self.shown_frame(id);
        let mut p = self.node(id).parent;
        while let Some(pid) = p {
            let pn = self.node(pid);
            let (px, py, _, _) = self.shown_frame(pid);
            x += px - pn.scroll.0;
            y += py - pn.scroll.1;
            p = pn.parent;
        }
        (x, y, w, h)
    }

    /// What the node shows now: its visual with moving properties sampled at the kernel's time.
    pub(crate) fn presented(&self, id: NodeId) -> Visual {
        let n = self.node(id);
        let mut v = n.visual;
        // At rest a property shows its resolved value exactly (no color round trip).
        if let Some(a) = &n.anim {
            let t = self.now;
            if a.background.rest_at(t).is_some() {
                v.background = a.background.sample(t);
            }
            if a.text.rest_at(t).is_some() {
                v.text = a.text.sample(t).unwrap_or(v.text);
            }
            if a.border.rest_at(t).is_some() {
                v.border = a.border.sample(t).unwrap_or(v.border);
            }
            if a.opacity.moving(t) {
                v.opacity = a.opacity.value(t).clamp(0.0, 1.0) as f32;
            }
        }
        v
    }

    /// The node's shadows as shown now.
    pub(crate) fn presented_shadows(&self, id: NodeId) -> Vec<BoxShadow> {
        let n = self.node(id);
        match &n.anim {
            Some(a) => a.shadows(self.now),
            None => n.visual.shadow.resolve(&self.res.theme.borrow()),
        }
    }

    /// Whether anything is moving at the kernel's time (nodes at rest leave the moving set).
    pub fn in_motion(&mut self) -> bool {
        self.reap_ghosts();
        if !self.glides.is_empty() {
            return true;
        }
        let t = self.now;
        let ids: Vec<NodeId> = self.moving.iter().copied().collect();
        for id in ids {
            let still = !self.is_alive(id) || self.node(id).anim.as_ref().is_none_or(|a| a.rest_at(t).is_none());
            if still {
                self.moving.remove(&id);
            }
        }
        !self.moving.is_empty()
    }

    /// Whether everything shows its resting values at the kernel's time: no property moving, no
    /// scroll gliding, no subtree fading out (whether or not they have been tidied away yet).
    pub(crate) fn at_rest(&self) -> bool {
        let t = self.now;
        let resting = |id: &NodeId| !self.is_alive(*id) || self.node(*id).anim.as_ref().is_none_or(|a| a.rest_at(t).is_none());
        self.moving.iter().all(resting)
            && self.glides.values().all(|[x, y]| x.rest_at(t).is_none() && y.rest_at(t).is_none())
            && self.ghosts.is_empty()
    }

    /// Moving nodes and when each comes to rest (seconds), for `App::pending`.
    pub fn motions(&self) -> Vec<(NodeId, f64)> {
        let t = self.now;
        let mut out: Vec<(NodeId, f64)> = self
            .moving
            .iter()
            .filter(|id| self.is_alive(**id))
            .filter_map(|id| self.node(*id).anim.as_ref().and_then(|a| a.rest_at(t)).map(|r| (*id, r)))
            .collect();
        for (id, [x, y]) in &self.glides {
            if let Some(r) = [x.rest_at(t), y.rest_at(t)].into_iter().flatten().reduce(f64::max) {
                out.push((*id, r));
            }
        }
        out.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        out
    }

    /// Lands every moving property on its target now.
    pub fn land_motion(&mut self) {
        let t = f64::INFINITY;
        let glides: Vec<(NodeId, [Track; 2])> = self.glides.drain().collect();
        for (id, [x, y]) in glides {
            if self.is_alive(id) {
                // A container following its tail lands on its end as it is now.
                let y = if self.is_following_tail(id) { f32::MAX } else { y.value(t) as f32 };
                self.move_scroll(id, x.value(t) as f32, y);
            }
        }
        for g in std::mem::take(&mut self.ghosts) {
            if self.is_alive(g) {
                self.node_mut(g).parent = None;
                self.destroy_rec(g);
            }
        }
        let ids: Vec<NodeId> = self.moving.drain().collect();
        for id in ids {
            if self.is_alive(id)
                && let Some(a) = self.node_mut(id).anim.as_mut()
            {
                a.land();
            }
        }
    }
}
