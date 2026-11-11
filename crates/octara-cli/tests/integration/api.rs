//! `api.json` matches the source, and answers the questions the CLI asks of it.

use octara_cli::api::{self, Kind};

#[test]
fn api_json_matches_the_source() {
    let fresh = api::generate().unwrap_or_else(|e| panic!("{e}"));
    let text = api::to_json(&fresh);
    if text != api::CHECKED_IN {
        let (added, removed, changed) = api::diff(&api::load(), &fresh);
        let show = |v: &[String]| v.iter().take(8).cloned().collect::<Vec<_>>().join(", ");
        panic!(
            "api.json is out of date — run `cargo run -p octara-cli -- api`\n  added: {}\n  removed: {}\n  changed: {}",
            show(&added),
            show(&removed),
            show(&changed)
        );
    }
}

#[test]
fn items_are_found_by_canonical_path_and_alias() {
    let api = api::load();
    assert_eq!(api.format, api::FORMAT);
    let state = api.get("octara::prelude::state").expect("the prelude alias resolves");
    assert_eq!(state.path, "octara::reactive::graph::state");
    assert_eq!(state.kind, Kind::Fn);
    assert!(state.sig.starts_with("pub fn state<T: Clone + 'static>(key: &'static str"), "{}", state.sig);
    let p = api.get("octara::view::Styled::p_4").expect("trait methods are items");
    assert_eq!((p.kind, p.sig.as_str()), (Kind::Method, "fn p_4(self) -> Self"));
    let set = api.get("octara::reactive::graph::Signal::set").unwrap();
    assert_eq!(set.within, "impl<T: Clone + 'static> Signal<T>", "methods keep their impl block's bounds");
    let spring = api.get("octara::motion::Spring").unwrap();
    assert!(spring.traits.iter().any(|t| t == "Into<Curve>"), "{:?}", spring.traits);
    let el = api.get("octara::view::El").unwrap();
    assert!(el.traits.iter().any(|t| t == "Styled"));
    assert_eq!(api.get("octara::kernel::Role").unwrap().target, "accesskit::Role", "re-exports name their target");
}

#[test]
fn items_are_unique_and_sorted() {
    for c in &api::load().crates {
        // A field and a method may share a name (`BoxShadow::blur_radius`); a path and kind may not.
        assert!(c.items.windows(2).all(|w| (&w[0].path, w[0].kind) < (&w[1].path, w[1].kind)), "{} items are sorted and unique", c.name);
        assert!(c.items.iter().all(|i| !i.file.starts_with('/')), "{}: files are relative to the workspace", c.name);
    }
}
