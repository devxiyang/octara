//! Geometry in logical pixels, including overlay placement.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// The point at the given logical coordinates.
pub const fn point(x: f32, y: f32) -> Point {
    Point::new(x, y)
}

impl From<(f32, f32)> for Point {
    fn from((x, y): (f32, f32)) -> Self {
        Self::new(x, y)
    }
}

impl From<Point> for (f32, f32) {
    fn from(value: Point) -> Self {
        (value.x, value.y)
    }
}

/// A displacement in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

impl From<(f32, f32)> for Vec2 {
    fn from((x, y): (f32, f32)) -> Self {
        Self::new(x, y)
    }
}

impl From<Vec2> for (f32, f32) {
    fn from(value: Vec2) -> Self {
        (value.x, value.y)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Size<T = f32> {
    pub width: T,
    pub height: T,
}

impl<T> Size<T> {
    pub const fn new(width: T, height: T) -> Self {
        Self { width, height }
    }
}

impl<T> From<(T, T)> for Size<T> {
    fn from((width, height): (T, T)) -> Self {
        Self::new(width, height)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Edges<T = f32> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

impl<T> From<[T; 4]> for Edges<T> {
    fn from([top, right, bottom, left]: [T; 4]) -> Self {
        Self { top, right, bottom, left }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LayoutBox {
    /// Border box in parent coordinates, before the parent's scroll offset.
    pub rect: Rect,
    pub padding: Edges,
    pub border: Edges,
}

/// An axis-aligned rectangle in logical pixels.
#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        let (x, y) = (self.x.max(o.x), self.y.max(o.y));
        let (r, b) = (self.right().min(o.right()), self.bottom().min(o.bottom()));
        Rect { x, y, w: (r - x).max(0.0), h: (b - y).max(0.0) }
    }
    pub fn contains(&self, at: Point) -> bool {
        !self.is_empty() && at.x >= self.x && at.x < self.right() && at.y >= self.y && at.y < self.bottom()
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    /// Fits circular radii (top-left clockwise) without letting adjacent corners overlap.
    pub fn corner_radii(&self, radii: [f32; 4]) -> [f32; 4] {
        let limit = self.w.max(self.h).max(1.0) * 1.0e6;
        let radii = radii.map(|radius| radius.max(0.0).min(limit));
        let [tl, tr, br, bl] = radii;
        let mut scale: f32 = 1.0;
        for (side, sum) in [(self.w, tl + tr), (self.w, bl + br), (self.h, tl + bl), (self.h, tr + br)] {
            if sum > side && sum > 0.0 {
                scale = scale.min(side.max(0.0) / sum);
            }
        }
        radii.map(|radius| radius * scale)
    }

    /// Tests a point against the rounded border box.
    pub fn contains_rounded(&self, at: Point, radii: [f32; 4]) -> bool {
        if !self.contains(at) {
            return false;
        }
        if radii == [0.0; 4] {
            return true;
        }
        let (left, top) = (at.x - self.x, at.y - self.y);
        let (right, bottom) = (self.w - left, self.h - top);
        [(left, top), (right, top), (right, bottom), (left, bottom)]
            .into_iter()
            .zip(self.corner_radii(radii))
            .all(|((x, y), radius)| x >= radius || y >= radius || (radius - x).hypot(radius - y) <= radius)
    }
}

/// A host window's layout extent and usable area in logical coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowViewport {
    pub size: Size,
    /// Device pixels per logical pixel.
    pub scale: f32,
    pub safe_area: Option<Rect>,
    /// Logical pixels occluded by a keyboard at the bottom of the window.
    #[serde(default)]
    pub keyboard_inset: f32,
}

impl Default for WindowViewport {
    fn default() -> Self {
        Self { size: Size::default(), scale: 1.0, safe_area: None, keyboard_inset: 0.0 }
    }
}

impl WindowViewport {
    /// The usable area clipped to the viewport's logical extent.
    pub fn safe_bounds(&self) -> Rect {
        let bounds = Rect::new(0.0, 0.0, self.size.width, self.size.height);
        self.safe_area.map_or(bounds, |safe_area| bounds.intersect(&safe_area))
    }

    /// Layout extent above the keyboard, including the system safe-area margins.
    pub fn unobscured_size(&self) -> Size {
        Size::new(self.size.width, (self.size.height - self.keyboard_inset.max(0.0)).max(0.0))
    }

    /// The intersection of the window's safe area and the area above the keyboard.
    pub fn visible_bounds(&self) -> Rect {
        let size = self.unobscured_size();
        Rect::new(0.0, 0.0, size.width, size.height).intersect(&self.safe_bounds())
    }
}

/// Which side of its anchor an overlay opens on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

impl Side {
    fn opposite(self) -> Side {
        match self {
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// How an overlay lines up with its anchor along the other axis.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Align {
    Start,
    Center,
    End,
}

/// An overlay opens beside its anchor and flips to the opposite side if needed.
/// It stays at least `margin` inside the usable area. Enable `fit` to limit the
/// overlay's size so it can scroll its content.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Placement {
    pub side: Side,
    pub align: Align,
    /// Distance from the anchor.
    pub gap: f32,
    /// Minimum distance from the window's edges, typically 8 logical pixels.
    pub margin: f32,
    /// Limit the size to the room available and scroll the content.
    pub fit: bool,
}

/// Where the last layout put an overlay.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Placed {
    pub x: f32,
    pub y: f32,
    pub side: Side,
    /// The size limit along the side's axis (with `fit`).
    pub limit: Option<f32>,
    /// It fits neither side and was pushed into the window (without `fit`).
    pub clamped: bool,
}

impl Placement {
    /// Below the anchor, left edges aligned, limited to the room below or above (menus, selects).
    pub const BELOW: Placement = Placement { side: Side::Bottom, align: Align::Start, gap: 4.0, margin: 8.0, fit: true };
    /// Above the anchor, centered (tooltips).
    pub const ABOVE: Placement = Placement { side: Side::Top, align: Align::Center, gap: 4.0, margin: 8.0, fit: false };
    /// To the right, top edges aligned (submenus).
    pub const RIGHT: Placement = Placement { side: Side::Right, align: Align::Start, gap: 0.0, margin: 8.0, fit: true };

    pub const fn side(self, side: Side) -> Self {
        Placement { side, ..self }
    }
    pub const fn align(self, align: Align) -> Self {
        Placement { align, ..self }
    }
    pub const fn gap(self, gap: f32) -> Self {
        Placement { gap, ..self }
    }
    pub const fn margin(self, margin: f32) -> Self {
        Placement { margin, ..self }
    }
    pub const fn fit(self, fit: bool) -> Self {
        Placement { fit, ..self }
    }

    /// Places an overlay next to its anchor inside the usable viewport.
    pub fn place(self, anchor: Rect, size: Size, bounds: Rect) -> Placed {
        let opposite = self.side.opposite();
        let room = |side| match side {
            Side::Bottom => bounds.bottom() - anchor.bottom() - self.gap - self.margin,
            Side::Top => anchor.y - bounds.y - self.gap - self.margin,
            Side::Right => bounds.right() - anchor.right() - self.gap - self.margin,
            Side::Left => anchor.x - bounds.x - self.gap - self.margin,
        };
        let extent = match self.side {
            Side::Top | Side::Bottom => size.height,
            Side::Left | Side::Right => size.width,
        };
        let preferred_room = room(self.side);
        let opposite_room = room(opposite);
        let neither_fits = preferred_room < extent && opposite_room < extent;
        let side = if preferred_room >= extent || (neither_fits && preferred_room >= opposite_room) { self.side } else { opposite };
        let limit = (self.fit && neither_fits).then_some(room(side).max(0.0));
        let size = match (side, limit) {
            (Side::Top | Side::Bottom, Some(limit)) => Size { height: limit, ..size },
            (Side::Left | Side::Right, Some(limit)) => Size { width: limit, ..size },
            (_, None) => size,
        };
        let Point { x, y } = self.position(side, anchor, size, bounds);
        Placed { x, y, side, limit, clamped: neither_fits && !self.fit }
    }

    /// Positions an already measured overlay on its chosen side of the anchor.
    pub fn position(self, side: Side, anchor: Rect, size: Size, bounds: Rect) -> Point {
        let Size { width, height } = size;
        let (x, y) = match side {
            Side::Top => (self.aligned(anchor.x, anchor.w, width), anchor.y - self.gap - height),
            Side::Bottom => (self.aligned(anchor.x, anchor.w, width), anchor.bottom() + self.gap),
            Side::Left => (anchor.x - self.gap - width, self.aligned(anchor.y, anchor.h, height)),
            Side::Right => (anchor.right() + self.gap, self.aligned(anchor.y, anchor.h, height)),
        };
        Point::new(
            x.min(bounds.right() - self.margin - width).max(bounds.x + self.margin),
            y.min(bounds.bottom() - self.margin - height).max(bounds.y + self.margin),
        )
    }

    fn aligned(self, start: f32, available: f32, extent: f32) -> f32 {
        match self.align {
            Align::Start => start,
            Align::Center => start + (available - extent) / 2.0,
            Align::End => start + available - extent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_flips_and_fits_inside_an_offset_safe_area() {
        let bounds = Rect::new(16.0, 32.0, 160.0, 100.0);
        let anchor = Rect::new(150.0, 104.0, 20.0, 12.0);
        let placed = Placement::BELOW.place(anchor, Size::new(70.0, 80.0), bounds);
        assert_eq!(placed, Placed { x: 98.0, y: 40.0, side: Side::Top, limit: Some(60.0), clamped: false });
    }
}
