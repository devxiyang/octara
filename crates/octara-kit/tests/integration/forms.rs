//! Retained form controls, validation semantics and native radio keyboard behavior.

use octara::accesskit::{Invalid, Toggled};
use octara::inspect::{self, Delivery};
use octara::prelude::*;
use octara::serde_json::json;
use octara_kit::*;

fn clean(window: &Window) {
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

fn focused(window: &Window, id: &str) -> bool {
    window.kernel().state_of(id).is_some_and(|state| state.contains(State::FOCUSED))
}

#[ui]
fn labeled_name() -> View {
    let name = state("name", || "Ada".to_owned());
    let info = state("info", || FieldInfo { description: "Use your full name.".into(), required: true, ..FieldInfo::default() });
    on_command("field.error", move || info.update(|info| info.error = Some("Enter a full name.".into())));
    on_command("field.other-error", move || info.update(|info| info.error = Some("Check the spelling.".into())));
    on_command("field.clear", move || info.update(|info| info.error = None));
    on_command("field.blank", move || {
        info.set(FieldInfo { description: String::new(), error: Some("  ".into()), required: false });
    });
    col()
        .size_full()
        .p_4()
        .gap_3()
        .child(field("name", "Name", text_field(name).w_full(), move || info.get()))
        .child(button("Outside").id("outside"))
        .into()
}

#[test]
fn field_semantics_are_on_the_control_and_validation_is_live() {
    let mut window = Window::headless(labeled_name);
    let target = window.inspect().find("name").unwrap().target.clone();
    let snapshot = window.inspect();
    let control = snapshot.find("name").unwrap();
    assert_eq!(control.name.as_deref(), Some("Name"));
    assert_eq!(control.description.as_deref(), Some("Use your full name."));
    assert_eq!(control.required, Some(true));
    assert_eq!(control.invalid, Some(false));
    let wrapper = snapshot.find("name-field").unwrap();
    assert_eq!(wrapper.required, None);
    assert_eq!(wrapper.invalid, None);
    assert!(snapshot.find("name-help").is_some());
    assert!(snapshot.find("name-error").is_none());
    window.tap("name-label").unwrap();
    assert!(focused(&window, "name"), "the visible label focuses its actual control");
    window.invoke("field.error").unwrap();
    let snapshot = window.inspect();
    let control = snapshot.find("name").unwrap();
    assert_eq!(control.target, target);
    assert_eq!(control.description.as_deref(), Some("Use your full name.\nEnter a full name."));
    assert_eq!(control.invalid, Some(true));
    assert_eq!(window.kernel().text_of("name-error").as_deref(), Some("Enter a full name."));
    let tree = window.accessibility_tree();
    let control_id = octara::kernel::a11y::a11y_id(window.kernel().find("name").unwrap());
    let control = &tree.nodes.iter().find(|(id, _)| *id == control_id).unwrap().1;
    assert_eq!(control.label(), Some("Name"));
    assert_eq!(control.description(), Some("Use your full name.\nEnter a full name."));
    assert!(control.is_required());
    assert_eq!(control.invalid(), Some(Invalid::True));
    let error_id = octara::kernel::a11y::a11y_id(window.kernel().find("name-error").unwrap());
    let error = &tree.nodes.iter().find(|(id, _)| *id == error_id).unwrap().1;
    assert_eq!(error.role(), Role::Alert);
    assert_eq!(error.live(), Some(Live::Polite));
    window.invoke("field.other-error").unwrap();
    assert_eq!(window.kernel().text_of("name-error").as_deref(), Some("Check the spelling."));
    window.invoke("field.clear").unwrap();
    assert!(!window.kernel().exists("name-error"));
    assert_eq!(window.inspect().find("name").unwrap().invalid, Some(false));
    window.invoke("field.blank").unwrap();
    let snapshot = window.inspect();
    let control = snapshot.find("name").unwrap();
    assert_eq!(control.target, target);
    assert_eq!(control.description.as_deref(), Some(""));
    assert_eq!(control.required, Some(false));
    assert_eq!(control.invalid, Some(false));
    assert!(!window.kernel().exists("name-help"));
    assert!(!window.kernel().exists("name-error"));
    clean(&window);
}

#[test]
fn validation_preserves_the_editor_selection_composition_undo_and_target() {
    let mut window = Window::headless(labeled_name);
    window.tap("name").unwrap();
    window.press_key("mod+a").unwrap();
    window.type_text("Grace");
    window.set_selection(1, 4);
    let target = window.inspect().find("name").unwrap().target.clone();
    let selection = window.text_field().unwrap().selection;
    window.invoke("field.error").unwrap();
    assert_eq!(window.text_field().unwrap().selection, selection);
    window.event(InputEvent::Preedit { text: "候选".into(), selection: Some((0, 6)) });
    let before = window.text_field().unwrap();
    window.invoke("field.other-error").unwrap();
    window.invoke("field.clear").unwrap();
    let after = window.text_field().unwrap();
    assert_eq!(window.inspect().find("name").unwrap().target, target);
    assert_eq!(after.composition, before.composition);
    assert_eq!(after.composition_selection, before.composition_selection);
    assert!(focused(&window, "name"));
    window.event(InputEvent::Preedit { text: String::new(), selection: None });
    assert_eq!(window.text_field().unwrap().selection, selection);
    window.press_key("mod+z").unwrap();
    assert_eq!(window.kernel().value_of("name").as_deref(), Some("Ada"));
    clean(&window);
}

#[test]
fn field_applies_to_toggle_controls_without_changing_their_models_or_disabled_state() {
    let mut window = Window::headless(|| {
        let checked = state("checked", || false);
        let disabled = state("disabled", || false);
        on_command("toggle.disable", move || disabled.set(true));
        col()
            .size_full()
            .p_4()
            .child(field("terms", "Accept terms", checkbox(checked).disabled(move || disabled.get()), move || FieldInfo {
                description: "Required to continue.".into(),
                error: (!checked.get()).then(|| "Accept the terms.".into()),
                required: true,
            }))
            .into()
    });
    assert_eq!(window.kernel().role_of("terms"), Some(Role::CheckBox));
    assert_eq!(window.inspect().find("terms").unwrap().invalid, Some(true));
    window.tap("terms").unwrap();
    assert_eq!(window.kernel().checked_of("terms"), Some(true));
    assert_eq!(window.inspect().find("terms").unwrap().invalid, Some(false));
    window.invoke("toggle.disable").unwrap();
    assert!(window.dispatch(Event::Toggle("terms".into())).is_err());
    assert_eq!(window.kernel().checked_of("terms"), Some(true));
    clean(&window);
}

fn radio_options() -> Vec<RadioOption> {
    vec![
        RadioOption::new("basic", "Basic").description("For personal projects."),
        RadioOption::new("team", "Team").description("Work with your team."),
        RadioOption::new("legacy", "Legacy").disabled(true),
        RadioOption::new("business", "Business"),
    ]
}

#[ui]
fn plans() -> View {
    let value = state("value", || Some("team".to_owned()));
    let options = state("options", radio_options);
    on_command("radios.reverse", move || options.update(|options| options.reverse()));
    on_command("radios.rename", move || {
        options.update(|options| {
            let option = options.iter_mut().find(|option| option.id == "team").unwrap();
            option.label = "Shared".into();
            option.description = "A shared workspace.".into();
        });
    });
    on_command("radios.remove-team", move || options.update(|options| options.retain(|option| option.id != "team")));
    on_command("radios.disable-team", move || {
        options.update(|options| options.iter_mut().find(|option| option.id == "team").unwrap().disabled = true);
    });
    on_command("radios.disable-all", move || options.update(|options| options.iter_mut().for_each(|option| option.disabled = true)));
    on_command("radios.duplicate", move || options.update(|options| options.push(RadioOption::new("team", "Duplicate"))));
    on_command("radios.restore", move || options.set(radio_options()));
    on_command("radios.empty", move || options.set(vec![]));
    on_command("radios.business", move || value.set(Some("business".into())));
    on_command("radios.clear", move || value.set(None));
    col()
        .size_full()
        .p_4()
        .gap_3()
        .child(button("Before").id("before"))
        .child(radio_group("plan", "Plan", value, move || options.get()))
        .child(button("After").id("after"))
        .child(text(move || value.get().unwrap_or_else(|| "none".into())).id("chosen"))
        .into()
}

#[test]
fn radios_have_checked_semantics_and_navigation_chooses_without_focus_side_effects() {
    let mut window = Window::headless(plans);
    assert_eq!(window.kernel().role_of("plan"), Some(Role::RadioGroup));
    let snapshot = window.inspect();
    let team = snapshot.find("plan-option-team").unwrap();
    assert_eq!(team.role, "radio_button");
    assert_eq!(team.name.as_deref(), Some("Team"));
    assert_eq!(team.description.as_deref(), Some("Work with your team."));
    assert_eq!(team.checked, Some(true));
    assert!(!team.states.iter().any(|state| state == "selected"));
    let tree = window.accessibility_tree();
    let team_id = octara::kernel::a11y::a11y_id(window.kernel().find("plan-option-team").unwrap());
    let team = &tree.nodes.iter().find(|(id, _)| *id == team_id).unwrap().1;
    assert_eq!(team.role(), Role::RadioButton);
    assert_eq!(team.toggled(), Some(Toggled::True));
    window.tap("before").unwrap();
    window.press_key("tab").unwrap();
    assert!(focused(&window, "plan-option-team"));
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("team"));
    window.press_key("tab").unwrap();
    assert!(focused(&window, "after"), "the whole group is a single Tab stop");
    window.press_key("shift+tab").unwrap();
    assert!(focused(&window, "plan-option-team"));
    window.dispatch(Event::Focus("plan-option-basic".into())).unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("team"), "focus alone never chooses");
    window.press_key("right").unwrap();
    assert!(focused(&window, "plan-option-team"));
    window.press_key("down").unwrap();
    assert!(focused(&window, "plan-option-business"), "disabled options are skipped");
    assert_eq!(window.kernel().checked_of("plan-option-business"), Some(true));
    window.press_key("right").unwrap();
    assert!(focused(&window, "plan-option-basic"), "arrows wrap");
    window.press_key("end").unwrap();
    assert!(focused(&window, "plan-option-business"));
    window.press_key("home").unwrap();
    assert!(focused(&window, "plan-option-basic"));
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("basic"));
    window.dispatch(Event::Focus("plan-option-team".into())).unwrap();
    window.press_key("space").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("team"));
    window.dispatch(Event::Focus("plan-option-business".into())).unwrap();
    window.press_key("enter").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("business"));
    assert!(window.dispatch(Event::Press("plan-option-legacy".into())).is_err());
    clean(&window);
}

