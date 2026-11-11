//! The [Lucide](https://lucide.dev) icon set for Octara: every icon, as path data on a 24-unit
//! grid drawn in 2-unit round strokes, tinted by the text color.
//!
//! ```
//! use octara::prelude::*;
//! use octara_icons as icons;
//!
//! let search = button_like().child(icon(icons::SEARCH)).label("Search");
//! # fn button_like() -> El { el().on_press(|| {}).focusable() }
//! # let _ = search;
//! ```
//!
//! Find an icon by name or by what it shows with [`by_name`] and [`search`]; `icons::ALL` lists
//! them all. Generated from lucide-static [`VERSION`] (see `examples/generate.rs`); the icons are
//! ISC licensed, some MIT from Feather (`LICENSE-LUCIDE`).

pub use octara::icon::Icon;

mod lucide;
pub use lucide::*;

/// An icon with its name and the words it is found by.
pub struct Entry {
    pub name: &'static str,
    pub tags: &'static [&'static str],
    pub icon: Icon,
}

/// The icon named `name` (Lucide's kebab-case name: `"arrow-left"`).
pub fn by_name(name: &str) -> Option<Icon> {
    ALL.binary_search_by(|e| e.name.cmp(name)).ok().map(|i| ALL[i].icon.clone())
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
