//! Frozen draw lists sampled on the host's presentation clock.

use super::{Decoration, radius, ring_rect};
use crate::motion::NodeMotion;
use octara_model::geometry::{Rect, Size, Vec2};
use octara_model::motion::{TextRevealTrack, Track};
use octara_model::paint::{DisplayList, Item, Part, ScrollPlacement, ScrollPlacements, TransitionPaint};
use octara_model::style::Visual;
use octara_model::theme::Color;
use octara_tree::{ExitTicket, NodeId};
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Arc;

#[derive(Debug)]
pub struct PaintScene {
    pub(super) size: Size,
    pub(super) background: Color,
    pub(super) root: Option<Arc<PaintNode>>,
    pub(super) scrolls: ScrollPlacements,
    pub(super) translations: Arc<[Translation]>,
}

impl PaintScene {
    pub(super) fn matches(&self, size: Size, background: Color, root: Option<&Arc<PaintNode>>, scrolls: &ScrollPlacements) -> bool {
        self.size == size
            && self.background == background
            && self.scrolls.same_snapshot(scrolls)
            && self.root.as_ref().zip(root).map_or(self.root.is_none() && root.is_none(), |(a, b)| Arc::ptr_eq(a, b))
    }

    pub(super) fn timed_at(&self, time: f64) -> bool {
        self.root.as_ref().is_some_and(|root| root.timing.timed_at(time)) || self.translations_at(time)
    }

    pub fn continuously_animated(&self, time: f64) -> bool {
        self.root.as_ref().is_some_and(|root| root.motion_at(time)) || self.translations_at(time)
    }

    fn translations_at(&self, time: f64) -> bool {
        self.translations.last().is_some_and(|translation| time < translation.until)
    }

    fn sample_placements(&self, time: f64) -> ScrollPlacements {
        let mut placements = self.scrolls.clone();
        if self.translations_at(time) {
            let active = self.translations.partition_point(|translation| translation.until <= time);
            for translation in &self.translations[active..] {
                let bounds = translation.motion.bounds(time);
                let offset = Vec2::new(bounds.x - translation.target.x, bounds.y - translation.target.y);
                placements.set(translation.node, Some(ScrollPlacement { offset, scrollbars: [None, None] }));
            }
        }
        placements
    }

    pub fn sample(&self, time: f64) -> Arc<DisplayList> {
        self.sample_counted(time).0
    }

    pub(super) fn sample_counted(&self, time: f64) -> (Arc<DisplayList>, SampleWork) {
        let scrolls = self.sample_placements(time);
        if let Some(root) = &self.root
            && !root.timing.timed_at(time)
        {
            let parts = vec![Part::ScrollFrame { scrolls, parts: root.resting.clone() }];
            return (Arc::new(DisplayList { size: self.size, background: self.background, parts }), SampleWork::default());
        }
        let mut sample = Sample { time, work: SampleWork::default() };
        let mut parts = Vec::new();
        if let Some(root) = &self.root {
            root.emit(&mut sample, &mut parts);
        }
        let parts = if parts.is_empty() { parts } else { vec![Part::ScrollFrame { scrolls, parts: parts.into() }] };
        (Arc::new(DisplayList { size: self.size, background: self.background, parts }), sample.work)
    }
}

#[derive(Debug)]
pub(super) struct Translation {
    pub node: NodeId,
    pub target: Rect,
    pub motion: Arc<NodeMotion>,
    pub until: f64,
}

#[derive(Debug)]
pub(super) struct Element {
    pub source: NodeId,
    pub visual: Visual,
    pub bounds: Rect,
    pub child_clip: Option<(Rect, [bool; 2])>,
    pub scroll: bool,
    pub decoration: Arc<Decoration>,
    pub own: Arc<[Item]>,
    pub content_start: usize,
    pub motion: Option<Arc<NodeMotion>>,
    pub reveal: Option<TextRevealTrack>,
    pub reveal_tint: Option<Color>,
    pub transition: Option<TransitionPaint>,
    pub caret: Option<(Item, f64)>,
}

#[derive(Debug)]
pub(super) struct ExitGhost {
    pub ticket: ExitTicket,
    frozen: Arc<[Part]>,
    pub opacity: Track,
}