#[test]
fn radio_source_changes_keep_row_targets_and_follow_live_order() {
    let mut window = Window::headless(plans);
    window.dispatch(Event::Focus("plan-option-team".into())).unwrap();
    let target = window.inspect().find("plan-option-team").unwrap().target.clone();
    let nodes = window.kernel().live_nodes();
    for _ in 0..20 {
        window.invoke("radios.reverse").unwrap();
        assert_eq!(window.kernel().live_nodes(), nodes, "reordering keeps a bounded retained list");
        assert_eq!(window.inspect().find("plan-option-team").unwrap().target, target);
        assert!(focused(&window, "plan-option-team"), "focus stays with the retained key");
    }
    window.invoke("radios.reverse").unwrap();
    let snapshot = window.inspect();
    let options: Vec<_> = snapshot
        .find("plan")
        .unwrap()
        .children
        .iter()
        .filter(|node| node.role == "radio_button")
        .map(|node| node.id.as_deref().unwrap())
        .collect();
    assert_eq!(options, ["plan-option-business", "plan-option-legacy", "plan-option-team", "plan-option-basic"]);
    window.invoke("radios.rename").unwrap();
    let snapshot = window.inspect();
    let team = snapshot.find("plan-option-team").unwrap();
    assert_eq!(team.target, target);
    assert_eq!(team.name.as_deref(), Some("Shared"));
    assert_eq!(team.description.as_deref(), Some("A shared workspace."));
    window.tap("before").unwrap();
    window.press_key("tab").unwrap();
    window.press_key("right").unwrap();
    assert!(focused(&window, "plan-option-basic"), "arrows follow the updated order");
    window.invoke("radios.remove-team").unwrap();
    assert!(!window.kernel().exists("plan-option-team"));
    assert!(window.kernel().live_nodes() < nodes);
    clean(&window);
}

