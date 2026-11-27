//! The API read from the source: canonical paths and aliases, signatures, members written by
//! macros, impl blocks' bounds, trait impls, re-exports, and where each item is.

use octara_cli::api::{self, Kind};
use octara_cli::catalog::Catalog;

#[test]
fn prelude_reexports_capabilities_and_handles() {
    let api = api::load();
    let catalog = Catalog::build(api);
    for name in [
        "AppDefinition",
        "BuildCx",
        "ReadCx",
        "WriteCx",
        "CleanupCx",
        "TaskCx",
        "Signal",
        "ReadSignal",
        "Binding",
        "Field",
        "Readable",
        "RowSignal",
        "Store",
        "Trigger",
        "Writable",
        "StateList",
        "ReadList",
        "TextBuffer",
        "Window",
        "El",
        "Styled",
        "Role",
        "input",
        "textarea",
        "each",
    ] {
        let alias = format!("octara::prelude::{name}");
        assert!(api.get(&alias).is_some(), "{alias}");
        assert!(catalog.prelude().any(|entry| entry.aliases.contains(&alias)), "{alias}");
    }
    for name in ["AgentRequest", "Tree", "HostEngine", "NativeEngine"] {
        assert!(api.get(&format!("octara::prelude::{name}")).is_none(), "{name}");
        assert!(!catalog.lookup(name).matches.is_empty(), "{name}");
    }
}

#[test]
fn items_resolve_capability_methods_and_host_types() {
    let api = api::load();
    let build = api.get("octara::prelude::BuildCx::signal").unwrap();
    assert_eq!(build.kind, Kind::Method);
    assert!(build.sig.contains("Signal<T>"), "{}", build.sig);
    assert!(api.get("octara::prelude::BuildCx::set").is_none());
    let set = api.get("octara::prelude::WriteCx::set").unwrap();
    assert!(set.sig.contains("Writable<T>"), "{}", set.sig);
    let get = api.get("octara::prelude::ReadCx::get").unwrap();
    assert!(get.sig.contains("Readable<T>"), "{}", get.sig);
    for (context, method) in [
        ("BuildCx", "store"),
        ("BuildCx", "memo_with"),
        ("BuildCx", "trigger"),
        ("ReadCx", "track"),
        ("WriteCx", "mutate"),
        ("WriteCx", "notify"),
    ] {
        let path = format!("octara::prelude::{context}::{method}");
        assert_eq!(api.get(&path).unwrap_or_else(|| panic!("{path}")).kind, Kind::Method);
    }
    assert!(api.get("octara::prelude::BuildCx::slice").is_none());
    assert!(api.get("octara::prelude::Binding::get").is_none());
    assert!(api.get("octara::prelude::ReadCx::set").is_none());
    assert!(api.get("octara::prelude::State::HOVER").is_some());
    assert!(api.get("octara::prelude::State::FOCUS_WITHIN").is_some());
    assert!(api.get("octara_tree::NodeFlags::POINTER_EVENTS_NONE").is_some());
    assert_eq!(api.get("octara_tree::NodeId").unwrap().path, "octara_model::identity::NodeId");
    assert_eq!(api.get("octara_inspect::AgentRequest"), api.get("octara_inspect::protocol::AgentRequest"));
    assert!(api.get("octara::diagnostics::codes::STALE_TARGET").is_some());
    let p = api.get("octara::view::Styled::p").unwrap();
    assert_eq!((p.kind, p.sig.as_str()), (Kind::Method, "fn p(self, length: impl Into<Length>) -> Self"));
}

#[test]
fn items_say_where_they_are() {
    let api = api::load();
    for c in &api.crates {
        assert!(c.items.windows(2).all(|w| (&w[0].path, w[0].kind) < (&w[1].path, w[1].kind)), "{} items are sorted and unique", c.name);
        assert!(std::path::Path::new(&c.root).join("Cargo.toml").exists(), "{}: root is the crate's directory", c.name);
    }
    for (path, owner, file, declaration) in [
        ("octara::view::text", "octara", "src/view/el.rs", "pub fn text("),
        ("octara::prelude::Theme", "octara-model", "src/theme.rs", "pub struct Theme"),
    ] {
        let item = api.get(path).unwrap();
        let (source, line) = item.file.rsplit_once(':').unwrap();
        assert_eq!(source, file);
        let code = std::fs::read_to_string(api::workspace_root().join("crates").join(owner).join(source)).unwrap();
        let at = code.lines().nth(line.parse::<usize>().unwrap() - 1).unwrap();
        assert!(at.contains(declaration), "{path}: {at}");
    }
}

