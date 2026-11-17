//! Pure anchor geometry, independent of mounted windows and their interaction policy.

/// Which side of its anchor an overlay opens on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Start,
    Center,
    End,
}

/// Where an overlay goes relative to its anchor (as Rune's `anchored`). It opens on `side`,
/// flips to the opposite one when it does not fit there, and stays `margin` inside the window.
/// With `fit`, it is also limited to the room on its side and scrolls the rest (Rune's
/// `anchored_fit`, Floating UI's `size`), instead of being pushed over its anchor.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Placement {
    pub side: Side,
    pub align: Align,
    /// Distance from the anchor.
    pub gap: f32,
    /// Minimum distance from the window's edges (8, as in Rune).
    pub margin: f32,
    /// Limit the size to the room available and scroll the content.
    pub fit: bool,
}

/// Where the last layout put an overlay.
#[derive(Clone, Copy, PartialEq, Debug)]
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

    /// Room for the overlay on `side` of anchor `a` in viewport `vp`, and the overlay's extent
    /// along that axis.
    fn room(self, side: Side, a: (f32, f32, f32, f32), size: (f32, f32), vp: (f32, f32)) -> (f32, f32) {
        let (ax, ay, aw, ah) = a;
        let m = self.margin;
        match side {
            Side::Bottom => (vp.1 - m - (ay + ah + self.gap), size.1),
            Side::Top => (ay - self.gap - m, size.1),
            Side::Right => (vp.0 - m - (ax + aw + self.gap), size.0),
            Side::Left => (ax - self.gap - m, size.0),
        }
    }

    /// Places an overlay of natural `size` next to anchor rect `a` in viewport `vp`.
    pub fn place(self, a: (f32, f32, f32, f32), size: (f32, f32), vp: (f32, f32)) -> Placed {
        let (ax, ay, aw, ah) = a;
        let opp = self.side.opposite();
        let (room, extent) = self.room(self.side, a, size, vp);
        let (room_opp, _) = self.room(opp, a, size, vp);
        let fits = |r: f32| r >= extent;
        // The requested side if it fits, else the opposite if that fits, else the roomier one.
        let side = if fits(room) || (!fits(room_opp) && room >= room_opp) { self.side } else { opp };
        let room_side = if side == self.side { room } else { room_opp };
        let neither = !fits(room) && !fits(room_opp);
        let limit = (self.fit && neither).then_some(room_side.max(0.0));
        let (mut w, mut h) = size;
        if let Some(l) = limit {
            match side {
                Side::Top | Side::Bottom => h = l,
                Side::Left | Side::Right => w = l,
            }
        }
        let main = match side {
            Side::Bottom => ay + ah + self.gap,
            Side::Top => ay - self.gap - h,
            Side::Right => ax + aw + self.gap,
            Side::Left => ax - self.gap - w,
        };
        let m = self.margin;
        let clamp = |v: f32, len: f32, max: f32| v.min(max - m - len).max(m);
        let cross = |start: f32, alen: f32, len: f32| match self.align {
            Align::Start => start,
            Align::Center => start + (alen - len) / 2.0,
            Align::End => start + alen - len,
        };
        let (x, y) = match side {
            Side::Top | Side::Bottom => (clamp(cross(ax, aw, w), w, vp.0), clamp(main, h, vp.1)),
            Side::Left | Side::Right => (clamp(main, w, vp.0), clamp(cross(ay, ah, h), h, vp.1)),
        };
        Placed { x, y, side, limit, clamped: neither && !self.fit }
    }
}