#[test]
fn duplicate_radio_ids_retire_rows_and_recover_with_fresh_targets() {
    let mut window = Window::headless(plans);
    let before = window.inspect();
    let group = before.find("plan").unwrap().target.clone();
    let team = before.find("plan-option-team").unwrap().target.clone();
    window.tap("plan-option-team").unwrap();
    window.invoke("radios.duplicate").unwrap();
    for id in ["basic", "team", "legacy", "business"] {
        assert!(!window.kernel().exists(&format!("plan-option-{id}")), "duplicate IDs invalidate the entire snapshot");
    }
    assert_eq!(window.inspect().find("plan").unwrap().target, group);
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("team"));
    window.tap("before").unwrap();
    window.press_key("tab").unwrap();
    assert!(focused(&window, "after"), "an unavailable snapshot has no radio Tab stop");
    assert!(window.app().diagnostics().iter().any(|d| d.code.id == "DUPLICATE_KEY"));

    window.invoke("radios.restore").unwrap();
    assert_eq!(window.inspect().find("plan").unwrap().target, group);
    assert_ne!(window.inspect().find("plan-option-team").unwrap().target, team);
    let reply = inspect::handle_line(&mut window, &json!({"op": "tap", "id": "plan-option-team", "target": team}).to_string());
    assert_eq!(reply.code.as_deref(), Some("STALE_TARGET"), "{reply:?}");
    assert_eq!(reply.delivery, Some(Delivery::Refused));
    assert_eq!(window.kernel().checked_of("plan-option-team"), Some(true));
    window.tap("before").unwrap();
    window.press_key("tab").unwrap();
    assert!(focused(&window, "plan-option-team"));
    window.press_key("right").unwrap();
    assert!(focused(&window, "plan-option-business"), "recovery restores navigation and skips disabled radios");
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("business"));
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.app().diagnostics().iter().all(|d| d.code.id == "DUPLICATE_KEY"), "{:?}", window.app().diagnostics());
}

