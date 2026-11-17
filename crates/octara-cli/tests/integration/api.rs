//! The API read from the source: canonical paths and aliases, signatures, members written by
//! macros, impl blocks' bounds, trait impls, re-exports, and where each item is.

use octara_cli::api::{self, Kind};
use octara_cli::catalog::Catalog;

#[test]
fn specialized_drawing_native_and_json_apis_require_explicit_imports() {
    let api = api::load();
    let catalog = Catalog::build(api);
    for (name, owner) in [
        ("Canvas", "octara_primitives::canvas::Canvas"),
        ("FillRule", "octara_primitives::path::FillRule"),
        ("LineCap", "octara_primitives::path::LineCap"),
        ("LineJoin", "octara_primitives::path::LineJoin"),
        ("Path", "octara_primitives::path::Path"),
        ("Stroke", "octara_primitives::path::Stroke"),
        ("Transform", "octara_primitives::path::Transform"),
        ("point", "octara_primitives::path::point"),
        ("NativeEvent", "octara_primitives::native::NativeEvent"),
        ("Slot", "octara_primitives::native::Slot"),
        ("canvas", "octara::view::canvas"),
        ("native_view", "octara::view::native_view"),
        ("web_view", "octara::view::web_view"),
    ] {
        assert!(api.get(&format!("octara::prelude::{name}")).is_none(), "{name} is no longer a prelude alias");
        assert!(api.get(owner).is_some(), "{owner} remains public");
        assert!(!catalog.get(owner).unwrap().prelude, "vocab requires an explicit import for {owner}");
        assert!(catalog.lookup(name).matches.iter().any(|entry| entry.path == owner), "vocab still finds {owner}");
    }
    assert!(api.get("octara::prelude::json").is_none());
    assert_eq!(octara::serde_json::json!({"explicit": true})["explicit"], true);
    for name in [
        "Rect",
        "Signal",
        "ReadSignal",
        "Watch",
        "state",
        "memo",
        "watch",
        "StateList",
        "ReadList",
        "state_list",
        "TextBuffer",
        "text_buffer",
    ] {
        assert!(api.get(&format!("octara::prelude::{name}")).is_some(), "the common prelude keeps {name}");
    }
}

