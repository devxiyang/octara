//! Damage from shared display-list spans and changes to their scroll placements.

use super::batch::grow;
use super::{PassBounds, union};
use crate::glyphs::{GlyphCache, GlyphKey, SUBPIXEL_STEPS};
use crate::paths;
use octara_model::geometry::{Rect, Vec2};
use octara_model::identity::NodeId;
use octara_model::paint::{DisplayList, Item, Part, ScrollPlacement, ScrollPlacements};
use std::sync::Arc;

/// The damaged pixel bounds clipped to the output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DamageRect(PassBounds);

impl DamageRect {
    pub fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    fn pixels(self) -> u64 {
        u64::from(self.0.size[0]) * u64::from(self.0.size[1])
    }

    pub fn is_partial(self, [width, height]: [u32; 2]) -> bool {
        5 * self.pixels() < 3 * u64::from(width) * u64::from(height)
    }

    pub fn area_ratio(self, [width, height]: [u32; 2]) -> f64 {
        self.pixels() as f64 / (f64::from(width) * f64::from(height))
    }

    /// Pixel bounds as `[x, y, width, height]`.
    pub fn pixels_rect(self) -> [u32; 4] {
        let PassBounds { origin: [x, y], size: [w, h] } = self.0;
        [x as u32, y as u32, w, h]
    }

    pub fn rect(self) -> Rect {
        super::batch::rect(self.0.rect())
    }
}

/// Retained display-list geometry used to compute damage against a previous frame.
#[derive(Clone)]
pub struct SceneSnapshot {
    list: DisplayList,
    scale: f32,
    extent: [u32; 2],
    ports: Option<Arc<[ScrollPort]>>,
}

impl SceneSnapshot {
    pub fn new(list: &DisplayList, scale: f32, extent: [u32; 2]) -> Self {
        Self { list: list.clone(), scale, extent, ports: None }
    }

    pub fn can_retain(&self) -> bool {
        let (parts, scrolls) = root(&self.list);
        balanced(parts, scrolls)
    }

    pub fn damage_since(&mut self, old: &Self, glyphs: &mut GlyphCache, spatial: bool) -> Option<DamageRect> {
        if self.extent != old.extent
            || self.scale != old.scale
            || self.list.size != old.list.size
            || self.list.background != old.list.background
            || !self.scale.is_finite()
            || self.scale <= 0.0
        {
            return None;
        }
        let window = Rect::new(0.0, 0.0, self.extent[0] as f32, self.extent[1] as f32);
        let (old_parts, old_scrolls) = root(&old.list);
        let (new_parts, new_scrolls) = root(&self.list);
        let shared = std::ptr::eq(old_parts, new_parts);
        if shared {
            self.ports = old.ports.clone();
        }
        let viewport = Rect::new(0.0, 0.0, self.list.size.width * self.scale, self.list.size.height * self.scale);
        let mut damage = SceneDamage { window, viewport, scale: self.scale, glyphs, spatial, ink: Rect::default() };
        match (old_scrolls, new_scrolls) {
            (Some(a), Some(b)) if !a.same_snapshot(b) => {
                let ports = self.ports.get_or_insert_with(|| find_ports(new_parts, self.scale).into());
                add_scroll_damage(&mut damage, ports, a, b)?;
                if !shared {
                    // Old ports may have moved or disappeared with the geometry edit.
                    let old_ports = old.ports.clone().unwrap_or_else(|| find_ports(old_parts, self.scale).into());
                    add_scroll_damage(&mut damage, &old_ports, a, b)?;
                }
            }
            (Some(_), None) | (None, Some(_)) => return None,
            _ => {}
        }
        if !shared {
            let old = Placement { scrolls: old_scrolls, ..Placement::default() };
            let new = Placement { scrolls: new_scrolls, ..Placement::default() };
            damage.parts(old_parts, new_parts, old, new, window)?;
        }
        Some(DamageRect(PassBounds::from_rect(damage.ink.intersect(&window))))
    }
}

fn root(list: &DisplayList) -> (&[Part], Option<&ScrollPlacements>) {
    match list.parts.as_slice() {
        [Part::ScrollFrame { scrolls, parts }] => (parts, Some(scrolls)),
        parts => (parts, None),
    }
}