impl ExitGhost {
    pub(super) fn capture(
        ticket: ExitTicket,
        snapshot: &PaintNode,
        now: f64,
        opacity: Track,
        independent: &FxHashSet<NodeId>,
        ancestors: &[(NodeId, Option<Item>, Vec2)],
        scrolls: &ScrollPlacements,
    ) -> Option<Self> {
        let element = snapshot.element.as_ref()?;
        let visual = element.motion.as_ref().map_or(element.visual, |track| track.visual(now));
        if visual.opacity <= 0.0 {
            return None;
        }
        let mut parts = Vec::new();
        element.emit(
            now,
            |output| {
                if let Some(children) = &snapshot.children.root {
                    children.emit_for_exit(now, independent, output);
                }
            },
            &mut parts,
        );
        for part in &mut parts {
            mark_exiting(part);
        }
        if parts.is_empty() {
            return None;
        }
        for (source, clip, offset) in ancestors {
            if *offset != Vec2::default() {
                parts =
                    vec![Part::TranslatedGroup { spatial_id: ScrollPlacements::spatial_id(*source), offset: *offset, parts: parts.into() }];
            }
            if let Some(clip) = clip {
                parts.insert(0, Part::Item(clip.clone()));
                parts.push(Part::Item(Item::PopClip));
            }
        }
        let frozen = Arc::from([Part::ScrollFrame { scrolls: scrolls.clone(), parts: parts.into() }]);
        Some(Self { ticket, frozen, opacity })
    }

