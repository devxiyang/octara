#![cfg(not(target_arch = "wasm32"))]

use octara::{EditorEvent, Input, InteractionEvent, prelude::*};
use octara_headless::{Choice, ComboboxState, ListboxState, Selection};
use octara_inspect::{
    AgentAction, AgentCommand, AgentRequest, HeadlessDriver, TargetRef,
    protocol::{NodeSnapshot, TreeSnapshot},
};
use octara_pipeline::NativeEngineConfig;

fn driver() -> HeadlessDriver {
    HeadlessDriver::with_namespace([19, 47], NativeEngineConfig { check_a11y_cache: true, ..Default::default() })
}

fn window_options() -> WindowOptions {
    WindowOptions { viewport: WindowViewport { size: Size::new(300.0, 220.0), scale: 1.0, safe_area: None }, ..Default::default() }
}

fn observe(driver: &mut HeadlessDriver, window: WindowId) -> TreeSnapshot {
    let receipt = driver.execute(AgentRequest::new(Some(window), AgentCommand::Observe));
    assert!(receipt.ok && receipt.progress.settled, "{receipt:?}");
    receipt.snapshot.unwrap()
}

fn node<'a>(snapshot: &'a TreeSnapshot, test_id: &str) -> &'a NodeSnapshot {
    snapshot.nodes.iter().find(|node| node.test_id.as_deref() == Some(test_id)).unwrap_or_else(|| panic!("missing {test_id}"))
}

fn focus(driver: &mut HeadlessDriver, window: WindowId, target: TargetRef) {
    let receipt = driver.execute(AgentRequest::new(
        Some(window),
        AgentCommand::Act { target, action: AgentAction::Accessibility { action: A11yAction::Focus } },
    ));
    assert_eq!(receipt.handled, Some(true), "{receipt:?}");
}

fn key(driver: &mut HeadlessDriver, window: WindowId, key: Key) -> bool {
    driver.dispatch(window, Input::Interaction(InteractionEvent::Key(KeyEvent::new(key, Modifiers::empty())))).unwrap().handled
}

#[test]
fn listbox_reenables_the_same_trigger_and_selects_duplicate_labels_by_key() {
    let mut driver = driver();
    let (rows, value, caller_disabled) =
        driver.app_mut().build(|b| (b.list::<u32, Choice>(), b.signal(None::<u32>), b.signal(false))).unwrap();
    let window = driver
        .app_mut()
        .open_window(window_options(), move |b| {
            let picker = ListboxState::new(b, rows, Selection::Single(value.into()));
            let trigger = picker.button(text("Model").test_id("picker").disabled(caller_disabled));
            col().child(trigger).child(show(
                move |r| picker.is_open(r),
                move |_| picker.options(col().test_id("options"), |_, key, _| text("Same").h(24).test_id(format!("choice-{key}"))),
            ))
        })
        .unwrap();

    let empty = observe(&mut driver, window);
    let trigger = node(&empty, "picker").target;
    assert!(!node(&empty, "picker").interactive);
    assert!(node(&empty, "picker").relations.controls.is_none());

    driver
        .app_mut()
        .update(|w| {
            w.push_row(rows, 101, Choice::new("Same"));
            w.push_row(rows, 201, Choice::new("Same").disabled(true));
            w.push_row(rows, 302, Choice::new("Same"));
            w.set(caller_disabled, true);
        })
        .unwrap();
    assert!(!node(&observe(&mut driver, window), "picker").interactive);
    driver.app_mut().update(|w| w.set(caller_disabled, false)).unwrap();
    let populated = observe(&mut driver, window);
    assert_eq!(node(&populated, "picker").target, trigger);
    assert!(node(&populated, "picker").interactive);
    focus(&mut driver, window, trigger);
    assert!(key(&mut driver, window, Key::Down));

    let opened = observe(&mut driver, window);
    let options = node(&opened, "options");
    assert_eq!(opened.focus, Some(options.target));
    assert_eq!(node(&opened, "picker").relations.controls, Some(options.target));
    assert_eq!(options.relations.active_descendant, Some(node(&opened, "choice-101").target));
    assert!(!node(&opened, "choice-201").interactive);
    assert!(key(&mut driver, window, Key::Char('S')));

    let typed = observe(&mut driver, window);
    let chosen = node(&typed, "choice-302").target;
    assert_eq!(node(&typed, "options").relations.active_descendant, Some(chosen));
    assert_eq!(node(&typed, "choice-101").label, node(&typed, "choice-302").label);
    driver.app_mut().update(|w| w.move_row(rows, &302, 0)).unwrap();
    assert_eq!(node(&observe(&mut driver, window), "options").relations.active_descendant, Some(chosen));

    assert!(key(&mut driver, window, Key::Enter));
    assert_eq!(driver.app_mut().update(|w| w.get(value)).unwrap(), Some(302));
    assert_eq!(observe(&mut driver, window).focus, Some(trigger));
    assert!(key(&mut driver, window, Key::Down));
    let reopened = observe(&mut driver, window);
    assert_eq!(node(&reopened, "options").relations.active_descendant, Some(node(&reopened, "choice-302").target));
    driver.app_mut().update(|w| w.remove_row(rows, &302)).unwrap();
    assert!(node(&observe(&mut driver, window), "options").relations.active_descendant.is_none());
    driver.app_mut().update(|w| w.push_row(rows, 302, Choice::new("Same"))).unwrap();
    assert!(node(&observe(&mut driver, window), "options").relations.active_descendant.is_none());

    driver
        .app_mut()
        .update(|w| {
            w.remove_row(rows, &101);
            w.remove_row(rows, &201);
            w.remove_row(rows, &302);
        })
        .unwrap();
    let empty = observe(&mut driver, window);
    assert_eq!(node(&empty, "picker").target, trigger);
    assert!(!node(&empty, "picker").interactive);
    assert!(node(&empty, "picker").relations.controls.is_none());
    assert!(!empty.nodes.iter().any(|node| node.test_id.as_deref() == Some("options")));
}