fn balanced(parts: &[Part], scrolls: Option<&ScrollPlacements>) -> bool {
    let mut depth = 0;
    for part in parts {
        match part {
            Part::Item(Item::PushClip { .. } | Item::PushAxisClip { .. }) => depth += 1,
            Part::Item(Item::PopClip) if depth > 0 => depth -= 1,
            Part::Item(item) if simple_draw(item) => {}
            Part::Drawing { items, .. } if items.iter().all(simple_draw) => {}
            Part::Group(parts) | Part::CulledGroup { parts, .. } | Part::TranslatedGroup { parts, .. } if balanced(parts, scrolls) => {}
            Part::ScrollContent { source, parts, .. }
                if scrolls.and_then(|scrolls| scrolls.get(*source)).is_none_or(simple_scrollbars) && balanced(parts, scrolls) => {}
            Part::MotionContent { parts, .. } if balanced(parts, scrolls) => {}
            _ => return false,
        }
    }
    depth == 0
}

fn simple_draw(item: &Item) -> bool {
    matches!(item, Item::Quad(_) | Item::Shadow(_) | Item::Image(_) | Item::Glyphs(_) | Item::Path(_) | Item::Native { cut: false, .. })
}

fn simple_scrollbars(placement: &ScrollPlacement) -> bool {
    placement.scrollbars.iter().flatten().all(|bar| matches!(&*bar.parts, [Part::Item(Item::Quad(_))]))
}

#[derive(Clone, Copy, Default)]
struct Placement<'a> {
    scrolls: Option<&'a ScrollPlacements>,
    local: Vec2,
    scroll: Vec2,
}

impl Placement<'_> {
    fn total(self, scale: f32) -> Vec2 {
        add(self.local, Vec2::new(self.scroll.x, (self.scroll.y * scale).round() / scale))
    }

    fn translated(self, offset: Vec2) -> Self {
        Self { local: add(self.local, offset), ..self }
    }

    fn scrolled(self, source: NodeId) -> Self {
        let scroll = self.scrolls.and_then(|scrolls| scrolls.get(source)).map_or(self.scroll, |p| add(self.scroll, p.offset));
        Self { scroll, ..self }
    }
}

struct SceneDamage<'a> {
    window: Rect,
    viewport: Rect,
    spatial: bool,
    scale: f32,
    glyphs: &'a mut GlyphCache,
    ink: Rect,
}

