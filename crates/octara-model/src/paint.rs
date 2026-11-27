//! Frozen drawing values shared by hosts, renderers and capture.

#[path = "canvas.rs"]
mod canvas;
#[path = "icon.rs"]
mod icon;
#[path = "path.rs"]
mod path;

use crate::image;
pub use crate::image::{Image, ImageFrame, ObjectFit, Sample};
pub use canvas::{Canvas, CanvasOp, CanvasText};
pub use icon::{Icon, IconShape, NodeIcon};
pub use path::{FillRule, LineCap, LineJoin, Path, PathCmd, PathStyle, Stroke, Transform};

use crate::geometry::{Point, Rect, Size, Vec2};
use crate::identity::NodeId;
use crate::native::NativeSlot;
use crate::theme::{Color, Gradient};
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::borrow::Cow;
use std::slice;
use std::sync::{Arc, Weak};

/// A rounded rectangle with an optional fill and border (borders are drawn inside the rect).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Quad {
    pub source: NodeId,
    pub rect: Rect,
    /// Corner radii in top-left, top-right, bottom-right, bottom-left order.
    pub radius: [f32; 4],
    pub fill: Option<Color>,
    /// A gradient fill (instead of `fill`).
    pub gradient: Option<Gradient>,
    /// Border widths: top, right, bottom, left.
    pub border: [f32; 4],
    pub border_color: Color,
}

impl Quad {
    fn fade(&mut self, k: f32) {
        if let Some(c) = &mut self.fill {
            *c = c.multiply_alpha(k);
        }
        self.border_color = self.border_color.multiply_alpha(k);
        if let Some(g) = &mut self.gradient {
            g.from.0 = g.from.0.multiply_alpha(k);
            g.to.0 = g.to.0.multiply_alpha(k);
        }
    }
}

/// A shared handle to a renderer-specific paint program.
#[derive(Clone, Debug)]
pub struct PaintProgram(Arc<dyn Any + Send + Sync>);

impl PartialEq for PaintProgram {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl PaintProgram {
    /// Stores a program that a renderer can recognize by type.
    pub fn new(program: impl Any + Send + Sync) -> Self {
        Self(Arc::new(program))
    }

    /// Gets the renderer's program if its type matches.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }

    /// Tracks the program's identity and lifetime without keeping it alive.
    pub fn downgrade(&self) -> Weak<dyn Any + Send + Sync> {
        Arc::downgrade(&self.0)
    }
}

/// Per-element arguments frozen with a drawing, including an optional immutable data version.
#[derive(Clone, Debug, Default)]
pub struct PaintInput {
    pub params: [f32; 4],
    data: Option<Arc<dyn Any + Send + Sync>>,
}

impl PaintInput {
    /// Binds a version of application data. Reuse the same `Arc` while the data is unchanged,
    /// and supply a new `Arc` to publish a change. Saved scenes keep the data they sampled.
    pub fn with_data<T: Any + Send + Sync>(params: [f32; 4], data: Arc<T>) -> Self {
        Self { params, data: Some(data) }
    }

    /// Reads the typed data version, if the drawing supplied one of this type.
    pub fn data<T: Any>(&self) -> Option<&T> {
        self.data.as_deref()?.downcast_ref()
    }

    /// Tracks the data's identity without extending its lifetime in the renderer's output cache.
    #[doc(hidden)]
    pub fn downgrade_data(&self) -> Option<Weak<dyn Any + Send + Sync>> {
        self.data.as_ref().map(Arc::downgrade)
    }
}

impl From<[f32; 4]> for PaintInput {
    fn from(params: [f32; 4]) -> Self {
        Self { params, data: None }
    }
}

impl PartialEq for PaintInput {
    fn eq(&self, other: &Self) -> bool {
        self.params.map(f32::to_bits) == other.params.map(f32::to_bits)
            && match (&self.data, &other.data) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }
}

/// The lifetime of one mounted drawing, including snapshots that keep it alive after detach.
#[derive(Clone, Debug, Default)]
pub struct PaintMount(Arc<()>);

impl PartialEq for PaintMount {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl PaintMount {
    /// Lets a renderer retain resources without prolonging the mount's lifetime.
    pub fn downgrade(&self) -> Weak<()> {
        Arc::downgrade(&self.0)
    }
}

/// A paint program occupying the same drawing position and geometry as its fallback quad.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomPaint {
    /// The styled box to draw when the renderer cannot run the program.
    pub fallback: Quad,
    pub program: PaintProgram,
    pub mount: PaintMount,
    /// The drawing belongs to a detached subtree finishing its exit.
    pub exiting: bool,
    pub input: PaintInput,
    /// Opacity to apply to the program's output. The fallback colors already include it.
    pub opacity: f32,
}

/// An opt-in region whose contents can be composed with the last accepted image of the region.
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionPaint {
    pub source: NodeId,
    /// The stable region, shared by its successive content revisions.
    pub mount: PaintMount,
    /// The current transition, kept alive by published drawings and captures. A settled region uses its mount.
    pub run: PaintMount,
    /// The region's content revision. A newly mounted region starts at zero.
    pub revision: u64,
    pub program: PaintProgram,
    /// The window's sampled progress, with one denoting an ordinary, settled region.
    pub progress: f32,
    pub opacity: f32,
}

/// A blurred rounded rectangle under an element (a box shadow), already offset and spread.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Shadow {
    pub source: NodeId,
    pub rect: Rect,
    pub radius: [f32; 4],
    pub color: Color,
    /// The CSS blur radius: twice the Gaussian's standard deviation (0: a sharp edge).
    pub blur: f32,
}