#[test]
fn external_radio_values_disabled_changes_and_missing_ids_do_not_force_fallbacks() {
    let mut window = Window::headless(plans);
    window.invoke("radios.business").unwrap();
    assert_eq!(window.kernel().checked_of("plan-option-business"), Some(true));
    window.tap("before").unwrap();
    window.press_key("tab").unwrap();
    assert!(focused(&window, "plan-option-business"));
    window.press_key("tab").unwrap();
    assert!(focused(&window, "after"));
    window.invoke("radios.clear").unwrap();
    window.tap("before").unwrap();
    window.press_key("tab").unwrap();
    assert!(focused(&window, "plan-option-business"), "an unchosen group remembers its enabled focus entry");
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("none"), "returning focus does not choose");
    window.tap("plan-option-team").unwrap();
    window.invoke("radios.disable-team").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("team"));
    assert_eq!(window.kernel().checked_of("plan-option-team"), Some(true));
    window.tap("before").unwrap();
    window.press_key("tab").unwrap();
    assert!(focused(&window, "plan-option-basic"), "a disabled checked item cannot be a Tab entry");
    window.invoke("radios.remove-team").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("team"));
    for id in ["basic", "legacy", "business"] {
        assert_eq!(window.kernel().checked_of(&format!("plan-option-{id}")), Some(false));
    }
    window.invoke("radios.clear").unwrap();
    window.invoke("radios.disable-all").unwrap();
    window.tap("before").unwrap();
    window.press_key("tab").unwrap();
    assert!(focused(&window, "after"), "an entirely disabled group has no Tab stop");
    window.invoke("radios.empty").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("none"));
    clean(&window);
}