impl SceneDamage<'_> {
    fn add(&mut self, rect: Rect, clip: Rect) -> Option<()> {
        if ![rect.x, rect.y, rect.right(), rect.bottom()].into_iter().all(f32::is_finite) {
            return None;
        }
        if !rect.is_empty() && !clip.is_empty() {
            // Both shape coverage and antialiased clip edges can reach neighboring pixels.
            self.ink = union(self.ink, grow(rect, 1.0).intersect(&grow(clip, 1.0)));
        }
        Some(())
    }

    fn parts(&mut self, old: &[Part], new: &[Part], a: Placement<'_>, b: Placement<'_>, clip: Rect) -> Option<()> {
        if std::ptr::eq(old, new) && a.total(self.scale) == b.total(self.scale) {
            return Some(());
        }
        let mut clips = vec![clip];
        let count = old.len().max(new.len());
        for i in 0..count {
            let clip = *clips.last()?;
            let left = old.get(i);
            let right = new.get(i);
            match (left, right) {
                (Some(Part::Item(left)), Some(Part::Item(right))) if is_clip(left) || is_clip(right) => {
                    if left != right || a.total(self.scale) != b.total(self.scale) {
                        return None;
                    }
                    if matches!(left, Item::PopClip) {
                        clips.pop();
                    } else {
                        clips.push(clip.intersect(&clip_rect(left, a.total(self.scale), self.scale, self.window)?));
                    }
                }
                (Some(Part::Group(left)), Some(Part::Group(right))) => self.parts(left, right, a, b, clip)?,
                (
                    Some(Part::CulledGroup { bounds: left_bounds, parts: left }),
                    Some(Part::CulledGroup { bounds: right_bounds, parts: right }),
                ) => {
                    let left_parts = left;
                    let left = if self.visible(*left_bounds, a, clip) { left.as_ref() } else { &[] };
                    let right = if self.visible(*right_bounds, b, clip) {
                        right.as_ref()
                    } else {
                        if !Arc::ptr_eq(left_parts, right) && !balanced(right, b.scrolls) {
                            return None;
                        }
                        &[]
                    };
                    self.parts(left, right, a, b, clip)?;
                }
                (
                    Some(Part::TranslatedGroup { offset: left, parts: old, .. }),
                    Some(Part::TranslatedGroup { offset: right, parts: new, .. }),
                ) => self.parts(old, new, a.translated(*left), b.translated(*right), clip)?,
                (
                    Some(Part::ScrollContent { source, parts: old, .. }),
                    Some(Part::ScrollContent { source: next_source, parts: new, .. }),
                ) if source == next_source => {
                    let (x, y) = (a.scrolled(*source), b.scrolled(*source));
                    if x.total(self.scale) != y.total(self.scale) {
                        if !Arc::ptr_eq(old, new) && !balanced(new, b.scrolls) {
                            return None;
                        }
                        self.add(clip, clip)?;
                    } else {
                        self.parts(old, new, x, y, clip)?;
                    }
                    self.scrollbars(left?, right?, a, b, clip)?;
                }
                (
                    Some(Part::Drawing { offset: x, fade: old_fade, items: old }),
                    Some(Part::Drawing { offset: y, fade: new_fade, items: new }),
                ) => {
                    self.items(old, new, a.translated(*x), b.translated(*y), clip, old_fade != new_fade)?;
                }
                (
                    Some(Part::MotionContent { source, parts: old, .. }),
                    Some(Part::MotionContent { source: next_source, parts: new, .. }),
                ) if source == next_source => self.parts(old, new, a.scrolled(*source), b.scrolled(*source), clip)?,
                (Some(Part::Item(left)), Some(Part::Item(right))) => {
                    self.items(std::slice::from_ref(left), std::slice::from_ref(right), a, b, clip, false)?;
                }
                _ => {
                    for (part, placement) in [(left, a), (right, b)] {
                        if let Some(part) = part {
                            self.span(part, placement, clip)?;
                        }
                    }
                }
            }
        }
        (clips.len() == 1).then_some(())
    }

    fn visible(&self, bounds: Rect, placement: Placement<'_>, clip: Rect) -> bool {
        // Spatial planning draws the entire retained span. Its ink can reach a clip edge
        // even when the model's nominal culling bounds do not intersect the clip.
        self.spatial || !scaled(bounds, placement.total(self.scale), self.scale).intersect(&clip.intersect(&self.viewport)).is_empty()
    }

    fn scrollbars(&mut self, old: &Part, new: &Part, a: Placement<'_>, b: Placement<'_>, clip: Rect) -> Option<()> {
        let bars = |part: &Part, placement: Placement<'_>| {
            let Part::ScrollContent { source, scrollbar_offset, .. } = part else { unreachable!() };
            placement.scrolls.and_then(|scrolls| scrolls.get(*source)).map(|bar| (*scrollbar_offset, bar.scrollbars.clone()))
        };
        let (old, new) = (bars(old, a), bars(new, b));
        if old != new {
            self.add(clip, clip)?;
        }
        Some(())
    }

    fn span(&mut self, part: &Part, at: Placement<'_>, clip: Rect) -> Option<()> {
        match part {
            Part::Drawing { offset, items, .. } => self.items(items, &[], at.translated(*offset), at, clip, false),
            Part::Item(item) => self.item(item, at.total(self.scale), clip),
            Part::Group(parts) => self.parts(parts, &[], at, at, clip),
            Part::CulledGroup { bounds, parts } => {
                if self.visible(*bounds, at, clip) {
                    self.parts(parts, &[], at, at, clip)?;
                }
                Some(())
            }
            Part::TranslatedGroup { offset, parts, .. } => self.parts(parts, &[], at.translated(*offset), at, clip),
            Part::ScrollContent { .. } | Part::MotionContent { .. } | Part::ScrollFrame { .. } => None,
        }
    }

    fn items(&mut self, old: &[Item], new: &[Item], a: Placement<'_>, b: Placement<'_>, clip: Rect, faded: bool) -> Option<()> {
        let a = a.total(self.scale);
        let b = b.total(self.scale);
        if std::ptr::eq(old, new) && !faded && a == b {
            return Some(());
        }
        for i in 0..old.len().max(new.len()) {
            let (left, right) = (old.get(i), new.get(i));
            if faded || a != b || left != right {
                if let Some(item) = left {
                    self.item(item, a, clip)?;
                }
                if let Some(item) = right {
                    self.item(item, b, clip)?;
                }
            }
        }
        Some(())
    }

    fn item(&mut self, item: &Item, offset: Vec2, clip: Rect) -> Option<()> {
        let scale = self.scale;
        let bounds = match item {
            // An overlay native view paints outside the GPU surface. Its marker
            // contributes no pixels when swapped for the frozen image.
            Item::Native { cut: false, .. } => return Some(()),
            Item::Quad(q) => scaled(q.rect, offset, scale),
            Item::Shadow(s) => grow(scaled(s.rect, offset, scale), s.reach() * scale),
            Item::Image(i) => {
                // Both the image placement and its rounded element box have their own
                // antialiasing fringe. Intersect their coverage envelopes.
                grow(scaled(i.rect, offset, scale), 0.5).intersect(&grow(scaled(i.bounds, offset, scale), 0.5))
            }
            Item::Glyphs(run) => {
                let mut ink = Rect::default();
                for (glyph, _) in run.placed_colored_at(offset, 1.0) {
                    let x = (glyph.x * scale).floor();
                    let y = (glyph.y * scale).round();
                    let phase = (((glyph.x * scale - x) * SUBPIXEL_STEPS).floor() as u8).min(SUBPIXEL_STEPS as u8 - 1);
                    let key = GlyphKey::new(run, glyph.id, run.size * scale, phase);
                    if let Some(glyph) = self.glyphs.rasterize(&run.font, key) {
                        ink = union(ink, Rect::new(x + glyph.left as f32, y - glyph.top as f32, glyph.width as f32, glyph.height as f32));
                    }
                }
                ink
            }
            Item::Path(_) => {
                let Item::Path(path) = item.placed(offset, 1.0) else { unreachable!() };
                let visible = grow(clip, 1.0);
                paths::place(&path, scale, (visible.x, visible.y, visible.right(), visible.bottom()))
                    .map_or_else(Rect::default, |p| Rect::new(p.x as f32, p.y as f32, p.width as f32, p.height as f32))
            }
            _ => return None,
        };
        self.add(bounds, clip)
    }
}