impl Shadow {
    /// How far the blur reaches beyond `rect` (three standard deviations).
    pub fn reach(&self) -> f32 {
        1.5 * self.blur
    }
}

/// One glyph of a run: its id in the font and its pen position (x, baseline y).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Glyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    /// End of the original UTF-8 cluster, or zero for independent attachment glyphs.
    pub source_end: usize,
}

/// Identifies a font face in the producer and renderer's shared font registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontId(pub u64);

/// Immutable font bytes which may be embedded, owned or backed by a read-only file mapping.
#[derive(Clone)]
pub struct FontData(Arc<dyn AsRef<[u8]> + Send + Sync>);

impl FontData {
    /// The backing must expose the same font bytes throughout its lifetime.
    pub fn new(data: Arc<dyn AsRef<[u8]> + Send + Sync>) -> Self {
        Self(data)
    }

    pub fn into_shared(self) -> Arc<dyn AsRef<[u8]> + Send + Sync> {
        self.0
    }
}

impl From<Arc<[u8]>> for FontData {
    fn from(data: Arc<[u8]>) -> Self {
        Self::new(Arc::new(data))
    }
}

impl From<&'static [u8]> for FontData {
    fn from(data: &'static [u8]) -> Self {
        Self::new(Arc::new(data))
    }
}

impl AsRef<[u8]> for FontData {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref().as_ref()
    }
}

impl std::ops::Deref for FontData {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.as_ref()
    }
}

impl PartialEq for FontData {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.as_ref() == other.as_ref()
    }
}

/// Immutable bytes and face index shared between shaping and rasterization.
#[derive(Clone, PartialEq)]
pub struct FontFace {
    pub id: FontId,
    pub data: FontData,
    pub index: u32,
}

impl std::fmt::Debug for FontFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontFace").field("id", &self.id).field("index", &self.index).finish_non_exhaustive()
    }
}

/// Glyphs of one font at one size and color, positioned from `origin`.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphRun {
    pub source: NodeId,
    pub font: FontFace,
    pub size: f32,
    /// Normalized variation coordinates (variable fonts), as the shaper used them.
    pub coords: Vec<i16>,
    /// Synthetic bold for a family without that weight.
    pub embolden: bool,
    /// Synthetic italic skew in degrees, or zero when the face needs no skew.
    pub skew: f32,
    pub color: Color,
    /// Paint-time color overrides in glyph order.
    pub colors: Option<Arc<[Color]>>,
    /// Where the glyphs' positions start from (moving the run moves only this).
    pub origin: Point,
    pub glyphs: Arc<[Glyph]>,
}

impl GlyphRun {
    /// Each glyph with its position (`origin` added).
    pub fn placed(&self) -> impl Iterator<Item = Glyph> + '_ {
        self.placed_at(self.origin)
    }

    pub fn placed_colored(&self) -> impl Iterator<Item = (Glyph, Color)> + '_ {
        self.placed_colored_at(Vec2::default(), 1.0)
    }

    /// Positions the glyphs and applies the drawing's fade to the run's default color.
    pub fn placed_colored_at(&self, offset: Vec2, fade: f32) -> impl Iterator<Item = (Glyph, Color)> + '_ {
        let mut origin = self.origin;
        if offset != Vec2::default() {
            origin.x += offset.x;
            origin.y += offset.y;
        }
        let color = if fade < 1.0 { self.color.multiply_alpha(fade) } else { self.color };
        self.placed_at(origin)
            .enumerate()
            .map(move |(i, glyph)| (glyph, self.colors.as_ref().and_then(|colors| colors.get(i)).copied().unwrap_or(color)))
    }

    fn placed_at(&self, origin: Point) -> impl Iterator<Item = Glyph> + '_ {
        self.glyphs.iter().map(move |g| Glyph { x: g.x + origin.x, y: g.y + origin.y, ..*g })
    }
}

