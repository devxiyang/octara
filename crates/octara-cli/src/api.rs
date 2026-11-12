//! `api.json`: the framework's public API, generated from rustdoc's JSON (nightly) and checked in,
//! so the CLI, the catalog and agents read it without a nightly toolchain.

mod model;
mod render;

pub use model::{Api, CrateApi, FORMAT, Item, Kind};

use rustdoc_types::{Crate, Id, ItemEnum, StructKind, Type, VariantKind, Visibility};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The framework's public crates, in the order `api.json` lists them.
pub const CRATES: &[&str] = &["octara", "octara-kit", "octara-render", "octara-shell", "octara-fonts"];

/// The checked-in file, embedded: what this build of the CLI answers from.
pub const CHECKED_IN: &str = include_str!("../api.json");

/// Where `api.json` lives in the workspace.
pub fn checked_in_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("api.json")
}

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("workspace root")
}

/// The API this build was made with.
pub fn load() -> Api {
    serde_json::from_str(CHECKED_IN).expect("api.json is valid (the freshness test regenerates it)")
}

/// The API as `api.json` text: pretty, one field per line, a newline at the end.
pub fn to_json(api: &Api) -> String {
    serde_json::to_string_pretty(api).expect("serializes") + "\n"
}

/// Runs nightly rustdoc's JSON output over the public crates and converts it. The toolchain is
/// `nightly`, or `OCTARA_RUSTDOC_TOOLCHAIN` (CI pins a dated nightly, whose JSON format matches
/// `rustdoc-types`).
pub fn generate() -> Result<Api, String> {
    let root = workspace_root();
    let toolchain = std::env::var("OCTARA_RUSTDOC_TOOLCHAIN").unwrap_or_else(|_| "nightly".into());
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&root)
        .arg(format!("+{toolchain}"))
        .args(["doc", "--lib", "--no-deps", "--all-features", "--quiet"])
        // Nested under `cargo test` or `cargo run`, the outer toolchain's settings must not leak in.
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("RUSTC")
        .env_remove("RUSTDOC")
        .env_remove("CARGO")
        .env("RUSTDOCFLAGS", "-Z unstable-options --output-format json");
    for c in CRATES {
        cmd.args(["-p", c]);
    }
    let out = cmd.output().map_err(|e| format!("could not run cargo: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains(&format!("toolchain '{toolchain}")) || err.contains("is not installed") {
            return Err(format!(
                "generating api.json needs a nightly toolchain for rustdoc's JSON: `rustup toolchain install {toolchain}`"
            ));
        }
        return Err(format!("rustdoc failed:\n{err}"));
    }
    let doc = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target")).join("doc");
    let mut crates = Vec::new();
    for name in CRATES {
        let file = doc.join(format!("{}.json", name.replace('-', "_")));
        let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let format: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
        let found = format.get("format_version").and_then(|v| v.as_u64()).unwrap_or(0);
        if found != u64::from(rustdoc_types::FORMAT_VERSION) {
            return Err(format!(
                "nightly rustdoc writes JSON format {found}; octara-cli reads format {} — update `rustdoc-types` to the matching version",
                rustdoc_types::FORMAT_VERSION
            ));
        }
        let krate: Crate = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
        crates.push(convert(name, &krate, &root));
    }
    Ok(Api { format: FORMAT, crates })
}

/// What changed between two APIs, by path: (added, removed, changed).
pub fn diff(old: &Api, new: &Api) -> (Vec<String>, Vec<String>, Vec<String>) {
    let index = |a: &Api| a.items().map(|i| (i.path.clone(), i.clone())).collect::<BTreeMap<_, _>>();
    let (o, n) = (index(old), index(new));
    let added = n.keys().filter(|p| !o.contains_key(*p)).cloned().collect();
    let removed = o.keys().filter(|p| !n.contains_key(*p)).cloned().collect();
    let mut changed: Vec<String> = n.iter().filter(|(p, i)| o.get(*p).is_some_and(|x| x != *i)).map(|(p, _)| p.clone()).collect();
    // A crate's own docs are not an item; report them under the crate's name.
    for c in &new.crates {
        if old.crates.iter().any(|o| o.name == c.name && o.docs != c.docs) {
            changed.push(c.name.replace('-', "_"));
        }
    }
    (added, removed, changed)
}

struct Walk<'a> {
    krate: &'a Crate,
    /// Every public path each local item is reachable at.
    paths: HashMap<Id, BTreeSet<String>>,
    /// Re-exports of items or crates outside this crate: public path → target path.
    reexports: BTreeMap<String, (String, Id)>,
    seen_modules: BTreeSet<(Id, String)>,
}