fn is_clip(item: &Item) -> bool {
    matches!(item, Item::PushClip { .. } | Item::PushAxisClip { .. } | Item::PopClip)
}

fn clip_rect(item: &Item, offset: Vec2, scale: f32, window: Rect) -> Option<Rect> {
    let rect = match *item {
        Item::PushClip { rect, .. } => scaled(rect, offset, scale),
        Item::PushAxisClip { rect, horizontal: true, .. } => Rect::new((rect.x + offset.x) * scale, window.y, rect.w * scale, window.h),
        Item::PushAxisClip { rect, horizontal: false, .. } => Rect::new(window.x, (rect.y + offset.y) * scale, window.w, rect.h * scale),
        _ => return None,
    };
    [rect.x, rect.y, rect.right(), rect.bottom()].into_iter().all(f32::is_finite).then_some(rect)
}

fn add(a: Vec2, b: Vec2) -> Vec2 {
    Vec2::new(a.x + b.x, a.y + b.y)
}

fn scaled(rect: Rect, offset: Vec2, scale: f32) -> Rect {
    Rect::new((rect.x + offset.x) * scale, (rect.y + offset.y) * scale, rect.w * scale, rect.h * scale)
}

/// An indexed clip chain lets stable scroll frames find dirty viewports without walking rows.
struct ScrollPort {
    source: NodeId,
    parent: Option<usize>,
    clips: Vec<PortClip>,
    motion: bool,
    scrollbar_offset: Vec2,
}

#[derive(Clone, Copy)]
struct PortClip {
    rect: Rect,
    axes: [bool; 2],
    spatial: Option<usize>,
}