/// An image drawn into `rect` (where its fit put it), clipped to `bounds` rounded by `radius`
/// (its element's box).
#[derive(Clone, Debug, PartialEq)]
pub struct ImagePaint {
    pub source: NodeId,
    pub image: Sample,
    pub rect: Rect,
    pub bounds: Rect,
    pub radius: [f32; 4],
    pub opacity: f32,
}

/// A path filled or stroked with a color or a gradient. `transform` maps the path's
/// coordinates to the window's logical pixels and scales the stroke width.
#[derive(Clone, Debug, PartialEq)]
pub struct PathPaint {
    pub source: NodeId,
    pub path: Path,
    pub transform: Transform,
    pub style: PathStyle,
    pub color: Color,
    /// A gradient (instead of `color`), spanning `bounds`.
    pub gradient: Option<Gradient>,
    /// The box a gradient spans (the element's), in window coordinates.
    pub bounds: Rect,
}

/// One drawing step.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Shadow(Shadow),
    Quad(Quad),
    Custom(CustomPaint),
    Image(ImagePaint),
    Glyphs(GlyphRun),
    Path(PathPaint),
    /// A native view's position in paint order. Hosts use this item to cut out areas for
    /// underlay views and detect content that overlaps overlay views.
    Native {
        source: NodeId,
        rect: Rect,
        radius: [f32; 4],
        slot: NativeSlot,
        /// The platform shows an underlay view here: the renderer clears the rect (rounded by
        /// `radius`) to transparent, so the view behind the surface shows through.
        cut: bool,
    },
    /// Clip what follows to `rect` (rounded by `radius`) until the matching `PopClip`.
    PushClip {
        source: NodeId,
        rect: Rect,
        radius: [f32; 4],
    },
    PopClip,
    /// Draw into a transparent layer until the matching `PopLayer`, then composite
    /// the entire layer at `opacity`.
    PushLayer {
        source: NodeId,
        opacity: f32,
    },
    /// Draw this region through the transition compositor, ending at the matching `PopLayer`.
    PushTransition(TransitionPaint),
    /// This region paints normally. An accepted frame retires any previous image for it.
    SkipTransition {
        source: NodeId,
        mount: PaintMount,
        revision: u64,
    },
    PopLayer,
}

impl Item {
    /// Whether the item draws pixels.
    pub fn draws(&self) -> bool {
        matches!(self, Item::Shadow(_) | Item::Quad(_) | Item::Custom(_) | Item::Image(_) | Item::Glyphs(_) | Item::Path(_))
    }

    /// The retained node whose custom drawing or transition produced this item.
    pub fn effect_source(&self) -> Option<NodeId> {
        match self {
            Item::Custom(paint) => Some(paint.fallback.source),
            Item::PushTransition(paint) => Some(paint.source),
            Item::SkipTransition { source, .. } => Some(*source),
            _ => None,
        }
    }