#[test]
fn items_are_found_by_canonical_path_and_alias() {
    let api = api::load();
    let state = api.get("octara::prelude::state").expect("the prelude alias resolves");
    assert_eq!(state.path, "octara::reactive::state");
    assert_eq!(state.kind, Kind::Fn);
    assert!(state.sig.starts_with("pub fn state<T: 'static>(key: &'static str"), "{}", state.sig);
    let p = api.get("octara::view::Styled::p_4").expect("methods a macro writes are items");
    assert_eq!((p.kind, p.sig.as_str()), (Kind::Method, "fn p_4(self) -> Self"));
    assert!(p.docs.contains("spacing steps"), "their docs too: {}", p.docs);
    let set = api.get("octara::reactive::Signal::set").unwrap();
    assert_eq!(set.within, "impl<T: 'static> Signal<T>", "writes do not require Clone");
    let get = api.get("octara::reactive::Signal::get").unwrap();
    assert!(get.sig.contains("T: Clone"), "owned reads retain their method-local bound: {}", get.sig);
    let spring = api.get("octara::prelude::Spring").unwrap();
    assert_eq!(spring.path, "octara_primitives::motion::Spring");
    assert_eq!(spring.kind, Kind::Struct);
    assert!(spring.traits.iter().any(|t| t == "Into<Curve>"), "{:?}", spring.traits);
    assert_eq!(api.get("octara::prelude::Spring::SNAP").unwrap().path, "octara_primitives::motion::Spring::SNAP");
    let el = api.get("octara::view::El").unwrap();
    assert!(el.traits.iter().any(|t| t == "Styled"));
    assert_eq!(api.get("octara::kernel::Role").unwrap().target, "accesskit::Role", "re-exports name their target");
    assert!(api.get("octara::kernel::State::HOVER").is_some(), "bitflags' flags are items");
    assert!(api.get("octara_primitives::identity::NodeId").is_some(), "new_key_type's keys are items");
    assert_eq!(api.get("octara_text::TextEngine").unwrap().kind, Kind::Trait);
    assert!(api.get("octara_text::ParleyEngine::system").is_some());
    assert!(api.get("octara::kernel::text::TextEngine").is_none(), "text layout has one public owner");
    assert!(api.get("octara::kernel::NodeId").is_none(), "primitive types are owned by their crate");
    assert!(api.get("octara::motion::Spring").is_none(), "removed module paths are not aliases");
    assert!(api.get("octara::diagnostics::OVERFLOW").is_some(), "codes! writes the registry's statics");
    assert!(api.get("octara::view::enter_component").is_none(), "#[doc(hidden)] is not API");
    assert!(api.get("octara::kernel::Node").is_none(), "private items are not API");
    assert!(api.items().all(|item| !item.path.contains("::reactive::graph") && !item.path.contains("::reactive::seq")));
    assert!(api.get("octara::reactive::graph::state").is_none(), "old implementation paths are not aliases");
    assert!(api.get("octara::prelude::Memo").is_none());
    assert!(api.get("octara::reactive::Signal::new").is_none());
    for method in ["before_input", "before_key", "before_blur"] {
        assert!(api.get(&format!("octara::view::El::{method}")).is_none());
    }
    assert!(api.get("octara::view::El::behavior").is_some());
    for method in ["on_input", "on_key", "on_blur", "on_submit", "on_toggle", "on_numeric_input", "on_press", "on_drag"] {
        assert!(api.get(&format!("octara::view::Behavior::{method}")).is_some());
    }
    assert!(api.get("octara::reactive::list::StateList::all").is_none());
    assert!(api.get("octara::reactive::list::Source").is_none());
    assert!(api.get("octara::prelude::Source").is_none());
    assert_eq!(api.get("octara::prelude::ReadList").unwrap().path, "octara::reactive::list::ReadList");
    for ty in ["StateList", "Groups"] {
        assert!(api.get(&format!("octara::reactive::list::{ty}::source")).is_none());
        assert!(api.get(&format!("octara::reactive::list::{ty}::read_only")).unwrap().sig.contains("ReadList<"));
    }
    assert!(api.get("octara::reactive::list::Derived").is_none());
    assert!(api.get("octara::prelude::Derived").is_none());
    for method in ["filter", "map", "join", "sort_by"] {
        for ty in ["StateList", "ReadList"] {
            assert!(api.get(&format!("octara::reactive::list::{ty}::{method}")).unwrap().sig.contains("ReadList<"));
        }
    }
    assert!(api.get("octara::reactive::list::Groups::flatten").unwrap().sig.contains("ReadList<"));
    assert!(api.get("octara::task::Task::detach").is_none());
    assert!(api.get("octara::app::App::shutdown").is_some());
    assert!(api.get("octara::app::Window::kernel_mut").is_none());
    assert!(api.get("octara::app::Window::platform_window_mut").is_none());
    assert!(api.get("octara::platform::PlatformWindow::as_any_mut").is_none());
    assert!(!api.get("octara::app::App::platform").unwrap().sig.contains("RefMut"));
    assert!(api.get("octara::kernel::Kernel::resources").is_none());
    assert!(api.get("octara::kernel::Resources").is_none());
    for method in [
        "new",
        "set_theme",
        "register_font",
        "register_fallback_font",
        "set_touch",
        "set_reduced_motion",
        "theme_changed",
        "fonts_changed",
        "touch_changed",
    ] {
        assert!(api.get(&format!("octara::kernel::Kernel::{method}")).is_none(), "Kernel no longer owns resource mutation: {method}");
    }
    assert!(api.get("octara::kernel::Resources::set_text_engine").is_none());
    assert!(api.get("octara::kernel::Kernel::set_text_engine").is_none());
    assert!(api.get("octara::app::App::set_text_engine").is_none());
    assert!(api.get("octara::app::App::use_system_fonts").is_none());
    assert!(api.get("octara::app::App::with_config").is_some());
    assert_eq!(api.get("octara::prelude::AppConfig").unwrap().path, "octara::app::AppConfig");
    for method in ["kernel", "reset_counters", "set_layout_enabled", "set_culling"] {
        assert!(api.get(&format!("octara::app::Window::{method}")).is_some());
    }
    assert!(api.get("octara::app::TargetGuard").is_some());
    assert!(api.get("octara::inspect::TargetGuard").is_none());
    assert!(api.get("octara::kernel::Kernel::inspect_tree").is_none());
    assert!(api.get("octara::kernel::Kernel::pick").is_none());
    assert!(api.get("octara::inspect::tree").is_some());
    assert!(api.get("octara::inspect::pick").is_some());
    assert!(api.get("octara::reactive::Signal::get_untracked").is_none());
    assert!(api.get("octara::reactive::ReadSignal::with_untracked").is_none());
    assert!(api.get("octara::reactive::Watch::stop").is_some());
    let watch = api.get("octara::reactive::watch").unwrap();
    assert!(!watch.sig.contains("Clone"), "watch sources may be non-Clone: {}", watch.sig);
    assert!(api.get("octara::reactive::memo").unwrap().sig.contains("ReadSignal<T>"));
}

