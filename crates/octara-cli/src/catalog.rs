//! The usage catalog: one entry per thing an app can use, merged from the API (signatures and
//! docs read from source) and the model's tables (diagnostic codes, theme colors and springs).

use crate::api::{Api, Kind};
use octara_model::diagnostics::{Code, Severity, codes};
use octara_model::motion::Spring;
use octara_model::theme::{Color, ColorToken, Theme};
use serde::Serialize;
use std::collections::BTreeMap;

/// One API item or diagnostic code.
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    /// The canonical path (`octara::view::Styled::bg`) or the code (`STALE_TARGET`).
    pub path: String,
    /// The API kind (`method`, `struct`, ..) or `diagnostic`.
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sig: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub within: String,
    /// The first paragraph of the docs, on one line.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub docs: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub file: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Paths of related entries: the parent type, doc links, the API a cause names.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<String>,
    /// Data from the model, including token colors, spring duration and diagnostic causes.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub facts: BTreeMap<String, String>,
    /// Types and traits: their members' names.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    /// Reachable with `use octara::prelude::*` (itself, or a member of a prelude type).
    pub prelude: bool,
}

impl Entry {
    /// The last path segment (`bg`), or the code itself.
    pub fn name(&self) -> &str {
        self.path.rsplit("::").next().unwrap_or(&self.path)
    }
}

pub struct Catalog {
    pub entries: Vec<Entry>,
}

/// What a lookup found: entries named exactly that, then entries whose names contain it.
#[derive(Serialize)]
pub struct Found<'a> {
    pub query: String,
    pub matches: Vec<&'a Entry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub similar: Vec<&'a str>,
}

/// The first paragraph of `docs`, on one line, links written as their text.
pub fn summary(docs: &str) -> String {
    let para: Vec<&str> = docs.lines().map(str::trim).take_while(|l| !l.is_empty()).collect();
    link_text(&para.join(" "))
}

/// `[text](target)` and `[`name`]` as their text.
fn link_text(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']').map(|c| open + c) else { break };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..close]);
        let after = &rest[close + 1..];
        rest = match after.strip_prefix('(').and_then(|a| a.find(')').map(|e| &a[e + 1..])) {
            Some(r) => r,
            None => after,
        };
    }
    out.push_str(rest);
    out
}

/// The first sentence of a summary.
pub fn first_sentence(s: &str) -> &str {
    s.find(". ").map(|i| &s[..=i]).unwrap_or(s)
}

fn hex(c: Color) -> String {
    let c = c.to_rgba8();
    if c.a == 255 { format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b) } else { format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a) }
}

