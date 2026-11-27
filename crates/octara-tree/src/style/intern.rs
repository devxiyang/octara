use std::hash::Hash;
use std::mem::discriminant;
use std::sync::Arc;

use octara_model::motion::Curve;
use octara_model::style::{GridPos, LayoutLength, LayoutStyle, Shadows, Visual};
use octara_model::theme::Color;
use rustc_hash::{FxHashMap, FxHasher};
use smallvec::SmallVec;

/// The shareable part of a style. Authored values and inherited token identities belong to the node.
#[derive(Debug, PartialEq)]
pub struct ResolvedStyle {
    pub layout: LayoutStyle,
    pub visual: Visual,
}

impl ResolvedStyle {
    fn fingerprint(&self) -> u64 {
        use std::hash::Hasher;

        let mut hash = FxHasher::default();
        let l = &self.layout;
        discriminant(&l.display).hash(&mut hash);
        discriminant(&l.position).hash(&mut hash);
        discriminant(&l.align_items).hash(&mut hash);
        l.align_self.as_ref().map(discriminant).hash(&mut hash);
        discriminant(&l.justify_content).hash(&mut hash);
        discriminant(&l.flex_direction).hash(&mut hash);
        discriminant(&l.flex_wrap).hash(&mut hash);
        discriminant(&l.overflow_x).hash(&mut hash);
        discriminant(&l.overflow_y).hash(&mut hash);
        l.grid_columns.hash(&mut hash);
        l.grid_rows.hash(&mut hash);
        for position in [l.grid_column.start, l.grid_column.end, l.grid_row.start, l.grid_row.end] {
            discriminant(&position).hash(&mut hash);
            match position {
                GridPos::Line(line) => line.hash(&mut hash),
                GridPos::Span(span) => span.hash(&mut hash),
                GridPos::Auto => {}
            }
        }
        l.aspect_ratio.map(float_bits).hash(&mut hash);
        for value in [l.flex_grow, l.flex_shrink] {
            float_bits(value).hash(&mut hash);
        }
        for size in [l.size, l.min_size, l.max_size] {
            size.width.map(length_key).hash(&mut hash);
            size.height.map(length_key).hash(&mut hash);
        }
        for edges in [l.margin, l.inset] {
            for edge in [edges.top, edges.right, edges.bottom, edges.left] {
                edge.map(length_key).hash(&mut hash);
            }
        }
        for edges in [l.padding, l.border] {
            for edge in [edges.top, edges.right, edges.bottom, edges.left] {
                length_key(edge).hash(&mut hash);
            }
        }
        length_key(l.gap.width).hash(&mut hash);
        length_key(l.gap.height).hash(&mut hash);
        l.flex_basis.map(length_key).hash(&mut hash);

        let v = &self.visual;
        v.background.map(color_key).hash(&mut hash);
        color_key(v.text).hash(&mut hash);
        color_key(v.border).hash(&mut hash);
        v.ring.map(|ring| (float_bits(ring.width), float_bits(ring.offset), color_key(ring.color))).hash(&mut hash);
        v.radius.map(float_bits).hash(&mut hash);
        v.z_index.hash(&mut hash);
        v.direction.hash(&mut hash);
        for value in [v.font_size, v.line_height, v.letter_spacing, v.opacity, v.text_scale] {
            float_bits(value).hash(&mut hash);
        }
        v.font_weight.hash(&mut hash);
        v.font_family.hash(&mut hash);
        (v.italic, v.underline, v.strike, v.tabular_nums, v.max_lines, v.pointer_events).hash(&mut hash);
        discriminant(&v.wrap).hash(&mut hash);
        discriminant(&v.align).hash(&mut hash);
        discriminant(&v.scrollbar).hash(&mut hash);
        v.user_select.as_ref().map(discriminant).hash(&mut hash);
        v.cursor.as_ref().map(discriminant).hash(&mut hash);
        v.gradient
            .map(|g| (float_bits(g.angle), color_key(g.from.0), float_bits(g.from.1), color_key(g.to.0), float_bits(g.to.1)))
            .hash(&mut hash);
        discriminant(&v.shadow).hash(&mut hash);
        match v.shadow {
            Shadows::None => {}
            Shadows::Token(token) => token.hash(&mut hash),
            Shadows::Custom(shadows) => {
                for shadow in shadows {
                    shadow
                        .map(|s| (color_key(s.color), [s.offset.0, s.offset.1, s.blur_radius, s.spread_radius].map(float_bits)))
                        .hash(&mut hash);
                }
            }
        }
        for curve in [v.motion.layout, v.motion.color, v.motion.opacity] {
            discriminant(&curve).hash(&mut hash);
            match curve {
                Curve::Spring(spring) => [spring.duration, spring.bounce].map(float_bits).hash(&mut hash),
                Curve::Fade(duration) => float_bits(duration).hash(&mut hash),
                Curve::Instant => {}
            }
        }
        hash.finish()
    }
}

fn float_bits(value: f32) -> u32 {
    if value == 0.0 { 0 } else { value.to_bits() }
}

fn color_key(value: Color) -> [u32; 4] {
    value.components.map(float_bits)
}

fn length_key(value: LayoutLength) -> (bool, u32) {
    match value {
        LayoutLength::Px(px) => (false, float_bits(px)),
        LayoutLength::Pct(fraction) => (true, float_bits(fraction)),
    }
}

#[derive(Default)]
pub(crate) struct StyleInterner {
    values: FxHashMap<u64, SmallVec<[Arc<ResolvedStyle>; 1]>>,
}

impl StyleInterner {
    pub(crate) fn intern(&mut self, style: ResolvedStyle) -> Arc<ResolvedStyle> {
        let bucket = self.values.entry(style.fingerprint()).or_default();
        if let Some(existing) = bucket.iter().find(|existing| existing.as_ref() == &style) {
            return Arc::clone(existing);
        }
        let style = Arc::new(style);
        bucket.push(Arc::clone(&style));
        style
    }

    /// Discards the table's reference once the last style slot releases a value.
    pub(crate) fn release(&mut self, style: Arc<ResolvedStyle>) {
        // The style slot's Arc and the interner's Arc are the only two remaining owners.
        if Arc::strong_count(&style) != 2 {
            return;
        }
        let key = style.fingerprint();
        if let Some(bucket) = self.values.get_mut(&key) {
            bucket.retain(|candidate| !Arc::ptr_eq(candidate, &style));
            if bucket.is_empty() {
                self.values.remove(&key);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.values.values().map(|bucket| bucket.len()).sum()
    }
}
