use super::{CompileOptions, next_frame};
use crate::layout::LayoutKernel;
use crate::projected::DenseProjected;
use octara_model::{
    geometry::{Point, Rect, Vec2},
    paint::{Item, Part, Quad, ScrollPlacement, ScrollbarPlacement},
    style::ScrollbarVisibility,
    theme::{Color, ColorToken},
};
use octara_tree::{
    DirtyBitmap, NodeFlags, NodeId, RetiredNode, Tree, TreeDelta,
    arbiter::{ScrollbarAxis, ScrollbarHit, ScrollbarKey, ScrollbarPart},
};
use rustc_hash::FxHashSet;
use std::sync::Arc;
use std::time::Duration;

const INSET: f32 = 2.0;
const THICKNESS: f32 = 5.0;
const EDGE: f32 = 12.0;
const MIN_THUMB: f32 = 18.0;
const IDLE: f64 = 0.7;
const FADE: f64 = 0.23;
const OPACITY: f32 = 0.55;
const ENGAGED_OPACITY: f32 = 0.8;

type Interaction = (Option<ScrollbarKey>, Option<ScrollbarKey>);

#[derive(Clone, Copy, PartialEq)]
struct Activity {
    time: f64,
    opacity: f32,
}

#[derive(Default)]
pub(super) struct Scrollbars {
    interaction: Interaction,
    projected: Interaction,
    previous: Option<Interaction>,
    activity: DenseProjected<Activity>,
    active: FxHashSet<NodeId>,
}

impl Scrollbars {
    pub(super) fn interact(&mut self, hovered: Option<ScrollbarKey>, dragged: Option<ScrollbarKey>) -> bool {
        let previous = std::mem::replace(&mut self.interaction, (hovered, dragged));
        self.interaction != previous
    }

    pub(super) fn step(
        &mut self,
        tree: &Tree,
        delta: &TreeDelta,
        layout: &LayoutKernel,
        options: &CompileOptions<'_>,
        local: &mut DirtyBitmap,
    ) -> Option<Duration> {
        self.previous.get_or_insert(self.projected);
        self.activity.begin();
        for &retired in &delta.retired {
            if let RetiredNode::Destroyed(node) = retired {
                self.activity.remove(node);
                self.active.remove(&node);
            }
        }
        let highlighted = |(hovered, dragged): Interaction| hovered.into_iter().chain(dragged);
        if self.projected != self.interaction {
            for key in highlighted(self.projected).chain(highlighted(self.interaction)) {
                local.insert(key.node);
                if tree.node(key.node).is_some() {
                    self.mark_activity(key.node, options.now);
                }
            }
            self.projected = self.interaction;
        }
        for node in layout.scroll_changed().iter() {
            if tree.node(node).is_some() && tree.visual(node).scrollbar == ScrollbarVisibility::Auto {
                self.mark_activity(node, options.now);
            }
        }
        let mut next_deadline = None;
        self.active.retain(|&node| {
            let Some(activity) = self.activity.get(node).copied() else { return false };
            let engaged = highlighted(self.projected).any(|key| key.node == node);
            let visible = tree.is_shown(node)
                && tree.visual(node).scrollbar == ScrollbarVisibility::Auto
                && layout.scroll_range(node).is_some_and(|range| range.max.x > 0.0 || range.max.y > 0.0);
            let alpha = if !visible {
                0.0
            } else if engaged {
                OPACITY
            } else {
                let start = activity.time + IDLE;
                let end = start + if options.reduced_motion { 0.0 } else { FADE };
                let deadline = if options.now < start {
                    Some(Duration::from_secs_f64(start))
                } else if options.now < end {
                    Some(next_frame(options.now).min(Duration::from_secs_f64(end)))
                } else {
                    None
                };
                next_deadline = next_deadline.into_iter().chain(deadline).min();
                if options.now < start {
                    OPACITY
                } else if options.now < end {
                    OPACITY * ((end - options.now) / FADE) as f32
                } else {
                    0.0
                }
            };
            if activity.opacity != alpha {
                local.insert(node);
            }
            let active = visible && alpha > 0.0;
            self.activity.set(node, active.then_some(Activity { opacity: alpha, ..activity }));
            active
        });
        next_deadline
    }

    fn mark_activity(&mut self, node: NodeId, time: f64) {
        let opacity = self.activity.get(node).map_or(0.0, |activity| activity.opacity);
        self.activity.set(node, Some(Activity { time, opacity }));
        self.active.insert(node);
    }