    /// Moves the item by `(dx, dy)`.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        let r = |r: &mut Rect| {
            r.x += dx;
            r.y += dy;
        };
        match self {
            Item::Shadow(s) => r(&mut s.rect),
            Item::Quad(q) => r(&mut q.rect),
            Item::Custom(p) => r(&mut p.fallback.rect),
            Item::Image(i) => {
                r(&mut i.rect);
                r(&mut i.bounds);
            }
            Item::Glyphs(g) => {
                g.origin.x += dx;
                g.origin.y += dy;
            }
            Item::Path(p) => {
                p.transform = p.transform.then(&Transform::translate(dx, dy));
                r(&mut p.bounds);
            }
            Item::Native { rect, .. } | Item::PushClip { rect, .. } => r(rect),
            Item::PopClip | Item::PushLayer { .. } | Item::PushTransition(_) | Item::SkipTransition { .. } | Item::PopLayer => {}
        }
    }

    /// The item shown at `offset` with its colors faded by `fade`.
    pub fn placed(&self, offset: Vec2, fade: f32) -> Item {
        let mut i = self.clone();
        i.translate(offset.x, offset.y);
        if fade < 1.0 {
            i.fade(fade);
        }
        i
    }

    /// Multiplies the item's colors' alpha by `k`.
    pub fn fade(&mut self, k: f32) {
        let f = |c: &mut Color| *c = c.multiply_alpha(k);
        match self {
            Item::Shadow(s) => f(&mut s.color),
            Item::Quad(q) => q.fade(k),
            Item::Custom(p) => {
                p.fallback.fade(k);
                p.opacity *= k;
            }
            Item::Image(i) => i.opacity *= k,
            Item::Glyphs(g) => f(&mut g.color),
            Item::Path(p) => {
                f(&mut p.color);
                if let Some(g) = &mut p.gradient {
                    f(&mut g.from.0);
                    f(&mut g.to.0);
                }
            }
            Item::PushClip { .. } | Item::PopClip | Item::PopLayer | Item::Native { .. } | Item::SkipTransition { .. } => {}
            Item::PushLayer { opacity, .. } => *opacity *= k,
            Item::PushTransition(paint) => paint.opacity *= k,
        }
    }
}

/// A step of a frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    /// A node's own drawing, in its own coordinates (kept while it does not change), shown at
    /// `offset` with its colors faded by `fade`.
    Drawing { offset: Vec2, fade: f32, items: Arc<[Item]> },
    /// One item in window coordinates: the clips and layers around children.
    Item(Item),
    /// A shared span of drawing steps in the same coordinate space.
    Group(Arc<[Part]>),
}

/// An item borrowed from a display list, with its drawing's placement in the window.
#[derive(Clone, Copy, Debug)]
pub struct PlacedItem<'a> {
    pub item: &'a Item,
    pub offset: Vec2,
    pub fade: f32,
}

impl<'a> PlacedItem<'a> {
    /// Gets the item in window coordinates, borrowing it when no offset or fade is needed.
    pub fn resolve(self) -> Cow<'a, Item> {
        if self.offset != Vec2::default() || self.fade < 1.0 {
            Cow::Owned(self.item.placed(self.offset, self.fade))
        } else {
            Cow::Borrowed(self.item)
        }
    }
}

/// A frame: the window size, its background and the parts in drawing order.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayList {
    pub size: Size,
    pub background: Color,
    pub parts: Vec<Part>,
}

impl DisplayList {
    /// The drawing steps in order, descending through shared groups.
    pub fn parts(&self) -> impl Iterator<Item = &Part> {
        let mut remaining = vec![self.parts.iter()];
        std::iter::from_fn(move || {
            loop {
                match remaining.last_mut()?.next() {
                    Some(Part::Group(parts)) => remaining.push(parts.iter()),
                    Some(part) => return Some(part),
                    None => {
                        remaining.pop();
                    }
                }
            }
        })
    }

    /// Iterates the shared items in drawing order without flattening or copying their payloads.
    pub fn placed_items(&self) -> impl Iterator<Item = PlacedItem<'_>> {
        self.parts().flat_map(|part| {
            let (items, offset, fade) = match part {
                Part::Drawing { offset, fade, items } => (items.as_ref(), *offset, *fade),
                Part::Item(item) => (slice::from_ref(item), Vec2::default(), 1.0),
                Part::Group(_) => unreachable!("parts expands shared groups"),
            };
            items.iter().map(move |item| PlacedItem { item, offset, fade })
        })
    }

    /// Clones every item in drawing order, in window coordinates. Use [`Self::placed_items`]
    /// to stream the shared items with their placement.
    pub fn items(&self) -> Vec<Item> {
        self.placed_items().map(|placed| placed.resolve().into_owned()).collect()
    }

    /// Calls `f` with every item in drawing order, in window coordinates.
    pub fn each_item(&self, mut f: impl FnMut(Item)) {
        for placed in self.placed_items() {
            f(placed.resolve().into_owned());
        }
    }
    pub fn quads(&self) -> impl Iterator<Item = Quad> {
        self.items().into_iter().filter_map(|i| if let Item::Quad(q) = i { Some(q) } else { None })
    }
    pub fn paths(&self) -> impl Iterator<Item = PathPaint> {
        self.items().into_iter().filter_map(|i| if let Item::Path(p) = i { Some(p) } else { None })
    }
    pub fn glyph_runs(&self) -> impl Iterator<Item = GlyphRun> {
        self.items().into_iter().filter_map(|i| if let Item::Glyphs(g) = i { Some(g) } else { None })
    }
}