fn severity(s: Severity) -> &'static str {
    match s {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

/// The API that code in backticks names: calls (`flex_shrink()`, `.overflow_y_scroll()`) and
/// types (`Placement`), not variables (`items` in `items.item(&key)` is one).
fn backticked_names(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for code in text.split('`').skip(1).step_by(2) {
        let mut rest = code;
        while let Some(start) = rest.find(|c: char| c.is_alphabetic() || c == '_') {
            let tail = &rest[start..];
            let end = tail.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(tail.len());
            let (word, after) = tail.split_at(end);
            let call = after.starts_with('(');
            let ty = word.starts_with(|c: char| c.is_ascii_uppercase()) && word.chars().any(|c| c.is_ascii_lowercase());
            if word.len() > 1 && (call || ty) {
                out.push(word);
            }
            rest = after;
        }
    }
    out
}

impl Catalog {
    pub fn build(api: &Api) -> Catalog {
        let mut entries: Vec<Entry> = Vec::new();
        let light = Theme::light();
        let dark = Theme::dark();

        for (item, root) in api.crates.iter().flat_map(|c| c.items.iter().map(move |i| (i, &c.root))) {
            // A code's static is the code's own entry (below).
            if item.path.strip_prefix("octara_model::diagnostics::codes::").is_some_and(|n| codes::lookup(n).is_some()) {
                continue;
            }
            let mut facts = BTreeMap::new();
            let parent = item.path.rsplit_once("::").map(|(p, _)| p.to_string()).unwrap_or_default();
            let name = item.path.rsplit("::").next().unwrap_or_default();
            if parent == "octara_model::theme::ColorToken"
                && let Some(t) = ColorToken::BUILTIN.iter().find(|t| format!("{t:?}") == name)
            {
                facts.insert("light".into(), hex(light.get(*t)));
                facts.insert("dark".into(), hex(dark.get(*t)));
            }
            if parent == "octara_model::motion::Spring"
                && let Some((_, s)) = Spring::NAMED.iter().find(|(n, _)| *n == name)
            {
                facts.insert("duration".into(), format!("{} s", s.duration));
                facts.insert("bounce".into(), format!("{}", s.bounce));
            }
            if !item.target.is_empty() {
                facts.insert("target".into(), item.target.clone());
            }
            if !item.traits.is_empty() {
                facts.insert("traits".into(), item.traits.join(", "));
            }
            if !item.implementors.is_empty() {
                facts.insert("implementors".into(), item.implementors.join(", "));
            }
            if let Some(d) = &item.deprecated {
                facts.insert("deprecated".into(), d.clone());
            }
            let member =
                matches!(item.kind, Kind::Method | Kind::AssocFn | Kind::AssocConst | Kind::AssocType | Kind::Field | Kind::Variant);
            let mut related = Vec::new();
            if member {
                related.push(parent.clone());
            }
            related.extend(item.links.iter().cloned());
            entries.push(Entry {
                path: item.path.clone(),
                kind: serde_json::to_value(item.kind).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default(),
                sig: item.sig.clone(),
                within: item.within.clone(),
                summary: summary(&item.docs),
                docs: item.docs.clone(),
                file: if root.is_empty() { item.file.clone() } else { format!("{root}/{}", item.file) },
                aliases: item.aliases.clone(),
                related,
                facts,
                members: Vec::new(),
                prelude: item.aliases.iter().any(|a| a.contains("::prelude::")),
            });
        }
        // The crates themselves, their top-level items as members.
        for c in &api.crates {
            let root = c.name.replace('-', "_");
            let members = c
                .items
                .iter()
                .filter_map(|i| i.path.strip_prefix(&format!("{root}::")).filter(|rest| !rest.contains("::")).map(str::to_owned))
                .collect();
            entries.push(Entry {
                path: root,
                kind: "crate".into(),
                sig: String::new(),
                within: String::new(),
                summary: summary(&c.docs),
                docs: c.docs.clone(),
                file: String::new(),
                aliases: Vec::new(),
                related: Vec::new(),
                facts: BTreeMap::new(),
                members,
                prelude: false,
            });
        }

        // Members of prelude types are in reach of the prelude too.
        let prelude_types: std::collections::BTreeSet<String> = entries.iter().filter(|e| e.prelude).map(|e| e.path.clone()).collect();
        for e in &mut entries {
            if let Some((parent, _)) = e.path.rsplit_once("::")
                && prelude_types.contains(parent)
            {
                e.prelude = true;
            }
        }

        // Members onto their types and traits.
        let mut members: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for e in &entries {
            if matches!(e.kind.as_str(), "method" | "assoc_fn" | "assoc_const" | "assoc_type" | "field" | "variant")
                && let Some((parent, name)) = e.path.rsplit_once("::")
            {
                members.entry(parent.to_string()).or_default().push(name.to_string());
            }
        }
        // Modules: their direct items.
        let modules: std::collections::BTreeSet<String> = entries.iter().filter(|e| e.kind == "module").map(|e| e.path.clone()).collect();
        for e in &entries {
            if let Some((parent, name)) = e.path.rsplit_once("::")
                && modules.contains(parent)
            {
                members.entry(parent.to_string()).or_default().push(name.to_string());
            }
        }
        for e in &mut entries {
            if let Some(m) = members.remove(&e.path) {
                e.members = m;
            }
        }

        let index: BTreeMap<String, String> = {
            // Bare names that resolve to one public path, for linking names in prose.
            let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for e in &entries {
                by_name.entry(e.name().to_string()).or_default().push(e.path.clone());
            }
            by_name.into_iter().filter(|(_, v)| v.len() == 1).map(|(k, mut v)| (k, v.remove(0))).collect()
        };

        for c in codes::ALL {
            entries.push(Self::diagnostic(c, &index));
        }
        Catalog { entries }
    }

    fn diagnostic(c: &Code, index: &BTreeMap<String, String>) -> Entry {
        let mut facts = BTreeMap::new();
        facts.insert("severity".into(), severity(c.severity).into());
        facts.insert("cause".into(), c.cause.into());
        facts.insert("doc".into(), c.doc.into());
        let mut related: Vec<String> = backticked_names(c.cause).into_iter().filter_map(|n| index.get(n).cloned()).collect();
        related.dedup();
        Entry {
            path: c.id.into(),
            kind: "diagnostic".into(),
            sig: String::new(),
            within: String::new(),
            summary: c.summary.into(),
            docs: String::new(),
            file: String::new(),
            aliases: vec![format!("octara_model::diagnostics::codes::{}", c.id), format!("octara::diagnostics::codes::{}", c.id)],
            related,
            facts,
            members: Vec::new(),
            prelude: false,
        }
    }

    /// What `use octara::prelude::*` brings in (not the members of those types).
    pub fn prelude(&self) -> impl Iterator<Item = &Entry> {
        self.entries
            .iter()
            .filter(|e| e.aliases.iter().any(|a| a.strip_prefix("octara::prelude::").is_some_and(|name| !name.contains("::"))))
    }

    pub fn get(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path == path)
    }

    /// Entries matching `query`: a full path or alias, a path suffix on `::` boundaries
    /// (`Styled::bg`, `bg`) or a code (any case). A bare name prefers standalone items
    /// over type members. Within each group, prelude entries come first.
    pub fn lookup(&self, query: &str) -> Found<'_> {
        let q = query.trim().trim_start_matches('.').trim_end_matches("()");
        let suffix = format!("::{q}");
        let mut matches: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| {
                e.path == q
                    || e.path.ends_with(&suffix)
                    || e.aliases.iter().any(|a| a == q || a.ends_with(&suffix))
                    || (e.kind == "diagnostic" && e.path.eq_ignore_ascii_case(q))
            })
            .collect();
        let bare = !q.contains("::");
        matches.sort_by_key(|e| {
            let member = matches!(e.kind.as_str(), "method" | "assoc_fn" | "assoc_const" | "assoc_type" | "field" | "variant");
            (bare && member, !e.prelude, e.path.matches("::").count(), e.path.clone())
        });
        // Names containing the query (three letters or more), those starting with it first.
        let lower = q.to_lowercase();
        let mut similar: Vec<(bool, &str)> = if lower.len() < 3 {
            Vec::new()
        } else {
            self.entries
                .iter()
                .filter(|e| !matches.iter().any(|m| m.path == e.path))
                .filter_map(|e| {
                    let n = e.name().to_lowercase();
                    n.contains(&lower).then(|| (!n.starts_with(&lower), e.path.as_str()))
                })
                .collect()
        };
        similar.sort_by_key(|(later, p)| (*later, p.len(), *p));
        similar.dedup();
        similar.truncate(20);
        let similar = similar.into_iter().map(|(_, p)| p).collect();
        Found { query: query.to_string(), matches, similar }
    }

    /// The registered code named in `text` (a code, or a message or panic that contains one).
    pub fn code_in(&self, text: &str) -> Option<&Entry> {
        let upper = text.to_uppercase();
        let mut words = upper.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'));
        let found = words.find_map(codes::lookup)?;
        self.get(found.id)
    }
}