#[test]
fn reexports_between_crates_resolve_types_modules_globs_and_members() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("api-cross-crate-exports");
    let sources: Vec<_> = [
        (
            "octara-test-model",
            r#"
pub mod shapes {
    /// A rectangle with a public width.
    pub struct Rect {
        /// Its width in logical pixels.
        pub width: f32,
    }
    impl Rect {
        /// Build a rectangle.
        pub fn new(width: f32) -> Self { Self { width } }
        /// Read the width.
        pub fn width(&self) -> f32 { self.width }
    }
}
pub mod edges {
    /// A rectangle edge.
    pub enum Edge { Top, Bottom }
}
"#,
        ),
        ("octara-bridge", "pub use octara_test_model::shapes as geometry;\npub use octara_test_model::shapes::Rect as Rectangle;"),
        (
            "octara",
            r#"
pub mod prelude {
    pub use octara_bridge::geometry::*;
    pub use octara_test_model::edges::*;
    pub use octara_bridge::Rectangle as Box;
}
pub use octara_bridge::geometry;
pub use outside::Thing;
"#,
        ),
    ]
    .into_iter()
    .map(|(name, text)| {
        let dir = dir.join(name);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), format!("[package]\nname = \"{name}\"\nversion = \"0.0.0\"\n")).unwrap();
        let lib = dir.join("src/lib.rs");
        std::fs::write(&lib, text).unwrap();
        api::Source { name: name.into(), dir, lib }
    })
    .collect();
    let api = api::read(&sources).unwrap();
    let rect = api.get("octara::prelude::Box").unwrap();
    assert_eq!(rect.path, "octara_test_model::shapes::Rect");
    assert_eq!(rect.docs, "A rectangle with a public width.");
    assert_eq!(rect.kind, Kind::Struct);
    assert!(rect.target.is_empty());
    assert_eq!(api.get("octara::geometry::Rect").unwrap(), rect);
    assert_eq!(api.get("octara::prelude::Rect").unwrap(), rect);
    assert_eq!(api.get("octara::prelude::Edge::Top").unwrap().path, "octara_test_model::edges::Edge::Top");
    let constructor = api.get("octara::prelude::Box::new").unwrap();
    assert_eq!(constructor.path, "octara_test_model::shapes::Rect::new");
    assert_eq!(constructor.docs, "Build a rectangle.");
    assert_eq!(constructor.kind, Kind::AssocFn);
    assert_eq!(constructor.sig, "pub fn new(width: f32) -> Self");
    let widths: Vec<_> = api.all("octara::prelude::Box::width").collect();
    assert_eq!(widths.len(), 2, "a field and method retain their shared alias");
    assert_eq!(api.get("octara::Thing").unwrap().target, "outside::Thing", "unindexed dependencies remain explicit re-exports");
    assert!(api.items().all(|i| !i.path.ends_with("::*")));

    let catalog = octara_cli::catalog::Catalog::build(&api);
    let found = catalog.lookup("octara::prelude::Box::new");
    let entry = found.matches[0];
    assert_eq!(entry.path, constructor.path);
    assert_eq!(entry.docs, constructor.docs);
    let source = sources[0].lib.display().to_string().replace('\\', "/");
    assert!(entry.file.starts_with(&format!("{source}:")), "{}", entry.file);
    assert!(catalog.prelude().all(|entry| entry.path != constructor.path), "member aliases do not become prelude imports");
    let (_, index) = octara_cli::reference::files(&api).into_iter().find(|(path, _)| path == "index.md").unwrap();
    assert!(index.contains("Prelude (`use octara::prelude::*`): Box, Edge, Rect"), "{index}");
    assert!(!index.contains("use octara_test_model::prelude::*"), "the facade's prelude belongs to its own crate");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_read_is_cached_until_the_source_changes() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let sources = api::workspace_sources();
    let a = api::cached(&sources, dir.path()).unwrap();
    assert!(dir.path().join("octara/api.json").exists());
    let b = api::cached(&sources, dir.path()).unwrap();
    assert_eq!(a, b);
}

#[test]
fn api_catalog_matches_checkout() {
    let mut expected = api::load().clone();
    let root = api::workspace_root();
    for (krate, source) in expected.crates.iter_mut().zip(api::workspace_sources()) {
        krate.root = source.dir.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
    }
    let catalog: api::Api = if std::env::var_os("OCTARA_VERIFY_RELEASE_API").is_some() {
        api::bundled().unwrap()
    } else {
        let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let path = dir.path().join("api.json.gz");
        api::export_bundle(&path).unwrap();
        let archive = flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap());
        serde_json::from_reader(archive).unwrap()
    };
    for module in ["octara-webview", "octara-video", "octara-picker", "octara-haptics"] {
        let krate = catalog.crates.iter().find(|krate| krate.name == module).expect("official module in the catalog");
        assert_eq!(krate.root, format!("modules/{module}"));
    }
    assert_eq!(catalog.format, expected.format);
    assert_eq!(catalog.crates.len(), expected.crates.len());
    for (bundled, expected) in catalog.crates.iter().zip(expected.crates) {
        assert!(
            *bundled == expected,
            "{} changed. Regenerate with `cargo run -p octara-cli -- export-api crates/octara-cli/skill/api.json.gz`",
            expected.name,
        );
    }
}

#[test]
fn project_lookup_prefers_its_direct_octara_over_a_transitive_version() {
    let temp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let fixture = |dir: &str, name: &str, version: &str, manifest: &str, code: &str| {
        let root = temp.path().join(dir);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2024\"\n{manifest}"),
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), code).unwrap();
        root
    };
    fixture("older", "octara", "0.1.0", "", "pub fn previous() {}");
    fixture("current", "octara", "0.2.0", "", "pub fn current() {}");
    fixture("adapter", "adapter", "0.0.0", "[dependencies]\noctara = { path = \"../older\" }\n", "");
    let project = fixture(
        "app",
        "app",
        "0.0.0",
        "[workspace]\n[dependencies]\nframework = { package = \"octara\", path = \"../current\" }\nadapter = { path = \"../adapter\" }\n",
        "",
    );
    let (sources, _) = api::project_sources(&project).unwrap();
    let api = api::read(&sources).unwrap();
    assert!(api.get("octara::current").is_some());
    assert!(api.get("octara::previous").is_none());
}
