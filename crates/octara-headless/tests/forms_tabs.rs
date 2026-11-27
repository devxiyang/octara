#![cfg(not(target_arch = "wasm32"))]

use octara::{Input, InteractionEvent, prelude::*};
use octara_headless::{
    TooltipState,
    checkbox::{checkbox, switch},
    field::{FieldState, FieldsetState, control, description, field, fieldset, label},
    range::{NumberFieldState, ProgressState, Range, SliderState},
    tabs::{Activation, Orientation, TabsState},
};
use octara_inspect::{AgentAction, AgentCommand, AgentRequest, HeadlessDriver, TargetRef, protocol::TreeSnapshot};
use octara_pipeline::{NativeEngine, NativeEngineConfig, NativePublication, a11y::node_identity};

fn driver() -> HeadlessDriver {
    HeadlessDriver::new(NativeEngineConfig { check_a11y_cache: true, ..Default::default() })
}

fn lookup(driver: &HeadlessDriver, window: WindowId, name: &str) -> TargetRef {
    let tree = driver.app().runtime().window(window).unwrap().tree();
    tree.guard(tree.test_ids(name)[0])
}

fn action(driver: &mut HeadlessDriver, window: WindowId, name: &str, action: A11yAction) {
    let target = lookup(driver, window, name);
    let receipt =
        driver.execute(AgentRequest::new(Some(window), AgentCommand::Act { target, action: AgentAction::Accessibility { action } }));
    assert!(receipt.ok && receipt.handled == Some(true), "{receipt:?}");
}

fn key(driver: &mut HeadlessDriver, window: WindowId, key: Key) {
    let event = InteractionEvent::Key(KeyEvent::new(key, Modifiers::empty()));
    assert!(driver.dispatch(window, Input::Interaction(event)).unwrap().handled);
}

fn observe(driver: &mut HeadlessDriver, window: WindowId) -> TreeSnapshot {
    driver.execute(AgentRequest::new(Some(window), AgentCommand::Observe)).snapshot.unwrap()
}

#[test]
fn field_updates_the_accessible_name_on_the_same_editor_and_inherits_disabled_state() {
    let publication = NativePublication::default();
    let mut app = App::new({
        let publication = publication.clone();
        move |_, _| Box::new(NativeEngine::with_publication(Default::default(), publication.clone()))
    });
    let (name, disabled) = app.build(|build| (build.signal(String::from("Name")), build.signal(false))).unwrap();
    let window = app
        .open_window(WindowOptions::default(), move |build| {
            let group = FieldsetState::new(build, disabled);
            fieldset(el().test_id("fieldset"), group, move |build| {
                let field_state = FieldState::new(build, false);
                field(el().test_id("field"), field_state, move |build| {
                    View::fragment([
                        label(build, text(name).test_id("label")).into_view(),
                        description(build, text("Use the full name").test_id("description")).into_view(),
                        TooltipState::new(build, Default::default())
                            .trigger(control(build, input().test_id("editor").value("Original")), Placement::BELOW, None, |_| {
                                text("Optional hint")
                            })
                            .into_view(),
                    ])
                })
            })
        })
        .unwrap();

    let tree = app.runtime().window(window).unwrap().tree();
    let [editor, label, description, field] = ["editor", "label", "description", "field"].map(|name| tree.test_ids(name)[0]);
    assert_eq!(tree.children(field), [label, description, editor]);
    let editor_target = tree.guard(editor);
    let get_name = || {
        let snapshot = publication.snapshot_full().unwrap();
        let (_, editor) = snapshot.nodes.iter().find(|(id, _)| *id == node_identity(editor)).unwrap();
        assert_eq!(editor.labelled_by(), &[node_identity(label)]);
        assert_eq!(editor.described_by(), &[node_identity(description)]);
        let (_, label) = snapshot.nodes.iter().find(|(id, _)| *id == node_identity(label)).unwrap();
        label.value().or(label.label()).unwrap().to_owned()
    };
    assert_eq!(get_name(), "Name");
    app.update(|write| write.set(name, "Full name".to_owned())).unwrap();
    assert_eq!(get_name(), "Full name");
    let tree = app.runtime().window(window).unwrap().tree();
    assert_eq!(tree.guard(tree.test_ids("editor")[0]), editor_target);
    assert_eq!(tree.editor(editor).unwrap().value(), "Original");
    app.update(|write| write.set(disabled, true)).unwrap();
    assert!(!app.runtime().window(window).unwrap().tree().is_interactive(editor));
}

