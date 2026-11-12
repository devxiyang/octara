//! The API as reference files for agents (`references/api/` of the skill): an index of crates
//! and modules, and one file per module (a type with many members gets its own), one line per
//! item with its signature, summary and where it is defined. Agents read or grep them without
//! running anything.

use crate::api::{Api, Item, Kind};
use crate::catalog::{first_sentence, summary};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A type with more members than this gets a file of its own.
const OWN_FILE: usize = 40;

fn member(k: Kind) -> bool {
    matches!(k, Kind::Method | Kind::AssocFn | Kind::AssocConst | Kind::AssocType | Kind::Field | Kind::Variant)
}

fn file_name(path: &str) -> String {
    format!("{}.md", path.replace("::", "."))
}

fn line(i: &Item) -> String {
    let mut s = format!("- `{}`", if i.sig.is_empty() { &i.path } else { &i.sig });
    let sum = summary(&i.docs);
    if !sum.is_empty() {
        let _ = write!(s, " — {sum}");
    }
    if !i.file.is_empty() {
        let _ = write!(s, " `{}`", i.file);
    }
    s
}

/// The files, relative to `references/api/`.
pub fn files(api: &Api) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut index = String::from(
        "# Octara API\n\nGenerated from the source this project builds with. Each line ends with where the item is \
         defined, relative to its crate's directory (given per crate below). Look a name up with `octara vocab <name>`, \
         or grep these files.\n",
    );
    for c in &api.crates {
        let krate = c.name.replace('-', "_");
        let _ = write!(index, "\n## {krate}\n\nSource: `{}`\n\n", c.root);
        let docs = summary(&c.docs);
        if !docs.is_empty() {
            let _ = writeln!(index, "{}\n", first_sentence(&docs));
        }
        let prelude: Vec<&str> = c
            .items
            .iter()
            .filter(|i| !member(i.kind) && i.aliases.iter().any(|a| a.contains("::prelude::")))
            .map(|i| i.path.rsplit("::").next().unwrap_or(&i.path))
            .collect();
        if !prelude.is_empty() {
            let _ = writeln!(index, "Prelude (`use {krate}::prelude::*`): {}\n", prelude.join(", "));
        }

        // Items by the module (or type) they belong to.
        let modules: BTreeMap<&str, &Item> = c.items.iter().filter(|i| i.kind == Kind::Module).map(|i| (i.path.as_str(), i)).collect();
        let mut by_parent: BTreeMap<String, Vec<&Item>> = BTreeMap::new();
        for i in &c.items {
            if i.kind == Kind::Module {
                continue;
            }
            let parent = i.path.rsplit_once("::").map(|(p, _)| p.to_string()).unwrap_or_default();
            by_parent.entry(parent).or_default().push(i);
        }
        let mut module_paths: Vec<String> = std::iter::once(krate.clone()).chain(modules.keys().map(|p| p.to_string())).collect();
        module_paths.retain(|m| by_parent.get(m).is_some_and(|v| v.iter().any(|i| !member(i.kind))));
        let _ = writeln!(index, "Modules:\n");
        for m in &module_paths {
            let sum = modules.get(m.as_str()).map(|i| summary(&i.docs)).unwrap_or_else(|| summary(&c.docs));
            let sum = first_sentence(&sum);
            let _ = writeln!(index, "- [`{m}`]({}){}", file_name(m), if sum.is_empty() { String::new() } else { format!(": {sum}") });
        }
        for m in &module_paths {
            let mut s = format!("# {m}\n\nSource: `{}`\n\n", c.root);
            let docs = modules.get(m.as_str()).map(|i| i.docs.clone()).unwrap_or_else(|| c.docs.clone());
            if !docs.is_empty() {
                let _ = writeln!(s, "{docs}\n");
            }
            for i in by_parent.get(m).into_iter().flatten().filter(|i| !member(i.kind)) {
                let _ = writeln!(s, "{}", line(i));
                let members: Vec<&Item> = by_parent.get(&i.path).into_iter().flatten().filter(|m| member(m.kind)).copied().collect();
                if members.len() > OWN_FILE {
                    let own = file_name(&i.path);
                    let _ = writeln!(s, "  - {} members: [{own}]({own})", members.len());
                    let mut t = format!("# {}\n\nSource: `{}`\n\n{}\n\n", i.path, c.root, line(i));
                    if !i.docs.is_empty() {
                        let _ = writeln!(t, "{}\n", i.docs);
                    }
                    for mm in &members {
                        let _ = writeln!(t, "{}", line(mm));
                    }
                    out.push((own, t));
                } else {
                    for mm in members {
                        let _ = writeln!(s, "  {}", line(mm));
                    }
                }
            }
            out.push((file_name(m), s));
        }
    }
    out.insert(0, ("index.md".into(), index));
    out
}
