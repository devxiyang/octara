//! Analytic tracks for resolved visuals and settled geometry.

use octara_model::geometry::Rect;
use octara_model::motion::{Curve, PropClass, Track};
use octara_model::style::{Ring, Visual};
use octara_model::theme::Color;
use octara_tree::NodeId;
use rustc_hash::FxHashMap;
use std::time::Duration;

#[derive(Clone, Debug)]
struct ColorTrack {
    components: [Track; 4],
    transparent: bool,
}

#[derive(Clone, Debug)]
struct RingMotion {
    ring: Ring,
    color: ColorTrack,
}

impl ColorTrack {
    fn new(color: Option<Color>) -> Self {
        Self {
            components: color.unwrap_or(Color::TRANSPARENT).components.map(|part| Track::new(f64::from(part))),
            transparent: color.is_none(),
        }
    }

    fn retarget(&mut self, color: Option<Color>, now: f64, curve: Curve) {
        let old = self.sample(now).unwrap_or(Color::TRANSPARENT);
        let next = color.unwrap_or_else(|| old.with_alpha(0.0));
        if self.transparent
            && !self.moving(now)
            && let Some(color) = color
        {
            self.components = color.with_alpha(0.0).components.map(|part| Track::new(f64::from(part)));
        }
        for (track, next) in self.components.iter_mut().zip(next.components) {
            track.retarget(f64::from(next), now, curve, PropClass::Color);
        }
        self.transparent = color.is_none();
    }

    fn sample(&self, now: f64) -> Option<Color> {
        if self.transparent && !self.moving(now) {
            return None;
        }
        Some(Color::new(self.components.map(|part| part.value(now) as f32)))
    }

    fn moving(&self, now: f64) -> bool {
        self.components.iter().any(|track| track.moving(now))
    }

    fn rest_at(&self) -> impl Iterator<Item = f64> + '_ {
        self.components.iter().filter_map(|track| track.rest_at(0.0))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct NodeMotion {
    target: Rect,
    visual: Visual,
    position: [Track; 4],
    opacity: Track,
    fill: ColorTrack,
    border: ColorTrack,
    text: ColorTrack,
    ring: Option<Box<RingMotion>>,
    transition: Option<(u64, Track)>,
}

impl NodeMotion {
    pub(crate) fn updated(previous: Option<&Self>, target: TargetMotion, options: MotionOptions) -> Self {
        if let Some(previous) = previous {
            let mut previous = previous.clone();
            previous.retarget(target, options);
            previous
        } else {
            Self::new(target.bounds, target.visual, target.transition.map(|(key, _)| key))
        }
    }

    fn new(bounds: Rect, visual: Visual, transition: Option<u64>) -> Self {
        Self {
            target: bounds,
            visual,
            position: [bounds.x, bounds.y, bounds.w, bounds.h].map(|value| Track::new(f64::from(value))),
            opacity: Track::new(f64::from(visual.opacity)),
            fill: ColorTrack::new(visual.background),
            border: ColorTrack::new(Some(visual.border)),
            text: ColorTrack::new(Some(visual.text)),
            ring: visual.ring.map(|ring| Box::new(RingMotion { ring, color: ColorTrack::new(Some(ring.color)) })),
            transition: transition.map(|key| (key, Track::new(1.0))),
        }
    }

    fn retarget(&mut self, value: TargetMotion, options: MotionOptions) {
        let TargetMotion { bounds, visual, transition } = value;
        let MotionOptions { now, reduced, immediate_layout } = options;
        let curve = |class: PropClass, selected| if reduced { class.reduced(selected) } else { selected };
        if self.target != bounds {
            self.target = bounds;
            let layout = if immediate_layout { Curve::Instant } else { curve(PropClass::Spatial, visual.motion.layout) };
            for (track, value) in self.position.iter_mut().zip([bounds.x, bounds.y, bounds.w, bounds.h]) {
                track.retarget(f64::from(value), now, layout, PropClass::Spatial);
            }
        }
        if self.visual != visual {
            let color = curve(PropClass::Color, visual.motion.color);
            self.fill.retarget(visual.background, now, color);
            self.border.retarget(Some(visual.border), now, color);
            self.text.retarget(Some(visual.text), now, color);
            if self.visual.ring != visual.ring
                && let Some(ring) = visual.ring.or(self.visual.ring)
            {
                let track = self.ring.get_or_insert_with(|| Box::new(RingMotion { ring, color: ColorTrack::new(None) }));
                track.ring = ring;
                track.color.retarget(visual.ring.map(|ring| ring.color), now, color);
            }
            self.opacity.retarget(f64::from(visual.opacity), now, curve(PropClass::Opacity, visual.motion.opacity), PropClass::Opacity);
            self.visual = visual;
        }

        match transition {
            Some((key, animation)) if self.transition.is_some_and(|(previous, _)| previous != key) => {
                let mut track = Track::new(0.0);
                track.retarget(1.0, now, curve(PropClass::Opacity, animation), PropClass::Opacity);
                self.transition = Some((key, track));
            }
            Some((key, _)) if self.transition.is_none() => self.transition = Some((key, Track::new(1.0))),
            None => self.transition = None,
            _ => {}
        }
    }

    pub(crate) fn bounds(&self, now: f64) -> Rect {
        let [x, y, w, h] = self.position.map(|value| value.value(now) as f32);
        Rect::new(x, y, w.max(0.0), h.max(0.0))
    }