impl Walk<'_> {
    fn module(&mut self, id: Id, prefix: &str) {
        if !self.seen_modules.insert((id, prefix.to_string())) {
            return;
        }
        let Some(ItemEnum::Module(m)) = self.krate.index.get(&id).map(|i| &i.inner) else { return };
        for child in &m.items {
            let Some(item) = self.krate.index.get(child) else { continue };
            match &item.inner {
                ItemEnum::Use(u) => {
                    let Some(target) = u.id else { continue };
                    match self.krate.index.get(&target) {
                        Some(t) if u.is_glob => {
                            if matches!(t.inner, ItemEnum::Module(_)) {
                                self.module(target, prefix);
                            }
                        }
                        Some(t) => {
                            let at = format!("{prefix}::{}", u.name);
                            self.paths.entry(target).or_default().insert(at.clone());
                            if matches!(t.inner, ItemEnum::Module(_)) {
                                self.module(target, &at);
                            }
                        }
                        None if !u.is_glob => {
                            let to = self.krate.paths.get(&target).map(|s| s.path.join("::")).unwrap_or_else(|| u.source.clone());
                            self.reexports.insert(format!("{prefix}::{}", u.name), (to, *child));
                        }
                        None => {}
                    }
                }
                _ => {
                    let Some(name) = &item.name else { continue };
                    let at = format!("{prefix}::{name}");
                    self.paths.entry(*child).or_default().insert(at.clone());
                    if matches!(item.inner, ItemEnum::Module(_)) {
                        self.module(*child, &at);
                    }
                }
            }
        }
    }
}

/// The canonical path: the shortest outside the prelude, then the first alphabetically.
fn canonical(paths: &BTreeSet<String>) -> (String, Vec<String>) {
    let best = paths
        .iter()
        .min_by_key(|p| (p.contains("::prelude::"), p.matches("::").count(), (*p).clone()))
        .cloned()
        .expect("at least one path");
    let aliases = paths.iter().filter(|p| **p != best).cloned().collect();
    (best, aliases)
}

