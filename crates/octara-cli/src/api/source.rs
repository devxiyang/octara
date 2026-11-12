//! The public API read from the source with `syn` (stable Rust): the module tree, `pub use`
//! re-exports (globs included), every public item with its signature, docs and line, the members
//! of types and traits, inherent and trait impls, and the items the framework's own macros write
//! (`steps!`) and two outside ones (`bitflags!`, `new_key_type!`).

use super::expand::{self, Macro};
use super::{CrateApi, Item, Kind};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;

/// One crate to read: its name (`octara-kit`) and the directory holding its `Cargo.toml`.
#[derive(Clone, Debug)]
pub struct Source {
    pub name: String,
    pub dir: PathBuf,
    /// The library's root file (`src/lib.rs`).
    pub lib: PathBuf,
}

struct Module {
    path: Vec<String>,
    /// Relative to the crate's directory, `/` separated.
    file: String,
    items: Vec<syn::Item>,
    docs: String,
    parent: Option<usize>,
    children: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Target {
    /// An item: the module declaring it and its index there.
    Def(usize, usize),
    Module(usize),
    External(String),
}

struct Crate {
    name: String,
    modules: Vec<Module>,
    macros: HashMap<String, Macro>,
}

/// Reads `src` into the API of one crate.
pub fn read(src: &Source) -> Result<CrateApi, String> {
    let name = src.name.replace('-', "_");
    let mut c = Crate { name: name.clone(), modules: Vec::new(), macros: HashMap::new() };
    let file = parse(&src.lib)?;
    let docs = docs(&file.attrs);
    let dir = src.lib.parent().expect("a file's directory").to_path_buf();
    c.load(src, Vec::new(), &src.lib, dir, file.items, docs, None)?;
    c.collect_macros();
    for m in 0..c.modules.len() {
        let items = std::mem::take(&mut c.modules[m].items);
        c.modules[m].items = c.expand_items(items, 0);
    }
    Ok(c.convert(src))
}

fn parse(path: &Path) -> Result<syn::File, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    syn::parse_file(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn rel(src: &Source, path: &Path) -> String {
    path.strip_prefix(&src.dir).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

fn has_attr(attrs: &[syn::Attribute], name: &str) -> bool {
    attrs.iter().any(|a| a.path().is_ident(name))
}

/// `#[cfg(test)]`: not part of the API.
fn test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("cfg") && a.meta.to_token_stream_string().replace(' ', "").contains("(test)"))
}

fn hidden(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("doc") && a.meta.to_token_stream_string().replace(' ', "").contains("(hidden)"))
}

trait MetaText {
    fn to_token_stream_string(&self) -> String;
}

impl MetaText for syn::Meta {
    fn to_token_stream_string(&self) -> String {
        quote::ToTokens::to_token_stream(self).to_string()
    }
}

/// The docs `///` (or `#[doc = ..]`, a macro's `concat!` included) write, unindented by the one
/// space after `///`.
fn docs(attrs: &[syn::Attribute]) -> String {
    let mut lines = Vec::new();
    for a in attrs {
        if !a.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &a.meta
            && let Some(s) = expand::doc_value(&nv.value)
        {
            for l in s.lines() {
                lines.push(l.strip_prefix(' ').unwrap_or(l).to_string());
            }
        }
    }
    lines.join("\n").trim().to_string()
}

fn deprecated(attrs: &[syn::Attribute]) -> Option<String> {
    let a = attrs.iter().find(|a| a.path().is_ident("deprecated"))?;
    let mut note = String::new();
    if let syn::Meta::List(_) = &a.meta {
        let _ = a.parse_nested_meta(|m| {
            if m.path.is_ident("note") {
                let s: syn::LitStr = m.value()?.parse()?;
                note = s.value();
            }
            Ok(())
        });
    }
    Some(note)
}

fn is_pub(vis: &syn::Visibility) -> bool {
    matches!(vis, syn::Visibility::Public(_))
}

fn line(span: proc_macro2::Span) -> usize {
    span.start().line
}

