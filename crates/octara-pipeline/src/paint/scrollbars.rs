use super::{CompileOptions, next_frame};
use crate::layout::LayoutEngine;
use octara_model::{
    geometry::{Point, Rect},
    paint::Quad,
    style::ScrollbarVisibility,
    theme::{Color, ColorToken},
};
use octara_tree::{
    DirtyBitmap, NodeFlags, NodeId, Tree,
    arbiter::{ScrollbarAxis, ScrollbarHit, ScrollbarKey, ScrollbarPart},
};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
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

#[derive(Clone, Default)]
struct State {
    interaction: Interaction,
    last_activity: FxHashMap<NodeId, f64>,
    opacity: FxHashMap<NodeId, f32>,
}

#[derive(Default)]
pub(super) struct Scrollbars {
    interaction: Interaction,
    state: State,
    previous: Option<State>,
}

impl Scrollbars {
    pub(super) fn interact(&mut self, hovered: Option<ScrollbarKey>, dragged: Option<ScrollbarKey>) -> bool {
        let previous = std::mem::replace(&mut self.interaction, (hovered, dragged));
        self.interaction != previous
    }

    pub(super) fn step(
        &mut self,
        tree: &Tree,
        layout: &LayoutEngine,
        options: &CompileOptions,
        local: &mut DirtyBitmap,
    ) -> Option<Duration> {
        self.previous.get_or_insert_with(|| self.state.clone());
        let State { interaction, last_activity, opacity } = &mut self.state;
        let highlighted = |(hovered, dragged): Interaction| hovered.into_iter().chain(dragged);
        if *interaction != self.interaction {
            for key in highlighted(*interaction).chain(highlighted(self.interaction)) {
                local.insert(key.node);
                last_activity.insert(key.node, options.now);
            }
            *interaction = self.interaction;
        }
        for node in layout.scrolled().iter() {
            if tree.visual(node).scrollbar == ScrollbarVisibility::Auto {
                last_activity.insert(node, options.now);
            }
        }
        let mut next_deadline = None;
        last_activity.retain(|&node, activity| {
            let engaged = highlighted(*interaction).any(|key| key.node == node);
            let visible = tree.is_shown(node)
                && tree.visual(node).scrollbar == ScrollbarVisibility::Auto
                && layout.scroll_range(node).is_some_and(|range| range.max.x > 0.0 || range.max.y > 0.0);
            let alpha = if !visible {
                0.0
            } else if engaged {
                OPACITY
            } else {
                let start = *activity + IDLE;
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
            let previous = if alpha > 0.0 { opacity.insert(node, alpha) } else { opacity.remove(&node) };
            if previous.unwrap_or_default() != alpha {
                local.insert(node);
            }
            visible && alpha > 0.0
        });
        next_deadline
    }

    pub(super) fn thumbs(&self, tree: &Tree, layout: &LayoutEngine, node: NodeId) -> SmallVec<[Quad; 2]> {
        let opacity = match tree.visual(node).scrollbar {
            ScrollbarVisibility::Auto => self.state.opacity.get(&node).copied().unwrap_or_default(),
            ScrollbarVisibility::Always => OPACITY,
            ScrollbarVisibility::Hidden => return SmallVec::new(),
        };
        if opacity <= 0.0 {
            return SmallVec::new();
        }
        let (hovered, dragged) = self.state.interaction;
        bars(layout, node)
            .map(|bar| {
                let alpha = if Some(bar.key) == hovered || Some(bar.key) == dragged { ENGAGED_OPACITY } else { opacity };
                let (color, _) = ColorToken::TextSubtle.alpha(alpha).resolve(tree.theme_at(node));
                Quad {
                    source: node,
                    rect: bar.thumb,
                    radius: [THICKNESS / 2.0; 4],
                    fill: Some(color),
                    gradient: None,
                    border: [0.0; 4],
                    border_color: Color::TRANSPARENT,
                }
            })
            .collect()
    }

    pub(super) fn commit(&mut self) {
        self.previous = None;
    }

    pub(super) fn rollback(&mut self) {
        if let Some(previous) = self.previous.take() {
            self.state = previous;
        }
    }
}

struct Bar {
    key: ScrollbarKey,
    track: Rect,
    thumb: Rect,
    page: f32,
    scale: f32,
}

pub(super) fn hit(tree: &Tree, layout: &LayoutEngine, node: NodeId, pos: Point) -> Option<ScrollbarHit> {
    if tree.visual(node).scrollbar == ScrollbarVisibility::Hidden
        || tree.node(node).is_some_and(|core| core.flags.contains(NodeFlags::POINTER_EVENTS_NONE))
        || !tree.is_interactive(node)
        || layout.child_clip(node).is_some_and(|clip| !clip.contains(pos))
    {
        return None;
    }
    bars(layout, node).find(|bar| bar.track.contains(pos)).map(|bar| {
        let pos = bar.key.axis.position(pos);
        let (start, end) = match bar.key.axis {
            ScrollbarAxis::Horizontal => (bar.thumb.x, bar.thumb.right()),
            ScrollbarAxis::Vertical => (bar.thumb.y, bar.thumb.bottom()),
        };
        let part = if pos >= start && pos <= end {
            ScrollbarPart::Thumb { scale: bar.scale }
        } else {
            ScrollbarPart::Track { page_delta: if pos < start { -bar.page } else { bar.page } }
        };
        ScrollbarHit { key: bar.key, part }
    })
}

fn bars(layout: &LayoutEngine, node: NodeId) -> impl Iterator<Item = Bar> {
    let port = layout.scroll_client(node).zip(layout.scroll_range(node));
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
        let thumb_pos = start + INSET + travel * (axis.offset(layout.scroll_offset(node)) / scroll);
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
        Some(Bar { key: ScrollbarKey { node, axis }, track, thumb, page, scale: scroll / travel })
    })
}
