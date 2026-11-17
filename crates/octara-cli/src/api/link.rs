//! Re-exports between the crates read together are aliases of their defining items. Keep the
//! definition in its own crate so signatures, documentation and source locations remain real.

use super::{Api, Kind};
use std::collections::{BTreeMap, BTreeSet};

struct Alias {
    path: String,
    glob: bool,
    export: Option<(usize, usize)>,
}

pub(super) fn reexports(api: &mut Api) {
    let mut aliases: BTreeMap<String, Vec<Alias>> = BTreeMap::new();
    for (c, krate) in api.crates.iter().enumerate() {
        for (i, item) in krate.items.iter().enumerate() {
            if item.kind == Kind::Reexport {
                let glob = item.target.ends_with("::*");
                let target = item.target.strip_suffix("::*").unwrap_or(&item.target);
                let path = item.path.strip_suffix("::*").unwrap_or(&item.path);
                aliases.entry(target.to_string()).or_default().push(Alias { path: path.to_string(), glob, export: Some((c, i)) });
            } else {
                for path in &item.aliases {
                    aliases.entry(item.path.clone()).or_default().push(Alias { path: path.clone(), glob: false, export: None });
                }
            }
        }
    }

    let mut resolved = BTreeSet::new();
    for item in api.crates.iter_mut().flat_map(|c| &mut c.items).filter(|i| i.kind != Kind::Reexport) {
        let mut paths = BTreeSet::from([item.path.clone()]);
        paths.extend(std::mem::take(&mut item.aliases));
        let mut queue: Vec<String> = paths.iter().cloned().collect();
        while let Some(path) = queue.pop() {
            // A module or type's aliases apply to its members too. Following each new alias
            // also handles chains through another crate or a renamed module.
            for end in std::iter::once(path.len()).chain(path.match_indices("::").map(|(at, _)| at)) {
                let Some(rules) = aliases.get(&path[..end]) else { continue };
                let suffix = &path[end..];
                for rule in rules {
                    if rule.glob && suffix.is_empty() {
                        continue;
                    }
                    if let Some(export) = rule.export {
                        resolved.insert(export);
                    }
                    let alias = format!("{}{suffix}", rule.path);
                    // Match the source reader's bound on recursive module re-exports.
                    if alias.matches("::").count() <= 8 && paths.insert(alias.clone()) {
                        queue.push(alias);
                    }
                }
            }
        }
        paths.remove(&item.path);
        item.aliases = paths.into_iter().collect();
    }

    for (c, krate) in api.crates.iter_mut().enumerate() {
        krate.items = std::mem::take(&mut krate.items)
            .into_iter()
            .enumerate()
            // Unknown third-party items stay re-exports. Their globs have no indexed items
            // to enumerate, just as when the source reader only knows one crate.
            .filter(|(i, item)| !resolved.contains(&(c, *i)) && !(item.kind == Kind::Reexport && item.target.ends_with("::*")))
            .map(|(_, item)| item)
            .collect();
    }
}