#[test]
fn radios_use_ids_even_when_labels_match_and_support_getter_setter_models() {
    let mut window = Window::headless(|| {
        let chosen = state("chosen", || None::<String>);
        col()
            .size_full()
            .p_4()
            .child(radio_group("place", "Place", (move || chosen.get(), move |value| chosen.set(value)), || {
                vec![RadioOption::new("one", "Springfield"), RadioOption::new("two", "Springfield")]
            }))
            .child(text(move || chosen.get().unwrap_or_else(|| "none".into())).id("chosen"))
            .into()
    });
    window.press_key("tab").unwrap();
    assert!(focused(&window, "place-option-one"));
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("none"), "entering an unchosen group does not select");
    window.tap("place-option-two").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("two"));
    assert_eq!(window.kernel().checked_of("place-option-one"), Some(false));
    assert_eq!(window.kernel().checked_of("place-option-two"), Some(true));
    clean(&window);
}

#[test]
fn radios_in_a_nested_modal_choose_inside_it_and_restore_focus_when_it_closes() {
    let mut window = Window::headless(|| {
        let dialog = state("dialog", || false);
        let popup = state("popup", || false);
        let value = state("value", || Some("team".to_owned()));
        col()
            .size_full()
            .p_4()
            .child(button("Open dialog").id("open-dialog").on_press(move || dialog.set(true)))
            .child(show(
                move || dialog.get(),
                move || {
                    col()
                        .id("dialog")
                        .role(Role::Dialog)
                        .label("Settings")
                        .absolute()
                        .left(40.0)
                        .top(40.0)
                        .w(320.0)
                        .h(400.0)
                        .p_4()
                        .bg(Token::Surface)
                        .modal(true, move || dialog.set(false))
                        .child(popover("more", popup, Placement::BELOW, button("Choose plan"), move || {
                            col().w(240.0).child(radio_group("plan", "Plan", value, radio_options)).into()
                        }))
                        .child(button("Close").id("close-dialog").on_press(move || dialog.set(false)))
                        .into()
                },
            ))
            .into()
    });
    window.tap("open-dialog").unwrap();
    window.tap("more").unwrap();
    assert!(focused(&window, "plan-option-team"));
    assert!(window.tap("close-dialog").is_err());
    window.press_key("down").unwrap();
    assert_eq!(window.kernel().checked_of("plan-option-business"), Some(true));
    window.press_key("tab").unwrap();
    assert!(focused(&window, "plan-option-business"), "Tab stays in the inner modal");
    window.press_key("escape").unwrap();
    assert!(!window.kernel().exists("more-popover"));
    assert!(window.kernel().exists("dialog"));
    assert!(focused(&window, "more"));
    window.press_key("escape").unwrap();
    assert!(!window.kernel().exists("dialog"));
    assert!(focused(&window, "open-dialog"));
    clean(&window);
}

#[test]
fn fields_and_radios_wrap_in_narrow_layouts_in_both_themes() {
    for width in [240.0, 390.0, 800.0] {
        for theme in [Theme::light(), Theme::dark()] {
            let mut window = Window::headless(|| {
                let name = state("name", String::new);
                let chosen = state("chosen", || None::<String>);
                col()
                    .size_full()
                    .p_4()
                    .gap_3()
                    .child(field("name", "Full name", text_field(name).w_full(), || FieldInfo {
                        description: "Your name appears on shared projects.".into(),
                        error: Some("Enter the name you want people to see.".into()),
                        required: true,
                    }))
                    .child(radio_group("plan", "Plan", chosen, || {
                        vec![
                            RadioOption::new("team", "Work together on a shared project")
                                .description("Invite the people who work with you."),
                        ]
                    }))
                    .into()
            });
            window.resize(width, 844.0);
            window.app().set_theme(theme);
            clean(&window);
        }
    }
}