fn convert(name: &str, krate: &Crate, root: &Path) -> CrateApi {
    let crate_name = name.replace('-', "_");
    let mut walk = Walk { krate, paths: HashMap::new(), reexports: BTreeMap::new(), seen_modules: BTreeSet::new() };
    walk.module(krate.root, &crate_name);
    let canon: HashMap<Id, (String, Vec<String>)> = walk.paths.iter().map(|(id, ps)| (*id, canonical(ps))).collect();
    let path_of =
        |id: &Id| -> Option<String> { canon.get(id).map(|(p, _)| p.clone()).or_else(|| krate.paths.get(id).map(|s| s.path.join("::"))) };
    let file_of = |item: &rustdoc_types::Item| -> String {
        item.span
            .as_ref()
            .map(|s| {
                let f = &s.filename;
                // With `/` on every platform, as everything Octara reports.
                f.strip_prefix(root).unwrap_or(f).to_string_lossy().replace('\\', "/")
            })
            .unwrap_or_default()
    };
    let links_of = |item: &rustdoc_types::Item| -> Vec<String> {
        let set: BTreeSet<String> = item.links.values().filter_map(&path_of).collect();
        set.into_iter().collect()
    };
    let base = |item: &rustdoc_types::Item, path: String, kind: Kind, sig: String| Item {
        path,
        kind,
        sig,
        within: String::new(),
        docs: item.docs.clone().unwrap_or_default(),
        file: file_of(item),
        aliases: Vec::new(),
        traits: Vec::new(),
        implementors: Vec::new(),
        links: links_of(item),
        target: String::new(),
        deprecated: item.deprecation.as_ref().map(|d| d.note.clone().or_else(|| d.since.clone()).unwrap_or_default()),
    };

    let mut items = Vec::new();
    let mut ids: Vec<&Id> = canon.keys().collect();
    ids.sort_by(|a, b| canon[*a].0.cmp(&canon[*b].0));
    for id in ids {
        let item = &krate.index[id];
        let (path, aliases) = canon[id].clone();
        let short = item.name.clone().unwrap_or_default();
        let vis = if item.visibility == Visibility::Public { "pub " } else { "" };
        let (kind, sig) = match &item.inner {
            ItemEnum::Module(_) => (Kind::Module, format!("{vis}mod {short}")),
            ItemEnum::Struct(s) => {
                let g = render::generics(&s.generics);
                let w = render::where_clause(&s.generics);
                let sig = match &s.kind {
                    StructKind::Unit => format!("{vis}struct {short}{g}{w};"),
                    StructKind::Tuple(fields) => {
                        let fs: Vec<String> = fields
                            .iter()
                            .map(|f| match f.and_then(|f| krate.index.get(&f)) {
                                Some(fi) => match &fi.inner {
                                    ItemEnum::StructField(t) => {
                                        format!("{}{}", if fi.visibility == Visibility::Public { "pub " } else { "" }, render::ty(t))
                                    }
                                    _ => "_".into(),
                                },
                                None => "_".into(),
                            })
                            .collect();
                        format!("{vis}struct {short}{g}({}){w};", fs.join(", "))
                    }
                    StructKind::Plain { fields, has_stripped_fields } => {
                        // Public fields are items of their own.
                        let body = if fields.is_empty() && !has_stripped_fields { " {}" } else { " { .. }" };
                        format!("{vis}struct {short}{g}{w}{body}")
                    }
                };
                (Kind::Struct, sig)
            }
            ItemEnum::Union(u) => (Kind::Union, format!("{vis}union {short}{}", render::generics(&u.generics))),
            ItemEnum::Enum(e) => {
                (Kind::Enum, format!("{vis}enum {short}{}{}", render::generics(&e.generics), render::where_clause(&e.generics)))
            }
            ItemEnum::Trait(t) => {
                let b = if t.bounds.is_empty() { String::new() } else { format!(": {}", render::bounds(&t.bounds)) };
                let u = if t.is_unsafe { "unsafe " } else { "" };
                (Kind::Trait, format!("{vis}{u}trait {short}{}{b}{}", render::generics(&t.generics), render::where_clause(&t.generics)))
            }
            ItemEnum::Function(f) => (Kind::Fn, format!("{vis}{}", render::function(&short, f))),
            ItemEnum::Constant { type_, .. } => (Kind::Const, format!("{vis}const {short}: {}", render::ty(type_))),
            ItemEnum::Static(s) => {
                (Kind::Static, format!("{vis}static {}{short}: {}", if s.is_mutable { "mut " } else { "" }, render::ty(&s.type_)))
            }
            ItemEnum::TypeAlias(t) => {
                (Kind::TypeAlias, format!("{vis}type {short}{} = {}", render::generics(&t.generics), render::ty(&t.type_)))
            }
            ItemEnum::Macro(_) => (Kind::Macro, format!("macro_rules! {short}")),
            ItemEnum::ProcMacro(_) => (Kind::Macro, short.clone()),
            _ => continue,
        };
        let mut entry = base(item, path.clone(), kind, sig);
        entry.aliases = aliases;

        // Members: variants, named fields, inherent methods and constants, trait items.
        let mut members = Vec::new();
        let mut trait_impls = BTreeSet::new();
        let impls: &[Id] = match &item.inner {
            ItemEnum::Struct(s) => &s.impls,
            ItemEnum::Enum(e) => &e.impls,
            ItemEnum::Union(u) => &u.impls,
            _ => &[],
        };
        match &item.inner {
            ItemEnum::Enum(e) => {
                for v in &e.variants {
                    let vi = &krate.index[v];
                    let vname = vi.name.clone().unwrap_or_default();
                    let ItemEnum::Variant(var) = &vi.inner else { continue };
                    let field_ty = |f: &Id| match &krate.index[f].inner {
                        ItemEnum::StructField(t) => render::ty(t),
                        _ => "_".into(),
                    };
                    let sig = match &var.kind {
                        VariantKind::Plain => match &var.discriminant {
                            Some(d) => format!("{vname} = {}", d.expr),
                            None => vname.clone(),
                        },
                        VariantKind::Tuple(fs) => format!(
                            "{vname}({})",
                            fs.iter().map(|f| f.as_ref().map(field_ty).unwrap_or_else(|| "_".into())).collect::<Vec<_>>().join(", ")
                        ),
                        VariantKind::Struct { fields, has_stripped_fields } => {
                            let mut fs: Vec<String> = fields
                                .iter()
                                .map(|f| format!("{}: {}", krate.index[f].name.clone().unwrap_or_default(), field_ty(f)))
                                .collect();
                            if *has_stripped_fields {
                                fs.push("..".into());
                            }
                            format!("{vname} {{ {} }}", fs.join(", "))
                        }
                    };
                    members.push(base(vi, format!("{path}::{vname}"), Kind::Variant, sig));
                }
            }
            ItemEnum::Struct(s) => {
                if let StructKind::Plain { fields, .. } = &s.kind {
                    for f in fields {
                        let fi = &krate.index[f];
                        let ItemEnum::StructField(t) = &fi.inner else { continue };
                        if fi.visibility != Visibility::Public {
                            continue;
                        }
                        let fname = fi.name.clone().unwrap_or_default();
                        members.push(base(fi, format!("{path}::{fname}"), Kind::Field, format!("pub {fname}: {}", render::ty(t))));
                    }
                }
            }
            ItemEnum::Trait(t) => {
                for m in &t.items {
                    let mi = &krate.index[m];
                    if let Some(member) = assoc(mi, &path, "", &base) {
                        members.push(member);
                    }
                }
                let mut implementors = BTreeSet::new();
                for i in &t.implementations {
                    if let Some(ItemEnum::Impl(imp)) = krate.index.get(i).map(|i| &i.inner)
                        && imp.blanket_impl.is_none()
                    {
                        let g = render::generics(&imp.generics);
                        implementors.insert(format!(
                            "{}{}",
                            if g.is_empty() { String::new() } else { format!("impl{g} ") },
                            render::ty(&imp.for_)
                        ));
                    }
                }
                entry.implementors = implementors.into_iter().collect();
            }
            _ => {}
        }
        for i in impls {
            let Some(ItemEnum::Impl(imp)) = krate.index.get(i).map(|i| &i.inner) else { continue };
            if imp.is_synthetic || imp.blanket_impl.is_some() || imp.is_negative {
                continue;
            }
            if let Some(tr) = &imp.trait_ {
                // Marker traits the compiler adds for derives say nothing to a caller.
                if tr.path.ends_with("StructuralPartialEq") {
                    continue;
                }
                let for_self = matches!(&imp.for_, Type::ResolvedPath(p) if p.id == *id);
                if for_self {
                    trait_impls.insert(render::path(tr));
                } else if tr.path.rsplit("::").next() == Some("From") {
                    // `impl From<Spring> for Curve` is listed on Spring too: it converts into Curve.
                    trait_impls.insert(format!("Into<{}>", render::ty(&imp.for_)));
                }
                continue;
            }
            let within = if imp.generics.params.is_empty() && imp.generics.where_predicates.is_empty() {
                String::new()
            } else {
                format!("impl{} {}{}", render::generics(&imp.generics), render::ty(&imp.for_), render::where_clause(&imp.generics))
            };
            for m in &imp.items {
                let mi = &krate.index[m];
                if mi.visibility != Visibility::Public {
                    continue;
                }
                if let Some(member) = assoc(mi, &path, "pub ", &base) {
                    members.push(Item { within: within.clone(), ..member });
                }
            }
        }
        entry.traits = trait_impls.into_iter().collect();
        items.push(entry);
        items.extend(members);
    }
    for (at, (to, use_id)) in &walk.reexports {
        let u = &krate.index[use_id];
        items.push(Item { target: to.clone(), aliases: Vec::new(), ..base(u, at.clone(), Kind::Reexport, format!("pub use {to}")) });
    }
    items.sort_by(|a, b| a.path.cmp(&b.path).then(a.kind.cmp(&b.kind)));
    items.dedup_by(|a, b| a.path == b.path && a.kind == b.kind);
    let docs = krate.index[&krate.root].docs.clone().unwrap_or_default();
    CrateApi { name: name.to_string(), docs, items }
}