/// A painted frame in straight-alpha sRGB RGBA8, ordered from top row to bottom row.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct Picture {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Picture {
    /// Creates a picture if the byte length matches the given dimensions.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        (image::rgba_len(width, height)? == rgba.len()).then_some(Self { width, height, rgba })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    pub fn into_rgba(self) -> Vec<u8> {
        self.rgba
    }

    /// The pixel at the given coordinates, if they are inside the picture.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let index = y as usize * self.width as usize + x as usize;
        Some(self.rgba.as_chunks::<4>().0[index])
    }
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({}×{})", self.width, self.height)
    }
}

impl<'de> Deserialize<'de> for Picture {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Rgba {
            width: u32,
            height: u32,
            rgba: Vec<u8>,
        }
        let Rgba { width, height, rgba } = Rgba::deserialize(deserializer)?;
        Self::new(width, height, rgba).ok_or_else(|| serde::de::Error::custom("invalid RGBA8 dimensions or byte count"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placed_items_borrow_payloads_and_preserve_nested_painter_order() {
        let shared: Arc<[Item]> =
            Arc::from([Item::PushClip { source: NodeId::default(), rect: Rect::new(1.0, 2.0, 8.0, 9.0), radius: [2.0; 4] }]);
        let layer: Arc<[Item]> = Arc::from([Item::PushLayer { source: NodeId::default(), opacity: 0.8 }, Item::PopLayer]);
        let offset = Vec2::new(4.0, 5.0);
        let parts = vec![
            Part::Group(Arc::from([
                Part::Drawing { offset, fade: 1.0, items: shared.clone() },
                Part::Group(Arc::from([Part::Drawing { offset: Vec2::default(), fade: 0.5, items: layer.clone() }])),
            ])),
            Part::Item(Item::PopClip),
        ];
        let list = DisplayList { size: Size::new(20.0, 20.0), background: Color::WHITE, parts };
        let mut items = list.placed_items();
        let clip = items.next().unwrap();
        assert!(std::ptr::eq(clip.item, &shared[0]));
        assert_eq!(clip.offset, offset);
        assert!(matches!(clip.resolve().as_ref(), Item::PushClip { rect, .. } if *rect == Rect::new(5.0, 7.0, 8.0, 9.0)));
        let faded = items.next().unwrap();
        assert!(std::ptr::eq(faded.item, &layer[0]));
        assert!(matches!(faded.resolve().as_ref(), Item::PushLayer { opacity, .. } if *opacity == 0.4));
        assert!(std::ptr::eq(items.next().unwrap().item, &layer[1]));
        assert!(matches!(items.next().unwrap().resolve(), Cow::Borrowed(Item::PopClip)));
        assert!(items.next().is_none());
    }

    #[test]
    fn placed_glyphs_use_the_borrowed_run_and_preserve_color_overrides() {
        let glyph = Glyph { id: 1, x: 3.0, y: 4.0, source_end: 1 };
        let run = GlyphRun {
            source: NodeId::default(),
            font: FontFace { id: FontId(1), data: FontData::from(&[][..]), index: 0 },
            size: 12.0,
            coords: vec![42],
            embolden: false,
            skew: 0.0,
            color: Color::WHITE,
            colors: Some(Arc::from([Color::BLACK])),
            origin: Point::new(5.0, 6.0),
            glyphs: Arc::from([glyph, Glyph { x: 7.0, source_end: 2, ..glyph }]),
        };
        let expected = [
            (Glyph { x: 10.0, y: 6.0, ..glyph }, Color::BLACK),
            (Glyph { x: 14.0, y: 6.0, source_end: 2, ..glyph }, Color::WHITE.multiply_alpha(0.5)),
        ];
        assert!(run.placed_colored_at(Vec2::new(2.0, -4.0), 0.5).eq(expected));
    }

    #[test]
    fn picture_wire_values_validate_rgba_dimensions() {
        let picture = Picture::new(2, 1, vec![0, 1, 2, 3, 4, 5, 6, 7]).unwrap();
        assert_eq!(picture.pixel(1, 0), Some([4, 5, 6, 7]));
        assert_eq!(picture.pixel(2, 0), None);
        assert_eq!(serde_json::from_value::<Picture>(serde_json::to_value(&picture).unwrap()).unwrap(), picture);
        assert!(serde_json::from_str::<Picture>(r#"{"width":2,"height":1,"rgba":[0,1,2,3]}"#).is_err());
    }
}