    pub(super) fn emit(&self, time: f64, output: &mut Vec<Part>) {
        let opacity = self.opacity.value(time).clamp(0.0, 1.0) as f32;
        if opacity > 0.0 {
            output.push(Part::Item(Item::PushLayer { source: self.ticket.node, opacity }));
            append_group(&self.frozen, output);
            output.push(Part::Item(Item::PopLayer));
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Timing {
    motion_until: Option<f64>,
    blinks: bool,
}

impl Timing {
    fn combine(self, other: Self) -> Self {
        Self { motion_until: self.motion_until.into_iter().chain(other.motion_until).reduce(f64::max), blinks: self.blinks || other.blinks }
    }

    fn motion_at(self, time: f64) -> bool {
        self.motion_until.is_some_and(|rest| time < rest)
    }

    fn timed_at(self, time: f64) -> bool {
        self.blinks || self.motion_at(time)
    }
}

/// A persistent, ordered view of a node's immediate children. Absent children keep their slots.
#[derive(Clone, Debug, Default)]
pub(super) struct ChildList {
    index: Option<Arc<FxHashMap<NodeId, (usize, i32)>>>,
    root: Option<Arc<ChildSpan>>,
}

#[derive(Debug)]
struct ChildSpan {
    len: usize,
    content: SpanContent,
    resting: Arc<[Part]>,
    coverage: Option<Rect>,
    timing: Timing,
}

#[derive(Debug)]
enum SpanContent {
    Leaf(Option<Arc<PaintNode>>),
    Branch(Arc<ChildSpan>, Arc<ChildSpan>),
}

impl ChildList {
    pub(super) fn new(children: impl ExactSizeIterator<Item = (NodeId, i32, Option<Arc<PaintNode>>)>, rebuilt: &mut u64) -> Self {
        let len = children.len();
        if len == 0 {
            return Self::default();
        }
        let mut index = FxHashMap::default();
        index.reserve(len);
        let mut children = children.enumerate().map(|(ordinal, (id, z_index, node))| {
            index.insert(id, (ordinal, z_index));
            node
        });
        let root = ChildSpan::build(len, &mut children, rebuilt);
        Self { index: Some(Arc::new(index)), root: Some(root) }
    }

    pub(super) fn has_z_index(&self, node: NodeId, z_index: i32) -> bool {
        self.index.as_ref().and_then(|index| index.get(&node)).is_some_and(|&(_, previous)| previous == z_index)
    }

    pub(super) fn replace(&mut self, node: NodeId, child: Option<Arc<PaintNode>>, rebuilt: &mut u64) {
        if let Some(&(index, _)) = self.index.as_ref().and_then(|index| index.get(&node)) {
            let root = self.root.as_mut().expect("indexed children");
            *root = root.replace(index, child, rebuilt);
        }
    }
}

impl ChildSpan {
    fn build(len: usize, children: &mut impl Iterator<Item = Option<Arc<PaintNode>>>, rebuilt: &mut u64) -> Arc<Self> {
        if len == 1 {
            Self::leaf(children.next().expect("remaining child"), rebuilt)
        } else {
            let left = Self::build(len / 2, children, rebuilt);
            let right = Self::build(len - len / 2, children, rebuilt);
            Self::branch(left, right, rebuilt)
        }
    }

    fn leaf(node: Option<Arc<PaintNode>>, rebuilt: &mut u64) -> Arc<Self> {
        *rebuilt = rebuilt.saturating_add(1);
        let (timing, resting, coverage) = node.as_ref().map_or_else(
            || (Timing::default(), Arc::from([]), Some(Rect::default())),
            |node| (node.timing, node.resting.clone(), node.coverage),
        );
        Arc::new(Self { len: 1, content: SpanContent::Leaf(node), resting, coverage, timing })
    }

    fn branch(left: Arc<Self>, right: Arc<Self>, rebuilt: &mut u64) -> Arc<Self> {
        *rebuilt = rebuilt.saturating_add(1);
        let resting = match (left.resting.is_empty(), right.resting.is_empty()) {
            (true, _) => right.resting.clone(),
            (_, true) => left.resting.clone(),
            (false, false) => Arc::from([group(&left.resting, left.coverage), group(&right.resting, right.coverage)]),
        };
        Arc::new(Self {
            len: left.len + right.len,
            timing: left.timing.combine(right.timing),
            coverage: left.coverage.zip(right.coverage).map(|(left, right)| union(left, right)),
            content: SpanContent::Branch(left, right),
            resting,
        })
    }

    fn replace(self: &Arc<Self>, index: usize, node: Option<Arc<PaintNode>>, rebuilt: &mut u64) -> Arc<Self> {
        match &self.content {
            SpanContent::Leaf(previous) => {
                let same = previous.as_ref().zip(node.as_ref()).map_or(previous.is_none() && node.is_none(), |(a, b)| Arc::ptr_eq(a, b));
                if same { self.clone() } else { Self::leaf(node, rebuilt) }
            }
            SpanContent::Branch(left, right) if index < left.len => {
                let changed = left.replace(index, node, rebuilt);
                if Arc::ptr_eq(left, &changed) { self.clone() } else { Self::branch(changed, right.clone(), rebuilt) }
            }
            SpanContent::Branch(left, right) => {
                let changed = right.replace(index - left.len, node, rebuilt);
                if Arc::ptr_eq(right, &changed) { self.clone() } else { Self::branch(left.clone(), changed, rebuilt) }
            }
        }
    }

    fn emit(&self, sample: &mut Sample, output: &mut Vec<Part>) {
        sample.work.spans += 1;
        if !self.timing.timed_at(sample.time) {
            append_covered(&self.resting, self.coverage, output);
            return;
        }
        match &self.content {
            SpanContent::Leaf(node) => {
                if let Some(node) = node {
                    node.emit(sample, output);
                }
            }
            SpanContent::Branch(left, right) => {
                left.emit(sample, output);
                right.emit(sample, output);
            }
        }
    }

    fn emit_for_exit(&self, time: f64, independent: &FxHashSet<NodeId>, output: &mut Vec<Part>) {
        match &self.content {
            SpanContent::Leaf(Some(node)) => {
                if let Some(element) = node.element.as_ref().filter(|element| !independent.contains(&element.source)) {
                    element.emit(
                        time,
                        |output| {
                            if let Some(children) = &node.children.root {
                                children.emit_for_exit(time, independent, output);
                            }
                        },
                        output,
                    );
                }
            }
            SpanContent::Leaf(None) => {}
            SpanContent::Branch(left, right) => {
                left.emit_for_exit(time, independent, output);
                right.emit_for_exit(time, independent, output);
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct PaintNode {
    pub element: Option<Arc<Element>>,
    pub translates: bool,
    pub children: ChildList,
    ghost: Option<Arc<ExitGhost>>,
    resting: Arc<[Part]>,
    coverage: Option<Rect>,
    timing: Timing,
}

#[derive(Default)]
pub(super) struct SampleWork {
    pub nodes: u64,
    pub spans: u64,
}

struct Sample {
    time: f64,
    work: SampleWork,
}

impl PaintNode {
    pub(super) fn element(element: Arc<Element>, children: ChildList, translates: bool) -> Self {
        let mut timing = Timing {
            motion_until: element
                .motion
                .as_ref()
                .filter(|_| !translates)
                .and_then(|track| track.rest_at())
                .into_iter()
                .chain(element.reveal.and_then(TextRevealTrack::rest_at))
                .reduce(f64::max),
            blinks: element.caret.is_some(),
        };
        let mut resting = Vec::new();
        element.emit(
            f64::INFINITY,
            |output| {
                if let Some(children) = &children.root {
                    append_covered(&children.resting, children.coverage, output);
                }
            },
            &mut resting,
        );
        // A transparent target can still have visible children while its opacity animates.
        if let Some(children) = &children.root {
            timing = timing.combine(children.timing);
        }
        let child_coverage = children.root.as_ref().map_or(Some(Rect::default()), |children| children.coverage);
        let coverage = if translates {
            resting = vec![Part::MotionContent {
                source: element.source,
                spatial_id: ScrollPlacements::spatial_id(element.source),
                parts: resting.into(),
            }];
            None
        } else {
            element.coverage(child_coverage)
        };
        Self { element: Some(element), translates, ghost: None, children, resting: resting.into(), coverage, timing }
    }

    pub(super) fn ghost(ghost: Option<Arc<ExitGhost>>, children: ChildList) -> Self {
        let mut timing = Timing { motion_until: ghost.as_ref().and_then(|ghost| ghost.opacity.rest_at(0.0)), blinks: false };
        let mut resting = Vec::new();
        if let Some(children) = &children.root {
            timing = timing.combine(children.timing);
            append_covered(&children.resting, children.coverage, &mut resting);
        }
        let coverage = children.root.as_ref().map_or(Some(Rect::default()), |children| children.coverage);
        Self { element: None, translates: false, ghost, children, resting: resting.into(), coverage, timing }
    }

    fn motion_at(&self, time: f64) -> bool {
        self.timing.motion_at(time)
    }

    fn emit(&self, sample: &mut Sample, output: &mut Vec<Part>) {
        if !self.timing.timed_at(sample.time) {
            append_covered(&self.resting, self.coverage, output);
            return;
        }
        sample.work.nodes += 1;
        if let Some(ghost) = &self.ghost {
            ghost.emit(sample.time, output);
        }
        if let Some(element) = &self.element {
            element.emit(
                sample.time,
                |output| {
                    if let Some(children) = &self.children.root {
                        children.emit(sample, output);
                    }
                },
                output,
            );
        } else if let Some(children) = &self.children.root {
            children.emit(sample, output);
        }
    }
}

impl Element {
    pub(super) fn emit(&self, time: f64, children: impl FnOnce(&mut Vec<Part>), output: &mut Vec<Part>) {
        let (visual, bounds) = self.motion.as_ref().map_or((self.visual, self.bounds), |track| (track.visual(time), track.bounds(time)));
        let alpha = visual.opacity.clamp(0.0, 1.0);
        if alpha == 0.0 {
            return;
        }
        let blend = alpha < 1.0 || self.transition.is_some();
        if let Some(transition) = &self.transition {
            let mut transition = transition.clone();
            transition.opacity = alpha;
            transition.progress = self.motion.as_ref().map_or(1.0, |motion| motion.transition_progress(time));
            if transition.progress == 1.0 {
                transition.run = transition.mount.clone();
            }
            output.push(Part::Item(Item::PushTransition(transition)));
        } else if blend {
            output.push(Part::Item(Item::PushLayer { source: self.source, opacity: alpha }));
        }

        let offset = Vec2::new(bounds.x, bounds.y);
        if !self.own.is_empty() || self.caret.is_some() || visual.ring.is_some() || visual.background.is_some() {
            let mut own = Vec::new();
            let items = if visual != self.visual || bounds.w != self.bounds.w || bounds.h != self.bounds.h {
                let mut items = self.decoration.items(Rect::new(0.0, 0.0, bounds.w, bounds.h), visual);
                items.extend(self.own[self.content_start..].iter().cloned().map(|mut item| {
                    match &mut item {
                        Item::Glyphs(glyphs) if glyphs.color == self.visual.text => glyphs.color = visual.text,
                        Item::Path(path) if path.color == self.visual.text => path.color = visual.text,
                        _ => {}
                    }
                    item
                }));
                items.into()
            } else {
                self.own.clone()
            };
            let items = if let Some(reveal) = self.reveal {
                let mut sampled = None;
                for (index, item) in items.iter().enumerate() {
                    let Item::Glyphs(run) = item else { continue };
                    if run.glyphs.iter().all(|glyph| reveal.opacity(glyph.source_end, time) == 1.0) {
                        continue;
                    }
                    let colors = run.glyphs.iter().map(|glyph| reveal.color(glyph.source_end, time, run.color, self.reveal_tint)).collect();
                    let mut run = run.clone();
                    run.colors = Some(colors);
                    sampled.get_or_insert_with(|| items.to_vec())[index] = Item::Glyphs(run);
                }
                sampled.map_or(items, Into::into)
            } else {
                items
            };
            if !items.is_empty() {
                own.push(Part::Drawing { offset, fade: 1.0, items });
            }
            if let Some((caret, start)) = &self.caret
                && time.is_finite()
                && (time - start).max(0.0) % 1.0 < 0.5
            {
                let mut caret = caret.clone();
                caret.translate(bounds.x - self.bounds.x, bounds.y - self.bounds.y);
                own.push(Part::Item(caret));
            }
            if !own.is_empty() {
                output.push(Part::CulledGroup { bounds: self.own_coverage(bounds, visual), parts: own.into() });
            }
        }

        if let Some(clip) = self.sampled_clip(bounds, visual) {
            output.push(Part::Item(clip));
        }
        if self.scroll {
            let mut content = Vec::new();
            children(&mut content);
            output.push(Part::ScrollContent {
                source: self.source,
                spatial_id: ScrollPlacements::spatial_id(self.source),
                scrollbar_offset: Vec2::new(bounds.x - self.bounds.x, bounds.y - self.bounds.y),
                parts: content.into(),
            });
        } else {
            children(output);
        }
        if self.child_clip.is_some() {
            output.push(Part::Item(Item::PopClip));
        }
        if blend {
            output.push(Part::Item(Item::PopLayer));
        }
    }
    fn own_coverage(&self, bounds: Rect, visual: Visual) -> Rect {
        let coverage = visual.ring.and_then(|ring| ring_rect(bounds, ring)).map_or(bounds, |ring| union(bounds, ring));
        self.decoration.shadow_bounds(bounds).fold(coverage, union)
    }

    fn coverage(&self, children: Option<Rect>) -> Option<Rect> {
        if self.transition.is_some() {
            return None;
        }
        let mut children = children?;
        if let Some((clip, axes)) = self.child_clip {
            if self.scroll {
                let [x, y] = axes;
                children = union(
                    clip,
                    Rect::new(
                        if x { clip.x } else { children.x },
                        if y { clip.y } else { children.y },
                        if x { clip.w } else { children.w },
                        if y { clip.h } else { children.h },
                    ),
                );
            } else {
                children = restrict(children, clip, axes);
            }
        } else if self.scroll {
            return None;
        }
        let own = self.own_coverage(self.bounds, self.visual);
        Some(union(own, children))
    }

    pub(super) fn clip_at(&self, time: f64) -> Option<Item> {
        let (visual, bounds) = self.motion.as_ref().map_or((self.visual, self.bounds), |track| (track.visual(time), track.bounds(time)));
        self.sampled_clip(bounds, visual)
    }

    fn sampled_clip(&self, bounds: Rect, visual: Visual) -> Option<Item> {
        let (mut clip, axes) = self.child_clip?;
        clip.x += bounds.x - self.bounds.x;
        clip.y += bounds.y - self.bounds.y;
        Item::clip(self.source, clip, axes, radius(visual, clip))
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    if a.is_empty() {
        return b;
    }
    if b.is_empty() {
        return a;
    }
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    Rect::new(x, y, a.right().max(b.right()) - x, a.bottom().max(b.bottom()) - y)
}

fn restrict(bounds: Rect, clip: Rect, [x, y]: [bool; 2]) -> Rect {
    bounds.intersect(&Rect::new(
        if x { clip.x } else { bounds.x },
        if y { clip.y } else { bounds.y },
        if x { clip.w } else { bounds.w },
        if y { clip.h } else { bounds.h },
    ))
}

fn group(parts: &Arc<[Part]>, coverage: Option<Rect>) -> Part {
    coverage.map_or_else(|| Part::Group(parts.clone()), |bounds| Part::CulledGroup { bounds, parts: parts.clone() })
}

fn append_group(parts: &Arc<[Part]>, output: &mut Vec<Part>) {
    append_covered(parts, None, output);
}

fn append_covered(parts: &Arc<[Part]>, coverage: Option<Rect>, output: &mut Vec<Part>) {
    if !parts.is_empty() {
        output.push(group(parts, coverage));
    }
}

fn mark_exiting(part: &mut Part) {
    let mark_item = |item: &mut Item| match item {
        Item::Custom(paint) => paint.exiting = true,
        Item::Native { cut, .. } => *cut = false,
        _ => {}
    };
    match part {
        Part::Item(item) => mark_item(item),
        Part::Drawing { items, .. } => Arc::make_mut(items).iter_mut().for_each(mark_item),
        Part::Group(parts)
        | Part::CulledGroup { parts, .. }
        | Part::TranslatedGroup { parts, .. }
        | Part::ScrollContent { parts, .. }
        | Part::MotionContent { parts, .. }
        | Part::ScrollFrame { parts, .. } => Arc::make_mut(parts).iter_mut().for_each(mark_exiting),
    }
}
