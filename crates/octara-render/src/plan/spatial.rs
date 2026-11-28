//! Identifies stable geometry and computes the scroll and motion offsets for each submission.

use octara_model::geometry::{Size, Vec2};
use octara_model::identity::NodeId;
use octara_model::paint::{DisplayList, Item, Part, ScrollPlacements};
use octara_model::style::Mat4;
use octara_model::theme::Color;
use rustc_hash::FxHashMap;
use std::sync::Arc;

/// A changed translation row in an otherwise retained instance buffer.
pub struct SpatialWrite {
    pub row: usize,
    pub offset: [f32; 4],
}

/// Dense translation rows for retained 2D scenes and indexed projective layer matrices.
#[derive(Default)]
pub struct SpatialTable {
    retained: Option<RetainedTranslations>,
    offsets: Vec<[f32; 4]>,
    projections: Vec<Mat4>,
}

struct RetainedTranslations {
    geometry: Arc<[Part]>,
    scrolls: ScrollPlacements,
    size: Size,
    background: Color,
    scale: f32,
    ports: Vec<ScrollPort>,
    slots: FxHashMap<u32, u32>,
}

struct ScrollPort {
    source: Option<NodeId>,
    translation: Vec2,
    parent: usize,
    row: usize,
    bars: [bool; 2],
    scrollbar_offset: Option<Vec2>,
    needs_integer_x: bool,
}

fn scroll_frame(list: &DisplayList) -> Option<(&ScrollPlacements, &Arc<[Part]>)> {
    let [Part::ScrollFrame { scrolls, parts }] = list.parts.as_slice() else { return None };
    Some((scrolls, parts))
}

impl SpatialTable {
    /// Builds the table for a retained scroll scene when its geometry supports translation.
    pub fn new(list: &DisplayList, scale: f32) -> Option<Self> {
        let (scrolls, geometry) = scroll_frame(list)?;
        if scrolls.is_empty() || !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let mut ports: Vec<ScrollPort> = Vec::new();
        let mut slots = FxHashMap::from_iter([(0, 0)]);
        let mut groups = vec![(geometry.iter(), 0)];
        while let Some((parts, parent)) = groups.last_mut() {
            match parts.next() {
                Some(
                    part @ (Part::ScrollContent { source, spatial_id, parts, .. } | Part::MotionContent { source, spatial_id, parts }),
                ) => {
                    let scrollbar_offset = match part {
                        Part::ScrollContent { scrollbar_offset, .. } => Some(*scrollbar_offset),
                        _ => None,
                    };
                    let row = slots.len();
                    if slots.insert(*spatial_id, u32::try_from(row).ok()?).is_some() {
                        return None;
                    }
                    let mut bars = [false; 2];
                    if scrollbar_offset.is_some()
                        && let Some(placement) = scrolls.get(*source)
                    {
                        for (index, bar) in placement.scrollbars.iter().enumerate() {
                            let Some(bar) = bar else { continue };
                            // Thumbs translate independently and may land on fractional pixels.
                            if !matches!(&*bar.parts, [Part::Item(Item::Quad(_))]) {
                                return None;
                            }
                            let slot = u32::try_from(slots.len()).ok()?;
                            if slots.insert(ScrollPlacements::scrollbar_spatial_id(*source, index), slot).is_some() {
                                return None;
                            }
                            bars[index] = true;
                        }
                    }
                    ports.push(ScrollPort {
                        source: Some(*source),
                        translation: Vec2::default(),
                        parent: *parent,
                        row,
                        bars,
                        scrollbar_offset,
                        needs_integer_x: false,
                    });
                    groups.push((parts.iter(), ports.len()));
                }
                Some(Part::TranslatedGroup { spatial_id, offset, parts }) => {
                    let row = slots.len();
                    if slots.insert(*spatial_id, u32::try_from(row).ok()?).is_some() {
                        return None;
                    }
                    ports.push(ScrollPort {
                        source: None,
                        translation: *offset,
                        parent: *parent,
                        row,
                        bars: [false; 2],
                        scrollbar_offset: None,
                        needs_integer_x: false,
                    });
                    groups.push((parts.iter(), ports.len()));
                }
                Some(Part::Group(parts) | Part::CulledGroup { parts, .. }) => {
                    let parent = *parent;
                    groups.push((parts.iter(), parent));
                }
                Some(Part::ScrollFrame { .. }) => return None,
                Some(Part::Drawing { items, .. }) if *parent != 0 => {
                    let port = &mut ports[*parent - 1];
                    port.needs_integer_x |= items.iter().any(needs_integer_x);
                }
                Some(Part::Item(item)) if *parent != 0 => ports[*parent - 1].needs_integer_x |= needs_integer_x(item),
                Some(Part::Drawing { .. } | Part::Item(_)) => {}
                None => {
                    groups.pop();
                }
            }
        }
        if ports.is_empty() {
            return None;
        }
        let offsets = spatial_offsets(&ports, slots.len(), scale, scrolls, None)?;
        let retained = RetainedTranslations {
            geometry: geometry.clone(),
            scrolls: scrolls.clone(),
            size: list.size,
            background: list.background,
            scale,
            ports,
            slots,
        };
        Some(Self { retained: Some(retained), offsets, projections: Vec::new() })
    }

