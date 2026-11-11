//! The checked-in API format (`api.json`): one entry per public item of the framework's crates,
//! in Octara's own stable shape. rustdoc's JSON changes with every nightly; this does not.

use serde::{Deserialize, Serialize};

/// Bumped when the shape below changes incompatibly.
pub const FORMAT: u32 = 1;

/// The whole public API of the framework.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Api {
    pub format: u32,
    pub crates: Vec<CrateApi>,
}

/// One crate's public items, sorted by path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrateApi {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs: String,
    pub items: Vec<Item>,
}

/// One public item. `path` is its canonical public path (`octara::view::Styled::bg`); other public
/// paths to the same item (the prelude, re-exports) are its `aliases`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub path: String,
    pub kind: Kind,
    /// The declaration as Rust source (`pub fn state<T: Clone + 'static>(..) -> Signal<T>`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sig: String,
    /// The generic `impl` block a method is declared in, when it adds parameters or bounds.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub within: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs: String,
    /// The source file, relative to the workspace (no line: lines move on every edit).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Types: the traits they implement (`From<Color>`, `Styled`), auto and blanket impls left out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<String>,
    /// Traits: the types implementing them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub implementors: Vec<String>,
    /// The public paths the docs link to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
    /// Re-exports of another crate's item or crate: what it points at.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Module,
    Struct,
    Enum,
    Union,
    Variant,
    Field,
    Trait,
    /// A free function.
    Fn,
    /// A function taking `self`.
    Method,
    /// An associated function without `self` (constructors).
    AssocFn,
    Const,
    AssocConst,
    AssocType,
    TypeAlias,
    Static,
    Macro,
    /// A public re-export of another crate (or of an item of another crate).
    Reexport,
}

impl Api {
    /// Every item of every crate.
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.crates.iter().flat_map(|c| c.items.iter())
    }

    /// The item at `path`, canonical or alias (a type's field before its method of the same name).
    pub fn get(&self, path: &str) -> Option<&Item> {
        self.all(path).next()
    }

    /// Every item at `path`: a field and a method may share one.
    pub fn all<'a>(&'a self, path: &str) -> impl Iterator<Item = &'a Item> + use<'a> {
        let path = path.to_string();
        self.items().filter(move |i| i.path == path || i.aliases.contains(&path))
    }
}
