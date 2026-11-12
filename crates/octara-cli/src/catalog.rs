//! The usage catalog: one entry per thing an app can use, merged from `api.json` (signatures and
//! docs) and the kernel's own tables (diagnostic codes, built-in commands and their keys, theme
//! token colors, named springs). `octara vocab` and `octara explain` answer from it.

use crate::api::{Api, Kind};
use octara::app::{BUILTIN_COMMANDS, Keymap, TargetPlatform};
use octara::diagnostics::{self, Area, Code, Severity};
use octara::kernel::{Theme, Token};
use octara::motion::Spring;
use serde::Serialize;
use std::collections::BTreeMap;

/// One usable thing: an API item, a diagnostic code or a built-in command.
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    /// The canonical path (`octara::view::Styled::bg`), the code (`OVERFLOW`) or the command
    /// (`focus.next`).
    pub path: String,
    /// The API kind (`method`, `struct`, ..), `diagnostic` or `command`.
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
    /// Data from the kernel: a token's colors, a spring's duration, a command's keys, a code's
    /// cause.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub facts: BTreeMap<String, String>,
    /// Types and traits: their members' names.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    /// Reachable with `use octara::prelude::*` (itself, or a member of a prelude type).
    pub prelude: bool,
}

impl Entry {
    /// The last path segment (`bg`), or the code or command itself.
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

/// The first paragraph of `docs`, on one line, intra-doc link brackets dropped.
pub fn summary(docs: &str) -> String {
    let para: Vec<&str> = docs.lines().map(str::trim).take_while(|l| !l.is_empty()).collect();
    para.join(" ").replace("[`", "`").replace("`]", "`")
}

fn hex(c: octara::kernel::Color) -> String {
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

fn area(a: Area) -> &'static str {
    match a {
        Area::Reactive => "reactive",
        Area::View => "view",
        Area::Layout => "layout",
        Area::Accessibility => "accessibility",
        Area::Inspect => "inspect",
    }
}

/// The chords bound to `command` in a keymap, with their context when they have one.
fn keys(map: &Keymap, command: &str) -> String {
    let ks: Vec<String> = map
        .bindings
        .iter()
        .filter(|b| b.command.as_deref() == Some(command))
        .map(|b| match &b.context {
            Some(c) => format!("{} (in {c})", b.chord),
            None => b.chord.to_string(),
        })
        .collect();
    ks.join(", ")
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
        let mac = Keymap::standard(TargetPlatform::MacOs);
        let other = Keymap::standard(TargetPlatform::Windows);
        let light = Theme::light();
        let dark = Theme::dark();

        for item in api.items() {
            // A code's static is the code's own entry (below).
            if item.path.strip_prefix("octara::diagnostics::").is_some_and(|n| diagnostics::code(n).is_some()) {
                continue;
            }
            let mut facts = BTreeMap::new();
            let parent = item.path.rsplit_once("::").map(|(p, _)| p.to_string()).unwrap_or_default();
            let name = item.path.rsplit("::").next().unwrap_or_default();
            if parent == "octara::kernel::Token"
                && let Some(t) = Token::BUILTIN.iter().find(|t| format!("{t:?}") == name)
            {
                facts.insert("light".into(), hex(light.get(*t)));
                facts.insert("dark".into(), hex(dark.get(*t)));
            }
            if parent == "octara::motion::Spring"
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
                file: item.file.clone(),
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

        for c in diagnostics::CODES {
            entries.push(Self::diagnostic(c, &index));
        }
        for (command, what) in BUILTIN_COMMANDS {
            let mut facts = BTreeMap::new();
            let (m, o) = (keys(&mac, command), keys(&other, command));
            if !m.is_empty() {
                facts.insert("keys.macos".into(), m);
            }
            if !o.is_empty() {
                facts.insert("keys.windows_linux".into(), o);
            }
            entries.push(Entry {
                path: command.to_string(),
                kind: "command".into(),
                sig: String::new(),
                within: String::new(),
                summary: format!("{}{}.", what[..1].to_uppercase(), &what[1..]),
                docs: String::new(),
                file: String::new(),
                aliases: Vec::new(),
                related: vec!["octara::app::App::invoke".into(), "octara::app::App::press_key".into()],
                facts,
                members: Vec::new(),
                prelude: false,
            });
        }
        Catalog { entries }
    }

    fn diagnostic(c: &Code, index: &BTreeMap<String, String>) -> Entry {
        let mut facts = BTreeMap::new();
        facts.insert("severity".into(), severity(c.severity).into());
        facts.insert("area".into(), area(c.area).into());
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
            aliases: vec![format!("octara::diagnostics::{}", c.id)],
            related,
            facts,
            members: Vec::new(),
            prelude: false,
        }
    }