    pub(crate) fn row(&self, id: u32) -> Option<u32> {
        self.retained.as_ref()?.slots.get(&id).copied()
    }

    pub(crate) fn push_projection(&mut self, matrix: Mat4) -> usize {
        let index = self.projections.len();
        self.projections.push(matrix);
        index
    }

    /// Matrix for a projected pass. Ordinary translations do not occupy this table.
    pub fn projection(&self, index: usize) -> &Mat4 {
        &self.projections[index]
    }

    /// Current shader rows in device pixels. Row zero holds the identity offset.
    pub fn offsets(&self) -> &[[f32; 4]] {
        &self.offsets
    }

    /// Advances a compatible scene and returns only the rows requiring upload.
    /// Returns `None` on geometry or raster phase changes that require replanning.
    pub fn update(&mut self, list: &DisplayList, scale: f32) -> Option<Vec<SpatialWrite>> {
        let (scrolls, geometry) = scroll_frame(list)?;
        let retained = self.retained.as_mut()?;
        if !Arc::ptr_eq(geometry, &retained.geometry)
            || retained.size != list.size
            || retained.background != list.background
            || retained.scale != scale
        {
            return None;
        }
        if scrolls.same_snapshot(&retained.scrolls) {
            return Some(Vec::new());
        }
        let offsets = spatial_offsets(&retained.ports, self.offsets.len(), retained.scale, scrolls, Some(&retained.scrolls))?;
        let writes = offsets
            .iter()
            .zip(&self.offsets)
            .enumerate()
            .filter_map(|(row, (&offset, &old))| (offset != old).then_some(SpatialWrite { row, offset }))
            .collect();
        retained.scrolls = scrolls.clone();
        self.offsets = offsets;
        Some(writes)
    }
}

fn needs_integer_x(item: &Item) -> bool {
    matches!(item, Item::Glyphs(_) | Item::Path(_))
}

fn spatial_offsets(
    ports: &[ScrollPort],
    count: usize,
    scale: f32,
    scrolls: &ScrollPlacements,
    previous: Option<&ScrollPlacements>,
) -> Option<Vec<[f32; 4]>> {
    let mut offsets = Vec::with_capacity(count);
    let mut cumulative = Vec::with_capacity(ports.len() + 1);
    offsets.push([0.0, 0.0, 1.0, 0.0]);
    cumulative.push((Vec2::default(), Vec2::default()));
    for port in ports {
        let placement = port.source.and_then(|source| scrolls.get(source));
        let prior = port.source.and_then(|source| previous.and_then(|scrolls| scrolls.get(source)));
        let (parent, inherited) = cumulative[port.parent];
        let local = placement.map_or_else(Vec2::default, |placement| placement.offset);
        let world = Vec2::new(parent.x + local.x, parent.y + local.y);
        let extra = Vec2::new(inherited.x + port.translation.x, inherited.y + port.translation.y);
        let x = (world.x + extra.x) * scale;
        let y = (world.y * scale).round() + extra.y * scale;
        // Fractional translation can change the glyph raster phase and path coverage.
        if !x.is_finite() || !y.is_finite() || (port.needs_integer_x && (x.fract() != 0.0 || y.fract() != 0.0)) {
            return None;
        }
        cumulative.push((world, extra));
        debug_assert_eq!(port.row, offsets.len());
        offsets.push([x, y, 1.0, 0.0]);
        let Some(scrollbar_offset) = port.scrollbar_offset else { continue };
        for (index, was_present) in port.bars.into_iter().enumerate() {
            let bar = placement.and_then(|placement| placement.scrollbars[index].as_ref());
            if bar.is_some() != was_present {
                return None;
            }
            let Some(bar) = bar else { continue };
            if previous.is_some() {
                let old = prior?.scrollbars[index].as_ref()?;
                if !Arc::ptr_eq(&bar.parts, &old.parts) {
                    return None;
                }
            }
            let x = (parent.x + (inherited.x + (scrollbar_offset.x + bar.offset.x))) * scale;
            let y = (parent.y * scale).round() + (inherited.y + (scrollbar_offset.y + bar.offset.y)) * scale;
            if !x.is_finite() || !y.is_finite() {
                return None;
            }
            offsets.push([x, y, 1.0, 0.0]);
        }
    }
    Some(offsets)
}