#[test]
fn interaction_feedback_on_a_child_preserves_its_parent_button_activation() {
    use octara::{PointerId, PointerKind};
    use octara_headless::interactive::{button, data_interactive};

    let mut driver = driver();
    let clicks = driver.app_mut().build(|b| b.signal(0)).unwrap();
    let window = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |_| {
            button(el())
                .test_id("button")
                .on_press(move |write| write.update(clicks, |clicks| *clicks += 1))
                .child(data_interactive(el().w(60).h(40).test_id("child")))
        })
        .unwrap();
    let child = lookup(&driver, window, "child").node().unwrap();
    let runtime = driver.app().runtime().window(window).unwrap();
    let bounds = runtime.geometry(child).unwrap().rect;
    let pos = point(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0);
    let target = runtime.hit_test(pos).unwrap();
    assert_eq!(target.hit.node(), Some(child));
    assert_eq!(target.action, Some(lookup(&driver, window, "button")));
    let id = PointerId(1);
    driver
        .dispatch(
            window,
            Input::Interaction(InteractionEvent::PointerDown {
                id,
                kind: PointerKind::Mouse,
                target,
                pos,
                button: Button::Primary,
                mods: Modifiers::empty(),
            }),
        )
        .unwrap();
    let pressed =
        |driver: &HeadlessDriver| driver.app().runtime().window(window).unwrap().tree().node(child).unwrap().state.contains(State::PRESSED);
    assert!(pressed(&driver));
    driver
        .dispatch(window, Input::Interaction(InteractionEvent::PointerUp { id, target: Some(target), pos, mods: Modifiers::empty() }))
        .unwrap();
    assert!(!pressed(&driver));
    assert_eq!(driver.app_mut().update(|w| w.get(clicks)).unwrap(), 1);
}

#[test]
fn nested_fieldset_disability_preserves_the_enclosing_field_description() {
    let mut driver = driver();
    let disabled = driver.app_mut().build(|b| b.signal(true)).unwrap();
    let window = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |build| {
            let outer = FieldState::new(build, false);
            field(col(), outer, move |build| {
                let inner = FieldsetState::new(build, disabled);
                col()
                    .child(description(build, text("Help").test_id("help")))
                    .child(fieldset(el(), inner, |build| control(build, input().test_id("control"))))
            })
        })
        .unwrap();
    let help = lookup(&driver, window, "help");
    let target = lookup(&driver, window, "control");
    for blocked in [true, false] {
        driver.app_mut().update(|write| write.set(disabled, blocked)).unwrap();
        let snapshot = observe(&mut driver, window);
        let node = snapshot.nodes.iter().find(|node| node.target == target).unwrap();
        assert_eq!(node.interactive, !blocked);
        assert_eq!(node.relations.described_by, Some(help));
    }
}

#[test]
fn disclosure_close_button_uses_the_panel_scope_and_restores_focus() {
    use octara_headless::{Control, DisclosureState, close::close_button};

    let mut driver = driver();
    let window = driver
        .app_mut()
        .open_window(WindowOptions::default(), |build| {
            let disclosure = DisclosureState::new(build, Control::Uncontrolled(false));
            col()
                .child(disclosure.button(text("Details").test_id("opener")))
                .child(disclosure.panel(|_| el().test_id("panel").child(close_button(text("Close").test_id("close")))))
        })
        .unwrap();
    action(&mut driver, window, "opener", A11yAction::Press);
    let panel = lookup(&driver, window, "panel");
    action(&mut driver, window, "close", A11yAction::Focus);
    action(&mut driver, window, "close", A11yAction::Press);
    let snapshot = observe(&mut driver, window);
    assert!(!snapshot.nodes.iter().any(|node| node.target == panel));
    assert_eq!(snapshot.focus, Some(lookup(&driver, window, "opener")));
}