impl Crate {
    #[allow(clippy::too_many_arguments)]
    fn load(
        &mut self,
        src: &Source,
        path: Vec<String>,
        file: &Path,
        dir: PathBuf,
        items: Vec<syn::Item>,
        docs: String,
        parent: Option<usize>,
    ) -> Result<usize, String> {
        let idx = self.modules.len();
        self.modules.push(Module { path: path.clone(), file: rel(src, file), items: Vec::new(), docs, parent, children: BTreeMap::new() });
        let mut kept = Vec::new();
        for item in items {
            if let syn::Item::Mod(m) = &item {
                if test_only(&m.attrs) {
                    continue;
                }
                let name = m.ident.to_string();
                let mut child_path = path.clone();
                child_path.push(name.clone());
                let child = match &m.content {
                    Some((_, inner)) => {
                        self.load(src, child_path, file, dir.join(&name), inner.clone(), self::docs(&m.attrs), Some(idx))?
                    }
                    None => {
                        let candidates = [dir.join(format!("{name}.rs")), dir.join(&name).join("mod.rs")];
                        let Some(found) = candidates.iter().find(|p| p.exists()) else { continue };
                        let parsed = parse(found)?;
                        let mut d = self::docs(&m.attrs);
                        let inner_docs = self::docs(&parsed.attrs);
                        if !inner_docs.is_empty() {
                            d = if d.is_empty() { inner_docs } else { format!("{d}\n{inner_docs}") };
                        }
                        self.load(src, child_path, found, dir.join(&name), parsed.items, d, Some(idx))?
                    }
                };
                self.modules[idx].children.insert(name, child);
            }
            kept.push(item);
        }
        self.modules[idx].items = kept;
        Ok(idx)
    }

    /// Every `macro_rules!` in the crate, by name (textual order is not followed: names are
    /// unique enough in one crate).
    fn collect_macros(&mut self) {
        for m in &self.modules {
            for item in &m.items {
                if let syn::Item::Macro(im) = item
                    && im.mac.path.is_ident("macro_rules")
                    && let Some(name) = &im.ident
                    && let Some(mac) = Macro::parse(im.mac.tokens.clone())
                {
                    self.macros.insert(name.to_string(), mac);
                }
            }
        }
    }

    fn macro_named(&self, mac: &syn::Macro) -> Option<Macro> {
        let name = mac.path.segments.last()?.ident.to_string();
        if name == "macro_rules" {
            return None;
        }
        if let Some(m) = self.macros.get(&name) {
            return Some(m.clone());
        }
        expand::builtin(&name)
    }

    /// Module items with macro invocations replaced by what they expand to (depth-limited).
    fn expand_items(&self, items: Vec<syn::Item>, depth: u32) -> Vec<syn::Item> {
        let mut out = Vec::new();
        for item in items {
            match item {
                syn::Item::Macro(im) if im.ident.is_none() && depth < 8 => {
                    match self.macro_named(&im.mac).and_then(|m| m.expand(im.mac.tokens.clone())) {
                        Some(ts) => match syn::parse2::<syn::File>(ts) {
                            Ok(f) => out.extend(self.expand_items(f.items, depth + 1)),
                            Err(_) => out.push(syn::Item::Macro(im)),
                        },
                        None => out.push(syn::Item::Macro(im)),
                    }
                }
                syn::Item::Trait(mut t) => {
                    t.items = self.expand_trait_items(std::mem::take(&mut t.items), depth);
                    out.push(syn::Item::Trait(t));
                }
                syn::Item::Impl(mut i) => {
                    i.items = self.expand_impl_items(std::mem::take(&mut i.items), depth);
                    out.push(syn::Item::Impl(i));
                }
                other => out.push(other),
            }
        }
        out
    }

