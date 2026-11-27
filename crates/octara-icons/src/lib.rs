//! The [Lucide](https://lucide.dev) icon set for Octara: every icon, as path data on a 24-unit
//! grid drawn in 2-unit round strokes, tinted by the text color.
//!
//! ```
//! use octara::prelude::*;
//! use octara_icons as icons;
//!
//! let search = el().child(icon(icons::SEARCH.into())).label("Search").focusable();
//! # let _ = search;
//! ```
//!
//! Find an icon by name or by what it shows with [`by_name`] and [`search`]; `icons::ALL` lists
//! them all. Generated from lucide-static [`VERSION`] (see `examples/generate.rs`); the icons are
//! ISC licensed, some MIT from Feather (`LICENSE-LUCIDE`).

pub use octara_model::paint::Icon;
use std::hash::{Hash, Hasher};

use octara_model::{
    geometry::{Rect, point},
    paint::{FillRule, IconShape, LineCap, LineJoin, Path, PathCmd, PathStyle, Stroke},
};

/// A bundled icon defined by static path commands and identified by its canonical name.
#[derive(Clone, Copy, Debug)]
pub struct LucideIcon {
    name: &'static str,
    parts: &'static [Part],
}

impl LucideIcon {
    const fn new(name: &'static str, parts: &'static [Part]) -> Self {
        Self { name, parts }
    }

    pub const fn name(self) -> &'static str {
        self.name
    }

    /// Materializes the icon with Lucide's round stroke and default width of two grid units.
    pub fn to_icon(self) -> Icon {
        self.with_stroke_width(2.0)
    }

    /// Materializes the icon with a different stroke width in grid units.
    pub fn with_stroke_width(self, width: f32) -> Icon {
        let stroke = Stroke::new(width).cap(LineCap::Round).join(LineJoin::Round);
        let mut shapes = Vec::with_capacity(self.parts.len());
        for part in self.parts {
            let path = Path::from_commands(part.commands.iter().copied());
            if part.fill {
                shapes.push(IconShape { path: path.clone(), style: PathStyle::Fill(FillRule::NonZero) });
            }
            if part.stroke {
                shapes.push(IconShape { path, style: PathStyle::Stroke(stroke.clone()) });
            }
        }
        Icon::new(self.name, Rect::new(0.0, 0.0, 24.0, 24.0), shapes)
    }
}

impl From<LucideIcon> for Icon {
    fn from(icon: LucideIcon) -> Self {
        icon.to_icon()
    }
}

impl PartialEq for LucideIcon {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for LucideIcon {}

impl Hash for LucideIcon {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

#[derive(Clone, Copy, Debug)]
struct Part {
    commands: &'static [PathCmd],
    fill: bool,
    stroke: bool,
}

// Compact names keep each generated glyph on a single line.
const fn s(commands: &'static [PathCmd]) -> Part {
    Part { commands, fill: false, stroke: true }
}

const fn f(commands: &'static [PathCmd], stroke: bool) -> Part {
    Part { commands, fill: true, stroke }
}

const fn m(x: f32, y: f32) -> PathCmd {
    PathCmd::MoveTo(point(x, y))
}

const fn l(x: f32, y: f32) -> PathCmd {
    PathCmd::LineTo(point(x, y))
}

const fn q(x1: f32, y1: f32, x: f32, y: f32) -> PathCmd {
    PathCmd::QuadTo(point(x1, y1), point(x, y))
}

const fn c(x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) -> PathCmd {
    PathCmd::CurveTo(point(x1, y1), point(x2, y2), point(x, y))
}

const Z: PathCmd = PathCmd::Close;

// Generated, one icon per line: rustfmt would spread it over ten times the lines.
#[rustfmt::skip]
#[allow(clippy::approx_constant)]
mod lucide;
pub use lucide::*;

/// An icon with its name and the words it is found by.
pub struct Entry {
    pub name: &'static str,
    pub tags: &'static [&'static str],
    pub icon: LucideIcon,
}

/// The icon named `name` (Lucide's kebab-case name: `"arrow-left"`).
pub fn by_name(name: &str) -> Option<Icon> {
    ALL.binary_search_by(|e| e.name.cmp(name)).ok().map(|i| ALL[i].icon.to_icon())
}

/// Icons whose name or tags contain every word of `query`, names matching first.
pub fn search(query: &str) -> Vec<&'static Entry> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let hit = |s: &str, w: &str| s.contains(w);
    let mut found: Vec<(bool, &'static Entry)> = ALL
        .iter()
        .filter(|e| words.iter().all(|w| hit(e.name, w) || e.tags.iter().any(|t| hit(t, w))))
        .map(|e| (words.iter().all(|w| hit(e.name, w)), e))
        .collect();
    found.sort_by_key(|(by_name, e)| (!by_name, e.name.len(), e.name));
    found.into_iter().map(|(_, e)| e).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn icon_identity_uses_the_canonical_name() {
        let same_name = LucideIcon::new(SEARCH.name(), &[]);
        assert_eq!(SEARCH, same_name);
        assert_ne!(SEARCH, HOUSE);
        assert_eq!(HashSet::from([SEARCH, same_name, HOUSE]).len(), 2);
    }
}