#[test]
fn picker_keyboard_navigation_honors_disabled_state_on_mounted_option_elements() {
    for editable in [false, true] {
        let mut driver = driver();
        let (rows, selected, disabled) =
            driver.app_mut().build(|b| (b.list::<u32, Choice>(), b.signal(None::<u32>), b.signal(false))).unwrap();
        driver
            .app_mut()
            .update(|w| {
                for key in 0..5 {
                    w.push_row(rows, key, Choice::new("Same"));
                }
            })
            .unwrap();
        let window = driver
            .app_mut()
            .open_window(window_options(), move |b| {
                let selection = Selection::Single(selected.into());
                let render = move |_: &mut BuildCx<'_>, key: &u32, _| {
                    let key = *key;
                    text("Same")
                        .h(24)
                        .test_id(format!("choice-{key}"))
                        .disabled(bind(move |r| matches!(key, 0 | 2) || key == 3 && r.get(disabled)))
                };
                if editable {
                    let picker = ComboboxState::new(b, rows, selection);
                    col()
                        .child(picker.input(input().test_id("trigger").h(30)))
                        .child(show(move |r| picker.is_open(r), move |_| picker.options(col().test_id("options"), render)))
                } else {
                    let picker = ListboxState::new(b, rows, selection);
                    col()
                        .child(picker.button(text("Choose").test_id("trigger").h(30)))
                        .child(show(move |r| picker.is_open(r), move |_| picker.options(col().test_id("options"), render)))
                }
            })
            .unwrap();
        let trigger = node(&observe(&mut driver, window), "trigger").target;
        focus(&mut driver, window, trigger);
        assert!(key(&mut driver, window, Key::Down));

        let owner = if editable { "trigger" } else { "options" };
        let opened = observe(&mut driver, window);
        assert_eq!(node(&opened, owner).relations.active_descendant, Some(node(&opened, "choice-1").target));
        assert!(!node(&opened, "choice-0").interactive);
        let next = if editable { Key::Down } else { Key::Char('S') };
        assert!(key(&mut driver, window, next));
        let advanced = observe(&mut driver, window);
        assert_eq!(node(&advanced, owner).relations.active_descendant, Some(node(&advanced, "choice-3").target));
        assert!(!node(&advanced, "choice-2").interactive);

        driver.app_mut().update(|w| w.set(disabled, true)).unwrap();
        assert!(key(&mut driver, window, Key::Enter));
        assert_eq!(driver.app_mut().update(|w| w.get(selected)).unwrap(), None);
        assert!(node(&observe(&mut driver, window), owner).relations.active_descendant.is_none());

        assert!(key(&mut driver, window, Key::Down));
        assert!(key(&mut driver, window, Key::Down));
        let advanced = observe(&mut driver, window);
        assert_eq!(node(&advanced, owner).relations.active_descendant, Some(node(&advanced, "choice-4").target));
        assert!(key(&mut driver, window, Key::Enter));
        assert_eq!(driver.app_mut().update(|w| w.get(selected)).unwrap(), Some(4));
    }
}

