//! The API read from the source: canonical paths and aliases, signatures, members written by
//! macros, impl blocks' bounds, trait impls, re-exports, and where each item is.

use octara_cli::api::{self, Kind};

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
    let spring = api.get("octara::motion::Spring").unwrap();
    assert!(spring.traits.iter().any(|t| t == "Into<Curve>"), "{:?}", spring.traits);
    let el = api.get("octara::view::El").unwrap();
    assert!(el.traits.iter().any(|t| t == "Styled"));
    assert_eq!(api.get("octara::kernel::Role").unwrap().target, "accesskit::Role", "re-exports name their target");
    assert!(api.get("octara::kernel::State::HOVER").is_some(), "bitflags' flags are items");
    assert!(api.get("octara::kernel::NodeId").is_some(), "new_key_type's keys are items");
    assert!(api.get("octara::diagnostics::OVERFLOW").is_some(), "codes! writes the registry's statics");
    assert!(api.get("octara::view::enter_component").is_none(), "#[doc(hidden)] is not API");
    assert!(api.get("octara::kernel::Node").is_none(), "private items are not API");
    assert!(api.items().all(|item| !item.path.contains("::reactive::graph") && !item.path.contains("::reactive::seq")));
    assert!(api.get("octara::reactive::graph::state").is_none(), "old implementation paths are not aliases");
    assert!(api.get("octara::prelude::Memo").is_none());
    assert!(api.get("octara::reactive::Signal::new").is_none());
    assert!(api.get("octara::task::Task::detach").is_none());
    assert!(api.get("octara::app::App::shutdown").is_some());
    assert!(api.get("octara::app::Window::kernel_mut").is_none());
    assert!(api.get("octara::kernel::Kernel::resources").is_none());
    assert!(api.get("octara::kernel::Resources::set_text_engine").is_none());
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