fn find_ports(parts: &[Part], scale: f32) -> Vec<ScrollPort> {
    fn visit(parts: &[Part], local: Vec2, parent: Option<usize>, scale: f32, clips: &mut Vec<PortClip>, ports: &mut Vec<ScrollPort>) {
        for part in parts {
            match part {
                Part::ScrollContent { source, parts, .. } | Part::MotionContent { source, parts, .. } => {
                    let index = ports.len();
                    let scrollbar_offset = match part {
                        Part::ScrollContent { scrollbar_offset, .. } => add(local, *scrollbar_offset),
                        _ => Vec2::default(),
                    };
                    ports.push(ScrollPort {
                        source: *source,
                        parent,
                        clips: clips.clone(),
                        motion: matches!(part, Part::MotionContent { .. }),
                        scrollbar_offset,
                    });
                    visit(parts, local, Some(index), scale, clips, ports);
                }
                Part::Group(parts) | Part::CulledGroup { parts, .. } => visit(parts, local, parent, scale, clips, ports),
                Part::TranslatedGroup { offset, parts, .. } => visit(parts, add(local, *offset), parent, scale, clips, ports),
                Part::Item(Item::PushClip { rect, .. }) => {
                    clips.push(PortClip { rect: scaled(*rect, local, scale), axes: [true, true], spatial: parent });
                }
                Part::Item(Item::PushAxisClip { rect, horizontal, .. }) => {
                    clips.push(PortClip { rect: scaled(*rect, local, scale), axes: [*horizontal, !*horizontal], spatial: parent });
                }
                Part::Item(Item::PopClip) => {
                    clips.pop();
                }
                _ => {}
            }
        }
    }
    let mut ports = Vec::new();
    visit(parts, Vec2::default(), None, scale, &mut Vec::new(), &mut ports);
    ports
}

fn add_scroll_damage(damage: &mut SceneDamage<'_>, ports: &[ScrollPort], old: &ScrollPlacements, new: &ScrollPlacements) -> Option<()> {
    let mut offsets = Vec::with_capacity(ports.len());
    for port in ports {
        let placements = [old.get(port.source), new.get(port.source)];
        let parents = port.parent.map_or([Vec2::default(); 2], |p| offsets[p]);
        let world = std::array::from_fn(|index| placements[index].map_or(parents[index], |p| add(parents[index], p.offset)));
        offsets.push(world);
        let scale = damage.scale;
        let px = |offset: Vec2| [offset.x * scale, (offset.y * scale).round()];
        let moved = px(world[0]) != px(world[1]);
        // A parent can cross a child's cumulative snapping threshold while the parent's
        // own rounded offset and the child's local placement both stay the same.
        if !moved && same_placement(placements) {
            continue;
        }
        if port.motion {
            if moved {
                return None;
            }
            continue;
        }
        if !placements.into_iter().flatten().all(simple_scrollbars) {
            return None;
        }
        for (i, parent) in parents.into_iter().enumerate() {
            let clip = port.clips.iter().fold(damage.window, |clip, entry| {
                let [x, y] = px(entry.spatial.map_or(Vec2::default(), |p| offsets[p][i]));
                let [horizontal, vertical] = entry.axes;
                let rect = Rect::new(
                    if horizontal { entry.rect.x + x } else { clip.x },
                    if vertical { entry.rect.y + y } else { clip.y },
                    if horizontal { entry.rect.w } else { clip.w },
                    if vertical { entry.rect.h } else { clip.h },
                );
                clip.intersect(&rect)
            });
            if moved {
                damage.add(clip, damage.window)?;
            } else if let Some(placement) = placements[i] {
                let parent = Vec2::new(parent.x, (parent.y * damage.scale).round() / damage.scale);
                for bar in placement.scrollbars.iter().flatten() {
                    let [Part::Item(item)] = &*bar.parts else { unreachable!("checked scrollbar part") };
                    damage.item(item, add(add(parent, port.scrollbar_offset), bar.offset), clip)?;
                }
            }
        }
    }
    Some(())
}

fn same_placement([a, b]: [Option<&ScrollPlacement>; 2]) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => std::ptr::eq(a, b) || a == b,
        (None, None) => true,
        _ => false,
    }
}