#[test]
fn combobox_composition_precedes_selection_and_virtual_navigation_reveals_the_active_row() {
    let mut driver = driver();
    let (rows, selected, commits) = driver.app_mut().build(|b| (b.list::<u32, Choice>(), b.signal(None::<u32>), b.signal(0))).unwrap();
    driver
        .app_mut()
        .update(|w| {
            for key in 0..40 {
                w.push_row(rows, key, Choice::new(format!("日本語 {key}")).disabled(key == 39));
            }
        })
        .unwrap();
    let window = driver
        .app_mut()
        .open_window(window_options(), move |b| {
            let value = b.binding(
                move |r| r.get(selected),
                move |w, value| {
                    w.set(selected, value);
                    w.update(commits, |commits| *commits += 1);
                },
            );
            let picker = ComboboxState::new(b, rows, Selection::Single(value));
            col().child(picker.input(input().test_id("query").label("Find a choice").h(30))).child(show(
                move |r| picker.is_open(r),
                move |b| {
                    picker.virtual_options(
                        b,
                        col().test_id("options").h(60).overflow_y(Overflow::Scroll),
                        VirtualOptions { estimated_row_height: 20.0, overscan: 0, ..Default::default() },
                        |_, key, row| {
                            text(bind(move |r| r.try_get(row).map(|choice| choice.label).unwrap_or_default()))
                                .h(20)
                                .test_id(format!("choice-{key}"))
                        },
                    )
                },
            ))
        })
        .unwrap();

    let initial = observe(&mut driver, window);
    let input = node(&initial, "query").target;
    assert_eq!(node(&initial, "query").role, Some(Role::EditableComboBox));
    focus(&mut driver, window, input);
    assert!(key(&mut driver, window, Key::Down));
    let opened = observe(&mut driver, window);
    assert_eq!(opened.focus, Some(input));
    assert_eq!(node(&opened, "query").relations.active_descendant, Some(node(&opened, "choice-0").target));
    assert_eq!(node(&opened, "options").scroll_offset.unwrap().y, 0.0);
    assert!(!opened.nodes.iter().any(|node| node.test_id.as_deref() == Some("choice-38")));

    let editor = opened.editor.unwrap();
    let composition = EditorEvent::Composition {
        session: editor.session,
        expected_rev: editor.document_rev,
        text: "に".into(),
        selection: Some((3, 3)),
        replacement: Some(0..0),
    };
    assert!(driver.dispatch(window, Input::Editor(composition)).unwrap().handled);
    let composing = observe(&mut driver, window).editor.unwrap();
    assert_eq!(composing.marked, Some(0..3));
    let enter = EditorEvent::KeyIntercept {
        session: composing.session,
        expected_rev: composing.document_rev,
        key: KeyEvent::new(Key::Enter, Modifiers::empty()),
    };
    assert!(!driver.dispatch(window, Input::Editor(enter)).unwrap().handled);
    assert_eq!(driver.app_mut().update(|w| (w.get(selected), w.get(commits))).unwrap(), (None, 0));
    assert_eq!(observe(&mut driver, window).editor.unwrap(), composing);

    let commit = EditorEvent::Commit { session: composing.session, expected_rev: composing.document_rev, text: "日本語".into() };
    assert!(driver.dispatch(window, Input::Editor(commit)).unwrap().handled);
    let composed = observe(&mut driver, window);
    let editor = composed.editor.unwrap();
    assert_eq!(editor.value.as_deref(), Some("日本語"));
    assert!(editor.marked.is_none());
    assert_eq!(driver.app_mut().update(|w| w.get(commits)).unwrap(), 0);

    assert!(key(&mut driver, window, Key::Up));
    let revealed = observe(&mut driver, window);
    let row = node(&revealed, "choice-38");
    let options = node(&revealed, "options");
    let list = options.target.node().unwrap();
    let offset = options.scroll_offset.unwrap();
    assert!(row.shown);
    assert_eq!(row.parent, Some(options.target));
    assert!(offset.y > 0.0);
    assert_eq!(node(&revealed, "query").relations.active_descendant, Some(row.target));
    assert_eq!(revealed.focus, Some(input));

    driver.app_mut().update_window(window, |w| w.scroll_to(list, Vec2::default())).unwrap();
    let scrolled_away = observe(&mut driver, window);
    assert!(!scrolled_away.nodes.iter().any(|node| node.test_id.as_deref() == Some("choice-38")));
    assert!(node(&scrolled_away, "query").relations.active_descendant.is_none());
    driver.app_mut().update_window(window, |w| w.scroll_to(list, offset)).unwrap();
    let scrolled_back = observe(&mut driver, window);
    assert_eq!(node(&scrolled_back, "query").relations.active_descendant, Some(node(&scrolled_back, "choice-38").target));

    assert!(key(&mut driver, window, Key::Enter));
    assert!(!key(&mut driver, window, Key::Enter));
    assert_eq!(driver.app_mut().update(|w| (w.get(selected), w.get(commits))).unwrap(), (Some(38), 1));
    let closed = observe(&mut driver, window);
    assert_eq!(closed.focus, Some(input));
    assert!(node(&closed, "query").relations.active_descendant.is_none());
}