    /// What `use octara::prelude::*` brings in (not the members of those types).
    pub fn prelude(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.aliases.iter().any(|a| a.starts_with("octara::prelude::")))
    }

    pub fn get(&self, path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.path == path)
    }

    /// Entries matching `query`: a full path or alias, a path suffix on `::` boundaries
    /// (`Styled::bg`, `bg`), a code (any case) or a command. Prelude entries and types come first.
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
        matches.sort_by_key(|e| (!e.prelude, e.path.matches("::").count(), e.path.clone()));
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

    /// The index (the skill's `references/index.md`): every name an app can use, grouped,
    /// signatures left to `octara vocab`.
    pub fn index(&self) -> String {
        // (heading before the line, label, names), factored together at the end.
        let mut lines: Vec<(Option<&str>, String, Vec<String>)> = Vec::new();
        let module = |e: &Entry| -> String {
            let parts: Vec<&str> = e.path.split("::").collect();
            parts.get(1).filter(|_| parts.len() > 2).map(|m| m.to_string()).unwrap_or_else(|| parts[0].to_string())
        };
        let mut by_module: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for e in self.prelude() {
            by_module.entry(module(e)).or_default().push(e.name().to_string());
        }
        let mut heading = Some("Prelude (`use octara::prelude::*`), by module:");
        for (m, mut names) in by_module {
            names.sort_by_key(|n| (n.starts_with(|c: char| c.is_uppercase()), n.clone()));
            lines.push((heading.take(), m, names));
        }
        let mut typed: Vec<&Entry> = self.prelude().filter(|e| !e.members.is_empty()).collect();
        typed.sort_by_key(|e| (e.members.len() < 8, e.name().to_string()));
        let mut heading = Some("Members of prelude types (`.name(..)`):");
        for e in typed {
            let label = match e.facts.get("implementors") {
                Some(i) if e.kind == "trait" => {
                    // Generic implementors (closures, `impl<T: ..> T`) by their shape only.
                    let mut shown: Vec<&str> =
                        split_top(i).into_iter().map(|t| if t.starts_with("impl<") { "generic impls" } else { t }).collect();
                    shown.dedup();
                    format!("{} (trait; {})", e.name(), shown.join(", "))
                }
                _ => e.name().to_string(),
            };
            let mut names = e.members.clone();
            // Bitflags types: their flags; the set operations are bitflags' own.
            if e.facts.get("traits").is_some_and(|t| t.split(", ").any(|t| t == "Flags")) {
                names.retain(|n| n.chars().all(|c| c.is_ascii_uppercase() || c == '_'));
            }
            names.dedup();
            lines.push((heading.take(), label, names));
        }
        let mut heading = Some("Other crates and modules:");
        for c in ["octara_kit", "octara_shell", "octara::inspect", "octara::diagnostics", "octara_fonts", "octara_render"] {
            if let Some(e) = self.get(c) {
                let mut names = e.members.clone();
                names.retain(|n| diagnostics::code(n).is_none() && n != "prelude");
                lines.push((heading.take(), c.to_string(), names));
            }
        }
        let commands: Vec<String> = BUILTIN_COMMANDS.iter().map(|(c, _)| c.to_string()).collect();
        lines.push((None, "commands (`App::invoke`, `on_command`)".into(), commands));
        lines.push((None, "diagnostic codes (`octara explain`)".into(), diagnostics::CODES.iter().map(|c| c.id.to_string()).collect()));

        let mut s = String::new();
        for (heading, label, names) in lines {
            if names.is_empty() {
                continue;
            }
            if let Some(h) = heading {
                if !s.is_empty() && !s.ends_with("\n\n") {
                    s += "\n";
                }
                s += h;
                s += "\n";
            }
            s += &format!("- {label}: {}\n", names.join(", "));
        }
        s
    }

    /// The registered code named in `text` (a code, or a message or panic that contains one).
    pub fn code_in(&self, text: &str) -> Option<&Entry> {
        let upper = text.to_uppercase();
        let words = upper.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'));
        let found = words.filter_map(diagnostics::code).next()?;
        self.get(found.id)
    }
}

/// `a, B<c, d>, e` split at the commas outside brackets.
fn split_top(s: &str) -> Vec<&str> {
    let (mut out, mut depth, mut start) = (Vec::new(), 0i32, 0);
    for (i, c) in s.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' if !s[..i].ends_with('-') => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim());
    out
}