#[test]
fn items_say_where_they_are() {
    let api = api::load();
    for c in &api.crates {
        assert!(c.items.windows(2).all(|w| (&w[0].path, w[0].kind) < (&w[1].path, w[1].kind)), "{} items are sorted and unique", c.name);
        assert!(std::path::Path::new(&c.root).join("Cargo.toml").exists(), "{}: root is the crate's directory", c.name);
    }
    let text = api.get("octara::view::text").unwrap();
    let (file, line) = text.file.rsplit_once(':').unwrap();
    assert_eq!(file, "src/view/element.rs");
    let source = std::fs::read_to_string(api::workspace_root().join("crates/octara").join(file)).unwrap();
    let at = source.lines().nth(line.parse::<usize>().unwrap() - 1).unwrap();
    assert!(at.contains("pub fn text("), "the line is the declaration: {at}");

    let theme = api.get("octara::prelude::Theme").unwrap();
    assert_eq!(theme.path, "octara_primitives::theme::Theme");
    assert_eq!(theme.kind, Kind::Struct);
    assert!(!theme.docs.is_empty(), "facade aliases retain the definition's docs");
    let (file, line) = theme.file.rsplit_once(':').unwrap();
    let source = std::fs::read_to_string(api::workspace_root().join("crates/octara-primitives").join(file)).unwrap();
    let at = source.lines().nth(line.parse::<usize>().unwrap() - 1).unwrap();
    assert!(at.contains("pub struct Theme"), "the alias points to the real declaration: {at}");
}

#[test]
fn reexports_between_crates_resolve_types_modules_globs_and_members() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("api-cross-crate-exports");
    let sources: Vec<_> = [
        (
            "octara-primitives",
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
        ("octara-bridge", "pub use octara_primitives::shapes as geometry;\npub use octara_primitives::shapes::Rect as Rectangle;"),
        (
            "octara",
            r#"
pub mod prelude {
    pub use octara_bridge::geometry::*;
    pub use octara_primitives::edges::*;
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
    assert_eq!(rect.path, "octara_primitives::shapes::Rect");
    assert_eq!(rect.docs, "A rectangle with a public width.");
    assert_eq!(rect.kind, Kind::Struct);
    assert!(rect.target.is_empty());
    assert_eq!(api.get("octara::geometry::Rect").unwrap(), rect);
    assert_eq!(api.get("octara::prelude::Rect").unwrap(), rect);
    assert_eq!(api.get("octara::prelude::Edge::Top").unwrap().path, "octara_primitives::edges::Edge::Top");
    let constructor = api.get("octara::prelude::Box::new").unwrap();
    assert_eq!(constructor.path, "octara_primitives::shapes::Rect::new");
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
    assert!(entry.file.starts_with(&format!("{}/src/lib.rs:", sources[0].dir.display())), "{}", entry.file);
    assert!(catalog.prelude().all(|entry| entry.path != constructor.path), "member aliases do not become prelude imports");
    let (_, index) = octara_cli::reference::files(&api).into_iter().find(|(path, _)| path == "index.md").unwrap();
    assert!(index.contains("Prelude (`use octara::prelude::*`): Box, Edge, Rect"), "{index}");
    assert!(!index.contains("use octara_primitives::prelude::*"), "the facade's prelude belongs to its own crate");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_read_is_cached_until_the_source_changes() {
    let dir = std::env::temp_dir().join(format!("octara-api-cache-{}", std::process::id()));
    let sources = api::workspace_sources();
    let a = api::cached(&sources, &dir).unwrap();
    assert!(dir.join("octara/api.json").exists());
    let b = api::cached(&sources, &dir).unwrap();
    assert_eq!(a, b);
    std::fs::remove_dir_all(&dir).unwrap();
}