#[test]
fn manual_tabs_move_focus_without_selecting_until_confirmation() {
    let mut driver = driver();
    let selected = driver.app_mut().build(|build| build.signal(Some(1))).unwrap();
    let window = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |build| {
            let tabs = TabsState::new(build, selected, Activation::Manual, Orientation::Horizontal);
            col()
                .child(
                    tabs.list(row().test_id("list"))
                        .child(tabs.tab(text("One").test_id("one"), 1))
                        .child(tabs.tab(text("Unavailable").test_id("two").disabled(true), 2))
                        .child(tabs.tab(text("Three").test_id("three"), 3)),
                )
                .child(
                    tabs.panels(col())
                        .child(tabs.panel(text("First").test_id("first-panel"), 1))
                        .child(tabs.panel(text("Third").test_id("third-panel"), 3)),
                )
        })
        .unwrap();
    action(&mut driver, window, "one", A11yAction::Focus);
    key(&mut driver, window, Key::Right);
    let before = observe(&mut driver, window);
    assert_eq!(before.focus, Some(lookup(&driver, window, "three")));
    let shown = |snap: &TreeSnapshot, id: TargetRef| snap.nodes.iter().find(|node| node.target == id).unwrap().shown;
    let first = lookup(&driver, window, "first-panel");
    let third = lookup(&driver, window, "third-panel");
    assert!(shown(&before, first) && !shown(&before, third));
    key(&mut driver, window, Key::Enter);
    let after = observe(&mut driver, window);
    assert!(!shown(&after, first) && shown(&after, third));
    let tab = after.nodes.iter().find(|node| Some(node.target) == after.focus).unwrap();
    assert_eq!(tab.relations.controls, Some(third));
    assert_eq!(driver.app_mut().update(|write| write.get(selected)).unwrap(), Some(3));
}

#[test]
fn checked_mixed_switch_and_range_use_the_callers_actual_elements() {
    let mut driver = driver();
    let (checked, switched, volume, draft) =
        driver.app_mut().build(|b| (b.signal(Checked::Mixed), b.signal(false), b.signal(3.0), b.signal(String::from("-")))).unwrap();
    let window = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |build| {
            let range = Range::new(0.0, 10.0, 0.5);
            let slider = SliderState::new(build, volume, range);
            let number = NumberFieldState::new(build, draft, range);
            let progress = ProgressState::new(build.signal(Some(40.0)), 30.0);
            col()
                .child(checkbox(el().test_id("check"), checked.into()))
                .child(switch(el().test_id("switch"), switched.into()))
                .child(slider.slider(el().test_id("volume")))
                .child(number.input(input().test_id("number")))
                .child(progress.progress(el().test_id("progress")))
        })
        .unwrap();
    let snapshot = observe(&mut driver, window);
    assert_eq!(snapshot.nodes.iter().find(|node| node.test_id.as_deref() == Some("check")).unwrap().checked, Some(Checked::Mixed));
    action(&mut driver, window, "check", A11yAction::Press);
    action(&mut driver, window, "switch", A11yAction::Press);
    action(&mut driver, window, "volume", A11yAction::SetNumericValue(8.8));
    action(&mut driver, window, "volume", A11yAction::Focus);
    key(&mut driver, window, Key::Left);
    action(&mut driver, window, "number", A11yAction::Focus);
    key(&mut driver, window, Key::Enter);
    let state = driver.app_mut().update(|w| (w.get(checked), w.get(switched), w.get(volume), w.get(draft))).unwrap();
    assert_eq!(state, (Checked::Checked, true, 8.5, "-".to_owned()));
    action(&mut driver, window, "number", A11yAction::SetNumericValue(5.2));
    assert_eq!(driver.app_mut().update(|w| w.get(draft)).unwrap(), "5");
}

#[test]
fn controllers_are_copy_with_owned_keys_and_payloads() {
    use octara_headless::{
        Collection, ComboboxState, ContextMenuState, DataTableState, DialogState, DisclosureState, ListboxState, MenuState, PopoverGroup,
        PopoverState, RadioGroupState, Selection, ToastQueue, TreeViewState, tooltip::TooltipGroup,
    };

    fn check<T: Copy>() {}

    check::<Collection<String, String>>();
    check::<Selection<String>>();
    check::<DialogState>();
    check::<DisclosureState>();
    check::<PopoverState>();
    check::<PopoverGroup>();
    check::<MenuState<String>>();
    check::<ContextMenuState<String>>();
    check::<ListboxState<String>>();
    check::<ComboboxState<String>>();
    check::<RadioGroupState<String>>();
    check::<FieldState>();
    check::<FieldsetState>();
    check::<TabsState<String>>();
    check::<TreeViewState<String>>();
    check::<DataTableState<String, String>>();
    check::<ToastQueue<String>>();
    check::<TooltipState>();
    check::<TooltipGroup>();
    check::<NumberFieldState>();
    check::<ProgressState>();
    check::<SliderState>();
}