/// A trait's or an impl block's method, associated function, constant or type.
fn assoc(
    mi: &rustdoc_types::Item,
    parent: &str,
    vis: &str,
    base: &impl Fn(&rustdoc_types::Item, String, Kind, String) -> Item,
) -> Option<Item> {
    let name = mi.name.clone()?;
    let path = format!("{parent}::{name}");
    Some(match &mi.inner {
        ItemEnum::Function(f) => {
            let kind = if render::has_receiver(f) { Kind::Method } else { Kind::AssocFn };
            base(mi, path, kind, format!("{vis}{}", render::function(&name, f)))
        }
        ItemEnum::AssocConst { type_, value, .. } => {
            let v = value.as_ref().filter(|v| v.len() <= 60 && *v != "_").map(|v| format!(" = {v}")).unwrap_or_default();
            base(mi, path, Kind::AssocConst, format!("{vis}const {name}: {}{v}", render::ty(type_)))
        }
        ItemEnum::AssocType { bounds, type_, generics, .. } => {
            let b = if bounds.is_empty() { String::new() } else { format!(": {}", render::bounds(bounds)) };
            let t = type_.as_ref().map(|t| format!(" = {}", render::ty(t))).unwrap_or_default();
            base(mi, path, Kind::AssocType, format!("type {name}{}{b}{t}", render::generics(generics)))
        }
        _ => return None,
    })
}