    pub(super) fn placements(
        &self,
        tree: &Tree,
        layout: &LayoutKernel,
        node: NodeId,
        previous: Option<&ScrollPlacement>,
    ) -> [Option<ScrollbarPlacement>; 2] {
        let opacity = match tree.visual(node).scrollbar {
            ScrollbarVisibility::Auto => self.activity.get(node).map_or(0.0, |activity| activity.opacity),
            ScrollbarVisibility::Always => OPACITY,
            ScrollbarVisibility::Hidden => 0.0,
        };
        let Some(bounds) = layout.paint_bounds(node).filter(|_| opacity > 0.0) else { return [None, None] };
        let Some(client) = layout.local_client(node) else { return [None, None] };
        let (hovered, dragged) = self.projected;
        let mut placements = [None, None];
        let client = Rect::new(bounds.x + client.x, bounds.y + client.y, client.w, client.h);
        for bar in bars(layout, node, Some(client)) {
            let alpha = if Some(bar.key) == hovered || Some(bar.key) == dragged { ENGAGED_OPACITY } else { opacity };
            let (color, _) = ColorToken::TextSubtle.alpha(alpha).resolve(tree.theme_at(node));
            let item = Part::Item(Item::Quad(Quad {
                source: node,
                rect: bar.thumb,
                radius: [THICKNESS / 2.0; 4],
                fill: Some(color),
                gradient: None,
                border: [0.0; 4],
                border_color: Color::TRANSPARENT,
            }));
            let axis = usize::from(bar.key.axis == ScrollbarAxis::Horizontal);
            let parts = previous
                .and_then(|previous| previous.scrollbars[axis].as_ref())
                .filter(|previous| previous.parts.as_ref() == std::slice::from_ref(&item))
                .map_or_else(|| Arc::from([item]), |previous| previous.parts.clone());
            placements[axis] = Some(ScrollbarPlacement { offset: bar.translation, parts });
        }
        placements
    }

    pub(super) fn commit(&mut self) {
        self.previous = None;
        self.activity.commit();
    }

    pub(super) fn rollback(&mut self) {
        if let Some(previous) = self.previous.take() {
            self.projected = previous;
            let changed: Vec<_> = self.activity.undo_keys().collect();
            self.activity.rollback();
            for node in changed {
                if self.activity.contains_key(node) {
                    self.active.insert(node);
                } else {
                    self.active.remove(&node);
                }
            }
        }
    }
}

struct Bar {
    key: ScrollbarKey,
    track: Rect,
    thumb: Rect,
    translation: Vec2,
    page: f32,
    scale: f32,
}

pub(super) fn hit(tree: &Tree, layout: &LayoutKernel, node: NodeId, pos: Point) -> Option<ScrollbarHit> {
    if tree.visual(node).scrollbar == ScrollbarVisibility::Hidden
        || tree.node(node).is_some_and(|core| core.flags.contains(NodeFlags::POINTER_EVENTS_NONE))
        || !tree.is_interactive(node)
        || layout.child_clip(node).is_some_and(|clip| !clip.contains(pos))
    {
        return None;
    }
    bars(layout, node, layout.scroll_client(node)).find(|bar| bar.track.contains(pos)).map(|bar| {
        let pos = bar.key.axis.position(pos);
        let (start, end) = match bar.key.axis {
            ScrollbarAxis::Horizontal => (bar.thumb.x, bar.thumb.right()),
            ScrollbarAxis::Vertical => (bar.thumb.y, bar.thumb.bottom()),
        };
        let translation = bar.key.axis.offset(bar.translation);
        let (start, end) = (start + translation, end + translation);
        let part = if pos >= start && pos <= end {
            ScrollbarPart::Thumb { scale: bar.scale }
        } else {
            ScrollbarPart::Track { page_delta: if pos < start { -bar.page } else { bar.page } }
        };
        ScrollbarHit { key: bar.key, part }
    })
}

fn bars(layout: &LayoutKernel, node: NodeId, client: Option<Rect>) -> impl Iterator<Item = Bar> {
    let port = client.zip(layout.scroll_range(node));
    [ScrollbarAxis::Vertical, ScrollbarAxis::Horizontal].into_iter().filter_map(move |axis| {
        let (client, range) = port?;
        let scroll = axis.offset(range.max);
        if scroll <= 0.0 || client.is_empty() {
            return None;
        }
        let (start, edge, page, other) = match axis {
            ScrollbarAxis::Horizontal => (client.x, client.bottom(), client.w, range.max.y),
            ScrollbarAxis::Vertical => (client.y, client.right(), client.h, range.max.x),
        };
        let edge_space = if other > 0.0 { EDGE } else { 0.0 };
        let track_len = page - 2.0 * INSET - edge_space;
        if track_len <= 0.0 {
            return None;
        }
        let thumb_len = (track_len * page / (page + scroll)).max(MIN_THUMB.min(track_len / 2.0));
        let travel = track_len - thumb_len;
        if travel <= 0.0 {
            return None;
        }
        let thumb_pos = start + INSET;
        let translation = travel * (axis.offset(layout.scroll_offset(node)) / scroll);
        let translation = match axis {
            ScrollbarAxis::Horizontal => Vec2::new(translation, 0.0),
            ScrollbarAxis::Vertical => Vec2::new(0.0, translation),
        };
        let (track, thumb) = match axis {
            ScrollbarAxis::Horizontal => (
                Rect::new(start, edge - EDGE.min(client.h), page - edge_space, EDGE.min(client.h)),
                Rect::new(thumb_pos, edge - INSET - THICKNESS, thumb_len, THICKNESS),
            ),
            ScrollbarAxis::Vertical => (
                Rect::new(edge - EDGE.min(client.w), start, EDGE.min(client.w), page - edge_space),
                Rect::new(edge - INSET - THICKNESS, thumb_pos, THICKNESS, thumb_len),
            ),
        };
        Some(Bar { key: ScrollbarKey { node, axis }, track, thumb, translation, page, scale: scroll / travel })
    })
}