    fn expand_trait_items(&self, items: Vec<syn::TraitItem>, depth: u32) -> Vec<syn::TraitItem> {
        let mut out = Vec::new();
        for item in items {
            match item {
                syn::TraitItem::Macro(tm) if depth < 8 => {
                    let expanded = self.macro_named(&tm.mac).and_then(|m| m.expand(tm.mac.tokens.clone())).and_then(|ts| {
                        let wrapped = quote::quote!(trait __Octara { #ts });
                        syn::parse2::<syn::ItemTrait>(wrapped).ok()
                    });
                    match expanded {
                        Some(t) => out.extend(self.expand_trait_items(t.items, depth + 1)),
                        None => out.push(syn::TraitItem::Macro(tm)),
                    }
                }
                other => out.push(other),
            }
        }
        out
    }

    fn expand_impl_items(&self, items: Vec<syn::ImplItem>, depth: u32) -> Vec<syn::ImplItem> {
        let mut out = Vec::new();
        for item in items {
            match item {
                syn::ImplItem::Macro(im) if depth < 8 => {
                    let expanded = self.macro_named(&im.mac).and_then(|m| m.expand(im.mac.tokens.clone())).and_then(|ts| {
                        let wrapped = quote::quote!(impl __Octara { #ts });
                        syn::parse2::<syn::ItemImpl>(wrapped).ok()
                    });
                    match expanded {
                        Some(i) => out.extend(self.expand_impl_items(i.items, depth + 1)),
                        None => out.push(syn::ImplItem::Macro(im)),
                    }
                }
                other => out.push(other),
            }
        }
        out
    }

    /// The names an item declares in its module, with the item's index.
    fn declared(&self, m: usize) -> Vec<(String, usize, bool)> {
        let mut out = Vec::new();
        for (i, item) in self.modules[m].items.iter().enumerate() {
            let (name, public, attrs) = match item {
                syn::Item::Fn(f) => (f.sig.ident.to_string(), is_pub(&f.vis), &f.attrs),
                syn::Item::Struct(s) => (s.ident.to_string(), is_pub(&s.vis), &s.attrs),
                syn::Item::Enum(e) => (e.ident.to_string(), is_pub(&e.vis), &e.attrs),
                syn::Item::Union(u) => (u.ident.to_string(), is_pub(&u.vis), &u.attrs),
                syn::Item::Trait(t) => (t.ident.to_string(), is_pub(&t.vis), &t.attrs),
                syn::Item::Const(c) => (c.ident.to_string(), is_pub(&c.vis), &c.attrs),
                syn::Item::Static(s) => (s.ident.to_string(), is_pub(&s.vis), &s.attrs),
                syn::Item::Type(t) => (t.ident.to_string(), is_pub(&t.vis), &t.attrs),
                syn::Item::Macro(im) if im.ident.is_some() => {
                    (im.ident.as_ref().expect("named").to_string(), has_attr(&im.attrs, "macro_export"), &im.attrs)
                }
                _ => continue,
            };
            if test_only(attrs) {
                continue;
            }
            out.push((name, i, public && !hidden(attrs)));
        }
        out
    }

    /// What `name` means in module `m` (any visibility): child modules, items, `use`s.
    fn scope(&self, m: usize, name: &str, depth: u32) -> Vec<Target> {
        let mut out = Vec::new();
        if let Some(c) = self.modules[m].children.get(name) {
            out.push(Target::Module(*c));
        }
        for (n, i, _) in self.declared(m) {
            if n == name {
                out.push(Target::Def(m, i));
            }
        }
        if out.is_empty() && depth < 12 {
            for item in &self.modules[m].items {
                if let syn::Item::Use(u) = item {
                    for (n, path, glob) in use_paths(&u.tree, Vec::new()) {
                        if glob {
                            if let Some(Target::Module(g)) = self.resolve(m, &path, depth + 1).into_iter().next() {
                                out.extend(self.scope(g, name, depth + 1));
                            }
                        } else if n == name {
                            out.extend(self.resolve(m, &path, depth + 1));
                        }
                    }
                }
            }
        }
        out
    }

    /// What a path written in module `m` names.
    fn resolve(&self, m: usize, segs: &[String], depth: u32) -> Vec<Target> {
        let Some((first, rest)) = segs.split_first() else { return Vec::new() };
        let mut cur: Vec<Target> = match first.as_str() {
            "crate" => vec![Target::Module(0)],
            "self" => vec![Target::Module(m)],
            "super" => vec![Target::Module(self.modules[m].parent.unwrap_or(0))],
            n if n == self.name => vec![Target::Module(0)],
            n => {
                let found = self.scope(m, n, depth);
                if found.is_empty() { vec![Target::External(n.to_string())] } else { found }
            }
        };
        for seg in rest {
            let mut next = Vec::new();
            for t in &cur {
                match t {
                    Target::Module(x) => match seg.as_str() {
                        "super" => next.push(Target::Module(self.modules[*x].parent.unwrap_or(0))),
                        "self" => next.push(Target::Module(*x)),
                        _ => next.extend(self.scope(*x, seg, depth + 1)),
                    },
                    Target::External(p) => next.push(Target::External(format!("{p}::{seg}"))),
                    Target::Def(..) => {}
                }
            }
            cur = next;
        }
        cur
    }

    /// What module `m` makes public: its `pub` items and modules, and its `pub use`s.
    fn exports(&self, m: usize, depth: u32) -> Vec<(String, Target)> {
        let mut out = Vec::new();
        for (name, i, public) in self.declared(m) {
            if public {
                out.push((name, Target::Def(m, i)));
            }
        }
        for item in &self.modules[m].items {
            match item {
                syn::Item::Mod(md) if is_pub(&md.vis) && !hidden(&md.attrs) => {
                    if let Some(c) = self.modules[m].children.get(&md.ident.to_string()) {
                        out.push((md.ident.to_string(), Target::Module(*c)));
                    }
                }
                syn::Item::Use(u) if is_pub(&u.vis) && !hidden(&u.attrs) && depth < 12 => {
                    for (name, path, glob) in use_paths(&u.tree, Vec::new()) {
                        if glob {
                            for t in self.resolve(m, &path, depth + 1) {
                                if let Target::Module(g) = t {
                                    out.extend(self.exports(g, depth + 1));
                                }
                            }
                        } else {
                            for t in self.resolve(m, &path, depth + 1) {
                                if !self.is_hidden(&t) {
                                    out.push((name.clone(), t));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// A `#[doc(hidden)]` item: not API, wherever it is re-exported.
    fn is_hidden(&self, t: &Target) -> bool {
        let Target::Def(m, i) = t else { return false };
        let attrs = match &self.modules[*m].items[*i] {
            syn::Item::Fn(f) => &f.attrs,
            syn::Item::Struct(s) => &s.attrs,
            syn::Item::Enum(e) => &e.attrs,
            syn::Item::Union(u) => &u.attrs,
            syn::Item::Trait(t) => &t.attrs,
            syn::Item::Const(c) => &c.attrs,
            syn::Item::Static(s) => &s.attrs,
            syn::Item::Type(t) => &t.attrs,
            syn::Item::Macro(m) => &m.attrs,
            _ => return false,
        };
        hidden(attrs)
    }

    fn convert(&self, src: &Source) -> CrateApi {
        // Every public path of every item and module.
        let mut paths: BTreeMap<Target, BTreeSet<String>> = BTreeMap::new();
        let mut reexports: BTreeMap<String, String> = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let mut queue = vec![(0usize, self.name.clone())];
        while let Some((m, prefix)) = queue.pop() {
            if !seen.insert((m, prefix.clone())) || prefix.matches("::").count() > 8 {
                continue;
            }
            for (name, t) in self.exports(m, 0) {
                let at = format!("{prefix}::{name}");
                match &t {
                    Target::Module(x) => {
                        paths.entry(t.clone()).or_default().insert(at.clone());
                        queue.push((*x, at));
                    }
                    Target::Def(..) => {
                        paths.entry(t).or_default().insert(at);
                    }
                    Target::External(p) => {
                        reexports.insert(at, p.clone());
                    }
                }
            }
        }
        let canon: BTreeMap<Target, (String, Vec<String>)> = paths.iter().map(|(t, ps)| (t.clone(), canonical(ps))).collect();
        let r = Render { canon: &canon, crate_: self };

        let mut items = Vec::new();
        let mut members: BTreeMap<Target, Vec<Item>> = BTreeMap::new();
        let mut traits: BTreeMap<Target, BTreeSet<String>> = BTreeMap::new();
        let mut implementors: BTreeMap<Target, BTreeSet<String>> = BTreeMap::new();

        // Impls anywhere in the crate, for the public types they name.
        for (m, module) in self.modules.iter().enumerate() {
            for item in &module.items {
                let syn::Item::Impl(imp) = item else { continue };
                if test_only(&imp.attrs) {
                    continue;
                }
                let generic_names: BTreeSet<String> = imp.generics.type_params().map(|p| p.ident.to_string()).collect();
                let self_path = type_path(&imp.self_ty);
                let blanket = self_path.as_ref().is_some_and(|p| p.len() == 1 && generic_names.contains(&p[0]));
                let self_t = self_path
                    .filter(|_| !blanket)
                    .and_then(|p| self.resolve(m, &p, 0).into_iter().find(|t| matches!(t, Target::Def(..)) && canon.contains_key(t)));
                let self_ty = r.ty(&imp.self_ty);
                match &imp.trait_ {
                    Some((tpath, _)) => {
                        let tname = r.path_text(tpath);
                        if let Some(t) = &self_t {
                            traits.entry(t.clone()).or_default().insert(tname.clone());
                        }
                        // `impl From<Spring> for Curve` is listed on Spring too: it converts into Curve.
                        if tpath.segments.last().is_some_and(|s| s.ident == "From")
                            && let Some(syn::PathArguments::AngleBracketed(a)) = tpath.segments.last().map(|s| &s.arguments)
                            && let Some(syn::GenericArgument::Type(from)) = a.args.first()
                            && let Some(fp) = type_path(from)
                            && let Some(ft) = self.resolve(m, &fp, 0).into_iter().find(|t| canon.contains_key(t))
                        {
                            traits.entry(ft).or_default().insert(format!("Into<{self_ty}>"));
                        }
                        let tsegs: Vec<String> = tpath.segments.iter().map(|s| s.ident.to_string()).collect();
                        if let Some(tt) = self.resolve(m, &tsegs, 0).into_iter().find(|t| canon.contains_key(t)) {
                            let g = r.generics(&imp.generics);
                            implementors.entry(tt).or_default().insert(if g.is_empty() {
                                self_ty.clone()
                            } else {
                                format!("impl{g} {self_ty}")
                            });
                        }
                    }
                    None => {
                        let Some(t) = &self_t else { continue };
                        let parent = &canon[t].0;
                        let within = if imp.generics.params.is_empty() && imp.generics.where_clause.is_none() {
                            String::new()
                        } else {
                            format!("impl{} {self_ty}{}", r.generics(&imp.generics), r.where_clause(&imp.generics))
                        };
                        for ii in &imp.items {
                            if let Some(member) = r.impl_item(ii, parent, &module.path) {
                                members.entry(t.clone()).or_default().push(Item { within: within.clone(), ..member });
                            }
                        }
                    }
                }
            }
        }

        for (t, (path, aliases)) in &canon {
            match t {
                Target::Module(x) => {
                    let md = &self.modules[*x];
                    let it = r.item(
                        path,
                        Kind::Module,
                        format!("pub mod {}", md.path.last().cloned().unwrap_or_default()),
                        &md.docs,
                        &md.file,
                        1,
                        None,
                    );
                    // A module's docs link from inside it.
                    let links = r.links(&md.docs, &md.path, Some(path));
                    items.push(Item { aliases: aliases.clone(), links, ..it });
                }
                Target::Def(m, i) => {
                    let item = &self.modules[*m].items[*i];
                    let Some((mut entry, own)) = r.def(item, path, &self.modules[*m]) else { continue };
                    entry.aliases = aliases.clone();
                    let mut derived: BTreeSet<String> = derives(item);
                    derived.extend(traits.remove(t).unwrap_or_default());
                    derived.retain(|d| !d.ends_with("StructuralPartialEq"));
                    entry.traits = derived.into_iter().collect();
                    entry.implementors = implementors.remove(t).unwrap_or_default().into_iter().collect();
                    items.push(entry);
                    items.extend(own);
                    items.extend(members.remove(t).unwrap_or_default());
                }
                Target::External(_) => {}
            }
        }
        for (at, to) in reexports {
            items.push(Item {
                target: to.clone(),
                ..r.item(&at, Kind::Reexport, format!("pub use {to}"), "", &self.modules[0].file, 1, None)
            });
        }
        items.sort_by(|a, b| a.path.cmp(&b.path).then(a.kind.cmp(&b.kind)));
        items.dedup_by(|a, b| a.path == b.path && a.kind == b.kind);
        CrateApi { name: src.name.clone(), root: src.dir.to_string_lossy().replace('\\', "/"), docs: self.modules[0].docs.clone(), items }
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

/// `use` trees flattened: (the name brought in, the path it names, whether a glob).
fn use_paths(tree: &syn::UseTree, prefix: Vec<String>) -> Vec<(String, Vec<String>, bool)> {
    let mut out = Vec::new();
    match tree {
        syn::UseTree::Path(p) => {
            let mut next = prefix;
            next.push(p.ident.to_string());
            out.extend(use_paths(&p.tree, next));
        }
        syn::UseTree::Name(n) => {
            let name = n.ident.to_string();
            let mut path = prefix.clone();
            if name == "self" {
                let last = prefix.last().cloned().unwrap_or_default();
                out.push((last, prefix, false));
            } else {
                path.push(name.clone());
                out.push((name, path, false));
            }
        }
        syn::UseTree::Rename(r) => {
            let mut path = prefix;
            path.push(r.ident.to_string());
            out.push((r.rename.to_string(), path, false));
        }
        syn::UseTree::Glob(_) => out.push((String::new(), prefix, true)),
        syn::UseTree::Group(g) => {
            for t in &g.items {
                out.extend(use_paths(t, prefix.clone()));
            }
        }
    }
    out
}

/// A type written as a path (`Signal<T>` → `Signal`), as segments.
fn type_path(ty: &syn::Type) -> Option<Vec<String>> {
    match ty {
        syn::Type::Path(p) if p.qself.is_none() => Some(p.path.segments.iter().map(|s| s.ident.to_string()).collect()),
        syn::Type::Group(g) => type_path(&g.elem),
        syn::Type::Paren(p) => type_path(&p.elem),
        _ => None,
    }
}

fn derives(item: &syn::Item) -> BTreeSet<String> {
    let attrs = match item {
        syn::Item::Struct(s) => &s.attrs,
        syn::Item::Enum(e) => &e.attrs,
        syn::Item::Union(u) => &u.attrs,
        _ => return BTreeSet::new(),
    };
    let mut out = BTreeSet::new();
    for a in attrs.iter().filter(|a| a.path().is_ident("derive")) {
        let _ = a.parse_nested_meta(|m| {
            if let Some(last) = m.path.segments.last() {
                out.insert(last.ident.to_string());
            }
            Ok(())
        });
    }
    out
}

/// Renders signatures with `prettyplease`, on one line.
struct Render<'a> {
    canon: &'a BTreeMap<Target, (String, Vec<String>)>,
    crate_: &'a Crate,
}

fn one_line(s: &str) -> String {
    let mut out = String::new();
    for w in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    let out = out.replace("( ", "(").replace(", )", ")").replace(" )", ")").replace("< ", "<").replace(", >", ">").replace(", {}", " {}");
    out.strip_suffix(',').map(str::to_owned).unwrap_or(out)
}

/// Writes paths in types by their last segment (`crate::platform::Platform` → `Platform`) and
/// drops binding modifiers (`mut check`), as rustdoc shows signatures.
struct Shorten;

impl syn::visit_mut::VisitMut for Shorten {
    fn visit_path_mut(&mut self, p: &mut syn::Path) {
        if p.segments.len() > 1 {
            let last = p.segments.last().cloned().expect("a segment");
            p.segments.clear();
            p.leading_colon = None;
            p.segments.push(last);
        }
        syn::visit_mut::visit_path_mut(self, p);
    }
    fn visit_type_path_mut(&mut self, t: &mut syn::TypePath) {
        // `<T as Trait>::Output` keeps its shape.
        if t.qself.is_none() {
            self.visit_path_mut(&mut t.path);
        }
    }
    fn visit_pat_ident_mut(&mut self, p: &mut syn::PatIdent) {
        p.mutability = None;
        p.by_ref = None;
    }
    fn visit_receiver_mut(&mut self, r: &mut syn::Receiver) {
        // `mut self` is the body's business (`&mut self` is in the kind, and stays).
        r.mutability = None;
        syn::visit_mut::visit_receiver_mut(self, r);
    }
}

fn unparse(mut item: syn::Item) -> String {
    syn::visit_mut::VisitMut::visit_item_mut(&mut Shorten, &mut item);
    one_line(&prettyplease::unparse(&syn::File { shebang: None, frontmatter: None, attrs: Vec::new(), items: vec![item] }))
}

/// An expression as written, when it is a literal worth showing (`= 1.5`), else nothing.
fn literal(e: &syn::Expr) -> Option<String> {
    match e {
        syn::Expr::Lit(l) => Some(quote::ToTokens::to_token_stream(&l.lit).to_string()),
        syn::Expr::Unary(u) if matches!(u.op, syn::UnOp::Neg(_)) => literal(&u.expr).map(|v| format!("-{v}")),
        syn::Expr::Group(g) => literal(&g.expr),
        _ => None,
    }
}

impl Render<'_> {
    fn ty(&self, t: &syn::Type) -> String {
        let s = unparse(syn::parse_quote!(type __T = #t;));
        s.strip_prefix("type __T = ").and_then(|s| s.strip_suffix(';')).unwrap_or(&s).to_string()
    }

    fn path_text(&self, p: &syn::Path) -> String {
        let t: syn::Type = syn::parse_quote!(#p);
        self.ty(&t)
    }

    fn generics(&self, g: &syn::Generics) -> String {
        if g.params.is_empty() {
            return String::new();
        }
        let params = &g.params;
        let s = unparse(syn::parse_quote!(struct __S<#params>;));
        s.strip_prefix("struct __S").and_then(|s| s.strip_suffix(';')).unwrap_or("").to_string()
    }

    fn where_clause(&self, g: &syn::Generics) -> String {
        match &g.where_clause {
            Some(w) if !w.predicates.is_empty() => {
                let s = unparse(syn::parse_quote!(struct __S #w;));
                s.strip_prefix("struct __S").and_then(|s| s.strip_suffix(';')).unwrap_or("").to_string()
            }
            _ => String::new(),
        }
    }

    fn sig(&self, vis: &syn::Visibility, sig: &syn::Signature) -> String {
        let f: syn::ItemFn = syn::parse_quote!(#vis #sig {});
        let s = unparse(syn::Item::Fn(f));
        s.strip_suffix("{}").map(str::trim_end).unwrap_or(&s).to_string()
    }

    #[allow(clippy::too_many_arguments)]
    fn item(&self, path: &str, kind: Kind, sig: String, docs: &str, file: &str, line: usize, dep: Option<String>) -> Item {
        Item {
            path: path.to_string(),
            kind,
            sig,
            within: String::new(),
            docs: docs.to_string(),
            file: format!("{file}:{line}"),
            aliases: Vec::new(),
            traits: Vec::new(),
            implementors: Vec::new(),
            links: Vec::new(),
            target: String::new(),
            deprecated: dep,
        }
    }

    /// An item with its links resolved in `module` (`Self` being the type a member is of).
    fn with_links(&self, mut it: Item, module: &[String]) -> Item {
        let parent = it.path.rsplit_once("::").map(|(p, _)| p.to_string());
        it.links = self.links(&it.docs, module, parent.as_deref());
        it
    }

    /// The public paths intra-doc links in `docs` name (`[`Signal`]`, `[text](Self::x)`).
    fn links(&self, docs: &str, module: &[String], parent: Option<&str>) -> Vec<String> {
        let m = self.crate_.modules.iter().position(|x| x.path == module).unwrap_or(0);
        let mut out = BTreeSet::new();
        let mut rest = docs;
        while let Some(open) = rest.find('[') {
            let after = &rest[open + 1..];
            let Some(close) = after.find(']') else { break };
            let text = &after[..close];
            let tail = &after[close + 1..];
            let target = match tail.strip_prefix('(').and_then(|t| t.find(')').map(|e| &t[..e])) {
                Some(t) if !t.contains("://") && !t.contains('.') && !t.contains('#') => t,
                Some(_) => "",
                None => text.trim_matches('`'),
            };
            let target = target.trim_end_matches("()");
            let resolved_self = target.strip_prefix("Self::").and_then(|rest| parent.map(|p| format!("{p}::{rest}")));
            if let Some(p) = resolved_self {
                out.insert(p);
                rest = &after[close + 1..];
                continue;
            }
            if !target.is_empty() && target.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ':') {
                let segs: Vec<String> = target.split("::").map(str::to_owned).collect();
                // The longest prefix that resolves; the rest are members.
                for k in (1..=segs.len()).rev() {
                    let found = self.crate_.resolve(m, &segs[..k], 0).into_iter().find_map(|t| self.canon.get(&t).map(|c| c.0.clone()));
                    if let Some(p) = found {
                        out.insert(std::iter::once(p).chain(segs[k..].iter().cloned()).collect::<Vec<_>>().join("::"));
                        break;
                    }
                }
            }
            rest = &after[close + 1..];
        }
        out.into_iter().collect()
    }

    /// A definition's entry and its own members (fields, variants, trait items).
    fn def(&self, item: &syn::Item, path: &str, module: &Module) -> Option<(Item, Vec<Item>)> {
        let file = &module.file;
        let mut own = Vec::new();
        let (kind, sig, attrs, line) = match item {
            syn::Item::Fn(f) => (Kind::Fn, self.sig(&f.vis, &f.sig), &f.attrs, self::line(f.sig.ident.span())),
            syn::Item::Struct(s) => {
                let mut c = s.clone();
                c.attrs.clear();
                let named = matches!(s.fields, syn::Fields::Named(_));
                let has = !s.fields.is_empty();
                if let syn::Fields::Named(f) = &mut c.fields {
                    f.named.clear();
                }
                if let syn::Fields::Unnamed(f) = &mut c.fields {
                    for field in f.unnamed.iter_mut() {
                        field.attrs.clear();
                        if !is_pub(&field.vis) {
                            field.ty = syn::parse_quote!(_);
                            field.vis = syn::Visibility::Inherited;
                        }
                    }
                }
                let mut sig = unparse(syn::Item::Struct(c));
                if named {
                    sig = sig.replace(" {}", if has { " { .. }" } else { " {}" });
                }
                if let syn::Fields::Named(f) = &s.fields {
                    for field in &f.named {
                        if !is_pub(&field.vis) || hidden(&field.attrs) {
                            continue;
                        }
                        let name = field.ident.as_ref().expect("named").to_string();
                        own.push(self.with_links(
                            self.item(
                                &format!("{path}::{name}"),
                                Kind::Field,
                                format!("pub {name}: {}", self.ty(&field.ty)),
                                &docs(&field.attrs),
                                file,
                                self::line(field.span()),
                                deprecated(&field.attrs),
                            ),
                            &module.path,
                        ));
                    }
                }
                (Kind::Struct, sig, &s.attrs, self::line(s.ident.span()))
            }
            syn::Item::Union(u) => {
                (Kind::Union, format!("pub union {}{}", u.ident, self.generics(&u.generics)), &u.attrs, self::line(u.ident.span()))
            }
            syn::Item::Enum(e) => {
                for v in &e.variants {
                    if hidden(&v.attrs) {
                        continue;
                    }
                    let name = v.ident.to_string();
                    let sig = match &v.fields {
                        syn::Fields::Unit => match &v.discriminant {
                            Some((_, d)) => format!("{name} = {}", quote::ToTokens::to_token_stream(d)),
                            None => name.clone(),
                        },
                        syn::Fields::Unnamed(f) => {
                            format!("{name}({})", f.unnamed.iter().map(|x| self.ty(&x.ty)).collect::<Vec<_>>().join(", "))
                        }
                        syn::Fields::Named(f) => format!(
                            "{name} {{ {} }}",
                            f.named
                                .iter()
                                .map(|x| format!("{}: {}", x.ident.as_ref().expect("named"), self.ty(&x.ty)))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    };
                    own.push(self.with_links(
                        self.item(
                            &format!("{path}::{name}"),
                            Kind::Variant,
                            sig,
                            &docs(&v.attrs),
                            file,
                            self::line(v.ident.span()),
                            deprecated(&v.attrs),
                        ),
                        &module.path,
                    ));
                }
                let vis = if is_pub(&e.vis) { "pub " } else { "" };
                (
                    Kind::Enum,
                    format!("{vis}enum {}{}{}", e.ident, self.generics(&e.generics), self.where_clause(&e.generics)),
                    &e.attrs,
                    self::line(e.ident.span()),
                )
            }
            syn::Item::Trait(t) => {
                let mut c = t.clone();
                c.attrs.clear();
                c.items.clear();
                let sig = unparse(syn::Item::Trait(c));
                let sig = sig.strip_suffix("{}").map(str::trim_end).unwrap_or(&sig).to_string();
                for ti in &t.items {
                    if let Some(member) = self.trait_item(ti, path, &module.path) {
                        own.push(member);
                    }
                }
                (Kind::Trait, sig, &t.attrs, self::line(t.ident.span()))
            }
            syn::Item::Const(c) => {
                (Kind::Const, format!("pub const {}: {}", c.ident, self.ty(&c.ty)), &c.attrs, self::line(c.ident.span()))
            }
            syn::Item::Static(s) => {
                let m = if matches!(s.mutability, syn::StaticMutability::Mut(_)) { "mut " } else { "" };
                (Kind::Static, format!("pub static {m}{}: {}", s.ident, self.ty(&s.ty)), &s.attrs, self::line(s.ident.span()))
            }
            syn::Item::Type(t) => (
                Kind::TypeAlias,
                format!("pub type {}{} = {}", t.ident, self.generics(&t.generics), self.ty(&t.ty)),
                &t.attrs,
                self::line(t.ident.span()),
            ),
            syn::Item::Macro(m) => {
                let name = m.ident.as_ref()?.to_string();
                (Kind::Macro, format!("macro_rules! {name}"), &m.attrs, self::line(m.ident.as_ref()?.span()))
            }
            _ => return None,
        };
        // A proc macro crate's attribute and function-like macros.
        let kind = if matches!(item, syn::Item::Fn(_))
            && ["proc_macro", "proc_macro_attribute", "proc_macro_derive"].iter().any(|a| has_attr(attrs, a))
        {
            Kind::Macro
        } else {
            kind
        };
        let entry = self.with_links(self.item(path, kind, sig, &docs(attrs), file, line, deprecated(attrs)), &module.path);
        Some((entry, own))
    }

    fn trait_item(&self, ti: &syn::TraitItem, parent: &str, module: &[String]) -> Option<Item> {
        let file = &self.crate_.modules.iter().find(|m| m.path == module).map(|m| m.file.clone()).unwrap_or_default();
        let it = match ti {
            syn::TraitItem::Fn(f) => {
                if hidden(&f.attrs) {
                    return None;
                }
                let kind = if f.sig.receiver().is_some() { Kind::Method } else { Kind::AssocFn };
                self.item(
                    &format!("{parent}::{}", f.sig.ident),
                    kind,
                    self.sig(&syn::Visibility::Inherited, &f.sig),
                    &docs(&f.attrs),
                    file,
                    line(f.sig.ident.span()),
                    deprecated(&f.attrs),
                )
            }
            syn::TraitItem::Const(c) => self.item(
                &format!("{parent}::{}", c.ident),
                Kind::AssocConst,
                format!("const {}: {}", c.ident, self.ty(&c.ty)),
                &docs(&c.attrs),
                file,
                line(c.ident.span()),
                None,
            ),
            syn::TraitItem::Type(t) => self.item(
                &format!("{parent}::{}", t.ident),
                Kind::AssocType,
                format!("type {}", t.ident),
                &docs(&t.attrs),
                file,
                line(t.ident.span()),
                None,
            ),
            _ => return None,
        };
        Some(self.with_links(it, module))
    }

    fn impl_item(&self, ii: &syn::ImplItem, parent: &str, module: &[String]) -> Option<Item> {
        let file = &self.crate_.modules.iter().find(|m| m.path == module).map(|m| m.file.clone()).unwrap_or_default();
        let it = match ii {
            syn::ImplItem::Fn(f) => {
                if !is_pub(&f.vis) || hidden(&f.attrs) || test_only(&f.attrs) {
                    return None;
                }
                let kind = if f.sig.receiver().is_some() { Kind::Method } else { Kind::AssocFn };
                self.item(
                    &format!("{parent}::{}", f.sig.ident),
                    kind,
                    self.sig(&f.vis, &f.sig),
                    &docs(&f.attrs),
                    file,
                    line(f.sig.ident.span()),
                    deprecated(&f.attrs),
                )
            }
            syn::ImplItem::Const(c) => {
                if !is_pub(&c.vis) || hidden(&c.attrs) {
                    return None;
                }
                let v = literal(&c.expr).map(|v| format!(" = {v}")).unwrap_or_default();
                self.item(
                    &format!("{parent}::{}", c.ident),
                    Kind::AssocConst,
                    format!("pub const {}: {}{v}", c.ident, self.ty(&c.ty)),
                    &docs(&c.attrs),
                    file,
                    line(c.ident.span()),
                    deprecated(&c.attrs),
                )
            }
            _ => return None,
        };
        Some(self.with_links(it, module))
    }
}