    pub(crate) fn visual(&self, now: f64) -> Visual {
        Visual {
            opacity: self.opacity.value(now).clamp(0.0, 1.0) as f32,
            background: self.fill.sample(now),
            border: self.border.sample(now).unwrap_or(Color::TRANSPARENT),
            text: self.text.sample(now).unwrap_or(Color::TRANSPARENT),
            ring: self.ring.as_ref().and_then(|track| track.color.sample(now).map(|color| Ring { color, ..track.ring })),
            ..self.visual
        }
    }

    pub(crate) fn transition_progress(&self, now: f64) -> f32 {
        self.transition.map_or(
            1.0,
            |(_, track)| {
                if track.moving(now) { (track.value(now) as f32).clamp(0.0, 1.0_f32.next_down()) } else { 1.0 }
            },
        )
    }

    pub(crate) fn translation_only(&self, now: f64) -> bool {
        self.position[..2].iter().any(|track| track.moving(now))
            && self.position[2..].iter().all(|track| !track.moving(now))
            && self.visual_rest_times().all(|rest| rest <= now)
    }

    pub(crate) fn rest_at(&self) -> Option<f64> {
        self.position.iter().filter_map(|track| track.rest_at(0.0)).chain(self.visual_rest_times()).reduce(f64::max)
    }

    fn visual_rest_times(&self) -> impl Iterator<Item = f64> + '_ {
        self.opacity
            .rest_at(0.0)
            .into_iter()
            .chain(self.fill.rest_at())
            .chain(self.border.rest_at())
            .chain(self.text.rest_at())
            .chain(self.ring.iter().flat_map(|track| track.color.rest_at()))
            .chain(self.transition.and_then(|(_, track)| track.rest_at(0.0)))
    }
}

pub(crate) struct TargetMotion {
    pub bounds: Rect,
    pub visual: Visual,
    pub transition: Option<(u64, Curve)>,
}

#[derive(Clone, Copy)]
pub(crate) struct MotionOptions {
    pub now: f64,
    pub reduced: bool,
    pub immediate_layout: bool,
}

pub(crate) fn next_frame(now: f64) -> Duration {
    Duration::from_secs_f64(now + 1.0 / 60.0)
}

/// A programmatic scroll follows the node's spatial curve and yields to direct input.
#[derive(Default)]
pub(crate) struct ScrollMotion {
    active: FxHashMap<NodeId, ScrollGlide>,
    previous: Option<FxHashMap<NodeId, ScrollGlide>>,
}

#[derive(Clone)]
struct ScrollGlide {
    x: Track,
    y: Track,
    last: octara_model::geometry::Vec2,
}

impl ScrollMotion {
    pub(crate) fn active_nodes<'a>(&'a self, tree: &'a octara_tree::Tree) -> impl Iterator<Item = NodeId> + 'a {
        self.active.iter().filter_map(|(&node, glide)| {
            (tree.is_live(node) && tree.scroll(node).is_some_and(|slot| slot.offset == glide.last)).then_some(node)
        })
    }

    pub(crate) fn commit_stage(&mut self) {
        self.previous = None;
    }

    pub(crate) fn rollback_stage(&mut self) {
        if let Some(previous) = self.previous.take() {
            self.active = previous;
        }
    }

    pub(crate) fn step(
        &mut self,
        tree: &octara_tree::Tree,
        layout: &crate::layout::LayoutEngine,
        updates: &mut Vec<octara_tree::RuntimeOp>,
        now: f64,
        reduced: bool,
    ) -> Option<Duration> {
        use octara_model::geometry::Vec2;
        use octara_tree::RuntimeOp;

        self.previous.get_or_insert_with(|| self.active.clone());
        self.active.retain(|&node, glide| {
            tree.is_live(node) && tree.scroll(node).is_some_and(|slot| slot.offset == glide.last) && layout.scroll_range(node).is_some()
        });
        for update in updates.iter() {
            let RuntimeOp::SetScroll { node, offset, .. } = *update else { continue };
            let Some(slot) = tree.scroll(node).filter(|slot| slot.requested.is_some()) else { continue };
            let curve = tree.visual(node).motion.layout;
            if reduced || curve == Curve::Instant {
                self.active.remove(&node);
                continue;
            }
            let glide = self.active.entry(node).or_insert_with(|| ScrollGlide {
                x: Track::new(f64::from(slot.offset.x)),
                y: Track::new(f64::from(slot.offset.y)),
                last: slot.offset,
            });
            glide.x.retarget(f64::from(offset.x), now, curve, PropClass::Spatial);
            glide.y.retarget(f64::from(offset.y), now, curve, PropClass::Spatial);
        }

        updates.retain(|update| !matches!(update, RuntimeOp::SetScroll { node, .. } if self.active.contains_key(node)));
        let mut nodes: Vec<_> = self.active.keys().copied().collect();
        nodes.sort_unstable();
        for node in nodes {
            let glide = self.active.get_mut(&node).expect("tracked glide");
            let range = layout.scroll_range(node).expect("live scroll range");
            let sample = |track: &Track| if reduced { track.target() as f32 } else { track.value(now) as f32 };
            let offset = range.clamp(Vec2::new(sample(&glide.x), sample(&glide.y)));
            let slot = tree.scroll(node).expect("live scroll slot");
            if offset != slot.offset || slot.requested.is_some() {
                updates.push(RuntimeOp::SetScroll { node, offset, clamped: false });
            }
            glide.last = offset;
            if reduced || (!glide.x.moving(now) && !glide.y.moving(now)) {
                self.active.remove(&node);
            }
        }
        (!self.active.is_empty()).then(|| next_frame(now))
    }
}
