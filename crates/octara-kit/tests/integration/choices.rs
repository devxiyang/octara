//! Modal popovers and searchable choices, driven through the public framework events.

use octara::app::{Event, InputEvent};
use octara::prelude::*;
use octara_kit::*;

fn options() -> Vec<Choice> {
    vec![
        Choice::new("amber", "Amber"),
        Choice::new("berry", "Berry"),
        Choice::new("blue", "Blue").disabled(true),
        Choice::new("coral", "Coral"),
    ]
}

#[ui]
fn choices() -> View {
    let value = state("value", || Some("berry".to_owned()));
    let options = state("options", options);
    on_command("choices.reverse", move || options.update(|options| options.reverse()));
    on_command("choices.rename", move || {
        options.update(|options| options.iter_mut().find(|o| o.id == "berry").unwrap().label = "Blueberry".into())
    });
    on_command("choices.remove-berry", move || options.update(|options| options.retain(|option| option.id != "berry")));
    on_command("choices.remove-coral", move || options.update(|options| options.retain(|option| option.id != "coral")));
    on_command("choices.disable-all", move || options.update(|options| options.iter_mut().for_each(|option| option.disabled = true)));
    on_command("choices.empty", move || options.set(vec![]));
    on_command("choices.insert", move || options.update(|options| options.insert(0, Choice::new("apple", "Apple"))));
    on_command("choices.select-coral", move || value.set(Some("coral".to_owned())));
    on_command("choices.clear", move || value.set(None));
    col()
        .size_full()
        .p_4()
        .gap_3()
        .child(combobox("color", "Color", value, move || options.get(), button))
        .child(text(move || value.get().unwrap_or_else(|| "none".into())).id("chosen"))
        .child(button("Outside").id("outside"))
        .into()
}

fn focused(window: &Window, id: &str) -> bool {
    window.kernel().state_of(id).is_some_and(|state| state.contains(State::FOCUSED))
}

fn selected(window: &Window, id: &str) -> bool {
    window.kernel().state_of(id).is_some_and(|state| state.contains(State::SELECTED))
}

fn active(window: &Window) -> Option<String> {
    window.inspect().find("color-search").unwrap().active_descendant.clone()
}

fn clean(window: &Window) {
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

#[test]
fn combobox_keeps_search_focused_and_selection_separate_from_keyboard_highlight() {
    let mut window = Window::headless(choices);
    assert_eq!(window.kernel().role_of("color"), Some(Role::ComboBox));
    assert_eq!(window.kernel().value_of("color").as_deref(), Some("Berry"));
    window.tap("color").unwrap();
    assert_eq!(window.kernel().expanded_of("color"), Some(true));
    assert_eq!(window.kernel().role_of("color-list"), Some(Role::ListBox));
    assert!(focused(&window, "color-search"));
    assert_eq!(active(&window).as_deref(), Some("color-option-berry"));
    window.press_key("down").unwrap();
    assert_eq!(active(&window).as_deref(), Some("color-option-coral"), "disabled Blue is skipped");
    assert!(focused(&window, "color-search"));
    assert!(selected(&window, "color-option-berry"));
    assert!(!selected(&window, "color-option-coral"), "highlight is not a new selection");
    let tree = window.accessibility_tree();
    let search = octara::kernel::a11y::a11y_id(window.kernel().find("color-search").unwrap());
    let coral = octara::kernel::a11y::a11y_id(window.kernel().find("color-option-coral").unwrap());
    assert_eq!(tree.nodes.iter().find(|(id, _)| *id == search).unwrap().1.active_descendant(), Some(coral));
    window.press_key("up").unwrap();
    assert_eq!(active(&window).as_deref(), Some("color-option-berry"));
    window.press_key("down").unwrap();
    window.press_key("enter").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("coral"));
    assert!(!window.kernel().exists("color-popover"));
    assert!(focused(&window, "color"));
    clean(&window);
}

#[test]
fn search_handles_case_disabled_only_and_empty_results_then_resets_on_reopen() {
    let mut window = Window::headless(choices);
    window.tap("color").unwrap();
    let search_target = window.inspect().find("color-search").unwrap().target.clone();
    let row_target = window.inspect().find("color-option-berry").unwrap().target.clone();
    window.type_text("BE");
    assert!(window.kernel().exists("color-option-amber"));
    assert!(window.kernel().exists("color-option-berry"));
    assert!(!window.kernel().exists("color-option-coral"));
    assert_eq!(active(&window).as_deref(), Some("color-option-amber"), "a query starts at its first enabled match");
    assert_eq!(window.inspect().find("color-search").unwrap().target, search_target);
    window.dispatch(Event::Input("color-search".into(), "BL".into())).unwrap();
    assert!(window.kernel().exists("color-option-blue"), "disabled matches remain visible");
    assert!(!window.kernel().exists("color-empty"));
    assert_eq!(active(&window), None);
    window.tap("color-option-blue").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("berry"));
    assert!(window.kernel().exists("color-popover"));
    assert!(window.dispatch(Event::Press("color-option-blue".into())).is_err());
    window.press_key("down").unwrap();
    window.press_key("enter").unwrap();
    assert!(window.kernel().exists("color-popover"), "no enabled choice cannot be picked");
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("berry"));
    window.dispatch(Event::Input("color-search".into(), "missing".into())).unwrap();
    assert!(window.kernel().exists("color-empty"));
    assert_eq!(active(&window), None);
    window.press_key("up").unwrap();
    window.press_key("enter").unwrap();
    window.dispatch(Event::Input("color-search".into(), String::new())).unwrap();
    let restored = window.inspect();
    assert_eq!(restored.find("color-search").unwrap().target, search_target);
    assert_ne!(restored.find("color-option-berry").unwrap().target, row_target);
    let request = octara::serde_json::json!({"op": "tap", "id": "color-option-berry", "target": row_target});
    let reply = octara::inspect::handle_line(&mut window, &request.to_string());
    assert_eq!(reply.code.as_deref(), Some("STALE_TARGET"));
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("berry"));
    window.dispatch(Event::Input("color-search".into(), "missing".into())).unwrap();
    window.press_key("escape").unwrap();
    assert!(!window.kernel().exists("color-popover"));
    window.press_key("enter").unwrap();
    assert_eq!(window.kernel().value_of("color-search").as_deref(), Some(""));
    assert_eq!(active(&window).as_deref(), Some("color-option-berry"));
    clean(&window);
}

#[test]
fn option_reordering_updates_labels_and_order_without_replacing_retained_rows_or_search() {
    let mut window = Window::headless(choices);
    window.tap("color").unwrap();
    let before = window.inspect();
    let berry = before.find("color-option-berry").unwrap().target.clone();
    let search = before.find("color-search").unwrap().target.clone();
    window.invoke("choices.reverse").unwrap();
    let after = window.inspect();
    assert_eq!(after.find("color-option-berry").unwrap().target, berry);
    assert_eq!(after.find("color-search").unwrap().target, search);
    let list = after.find("color-list").unwrap();
    let ids: Vec<_> = list.children.iter().filter_map(|node| node.id.as_deref()).collect();
    assert_eq!(ids, ["color-option-coral", "color-option-blue", "color-option-berry", "color-option-amber"]);
    assert_eq!(active(&window).as_deref(), Some("color-option-berry"));
    window.invoke("choices.rename").unwrap();
    assert_eq!(window.kernel().value_of("color").as_deref(), Some("Blueberry"));
    assert_eq!(window.kernel().text_of("color-option-berry").as_deref(), Some("Blueberry"));
    assert_eq!(window.inspect().find("color-option-berry").unwrap().target, berry);
    window.invoke("choices.insert").unwrap();
    assert!(window.kernel().exists("color-option-apple"));
    assert_eq!(window.inspect().find("color-option-berry").unwrap().target, berry);
    clean(&window);
}

#[test]
fn external_values_option_removal_and_disabling_are_reconciled_while_open() {
    let mut window = Window::headless(choices);
    window.tap("color").unwrap();
    window.invoke("choices.select-coral").unwrap();
    assert!(selected(&window, "color-option-coral"));
    assert!(!selected(&window, "color-option-berry"));
    assert_eq!(active(&window).as_deref(), Some("color-option-berry"), "external selection does not move a valid keyboard highlight");
    window.invoke("choices.remove-berry").unwrap();
    assert_eq!(active(&window).as_deref(), Some("color-option-coral"));
    window.invoke("choices.remove-coral").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("coral"), "the model remains owned by the caller");
    assert_eq!(window.kernel().value_of("color").as_deref(), Some("Color"));
    assert_eq!(active(&window).as_deref(), Some("color-option-amber"));
    window.invoke("choices.disable-all").unwrap();
    assert_eq!(active(&window), None);
    window.press_key("enter").unwrap();
    assert!(window.kernel().exists("color-popover"));
    window.invoke("choices.clear").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("none"));
    window.invoke("choices.empty").unwrap();
    assert!(window.kernel().exists("color-empty"));
    clean(&window);
}

#[test]
fn source_updates_preserve_search_selection_undo_and_an_active_input_method() {
    let mut window = Window::headless(choices);
    window.tap("color").unwrap();
    window.type_text("be");
    let search = window.inspect().find("color-search").unwrap().target.clone();
    window.set_selection(0, 2);
    window.event(InputEvent::Preedit { text: "候选".into(), selection: Some((0, 6)) });
    let before = window.text_field().unwrap();
    window.invoke("choices.reverse").unwrap();
    window.invoke("choices.rename").unwrap();
    let after = window.text_field().unwrap();
    assert_eq!(window.inspect().find("color-search").unwrap().target, search);
    assert_eq!(after.composition, before.composition);
    assert_eq!(after.composition_selection, before.composition_selection);
    assert!(focused(&window, "color-search"));
    window.press_key("enter").unwrap();
    window.press_key("escape").unwrap();
    assert!(window.kernel().exists("color-popover"), "composition owns Enter and Escape before the chooser");
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("berry"));
    window.event(InputEvent::Preedit { text: String::new(), selection: None });
    assert_eq!(window.text_field().unwrap().selection, 0..2);
    window.press_key("mod+z").unwrap();
    assert_eq!(window.kernel().value_of("color-search").as_deref(), Some(""), "source reconciliation preserves the editor's undo history");
    window.press_key("escape").unwrap();
    clean(&window);
}

#[test]
fn modified_navigation_and_text_editing_keys_leave_the_active_option_alone() {
    let mut window = Window::headless(choices);
    window.tap("color").unwrap();
    window.type_text("be");
    let before = active(&window);
    for chord in ["home", "end", "shift+up", "shift+down", "mod+left", "mod+right"] {
        window.press_key(chord).unwrap();
        assert_eq!(active(&window), before, "{chord} belongs to the editor");
        assert!(focused(&window, "color-search"));
    }
    window.press_key("escape").unwrap();
    clean(&window);
}

#[test]
fn active_rows_scroll_into_view_without_moving_search_focus() {
    let mut window = Window::headless(|| {
        let value = state("value", || Some("item-75".to_owned()));
        col().size_full().p_4().child(combobox(
            "color",
            "Item",
            value,
            || (0..120).map(|index| Choice::new(format!("item-{index}"), format!("Item {index}"))).collect(),
            button,
        ))
    });
    window.tap("color").unwrap();
    assert_eq!(active(&window).as_deref(), Some("color-option-item-75"));
    assert!(focused(&window, "color-search"));
    {
        let kernel = window.kernel();
        let list = kernel.find("color-list").unwrap();
        let row = kernel.find("color-option-item-75").unwrap();
        let (_, list_y, _, list_h) = kernel.shown_rect(list);
        let (_, row_y, _, row_h) = kernel.shown_rect(row);
        assert!(kernel.scroll_offset(list).1 > 0.0);
        assert!(row_y >= list_y && row_y + row_h <= list_y + list_h + 0.1, "the active row must be visible");
    }
    for _ in 0..12 {
        window.press_key("down").unwrap();
    }
    assert_eq!(active(&window).as_deref(), Some("color-option-item-87"));
    assert!(focused(&window, "color-search"));
    clean(&window);
}

#[test]
fn a_standalone_popover_is_controlled_modal_and_restores_previous_focus() {
    let mut window = Window::headless(|| {
        let open = state("open", || false);
        let name = state("name", String::new);
        on_command("popup.open", move || open.set(true));
        col().size_full().p_4().gap_3().child(button("Outside").id("outside")).child(popover(
            "details",
            open,
            Placement::BELOW,
            button("Details"),
            move || text_field(name).id("details-name").label("Name").autofocus(),
        ))
    });
    window.tap("outside").unwrap();
    window.invoke("popup.open").unwrap();
    assert!(focused(&window, "details-name"));
    assert_eq!(window.kernel().expanded_of("details"), Some(true));
    assert!(window.tap("outside").is_err(), "modal popup makes the window behind inert");
    window.press_key("escape").unwrap();
    assert!(focused(&window, "outside"), "an externally opened popup restores the previous focus");
    window.tap("details").unwrap();
    assert!(focused(&window, "details-name"));
    window.click_at(790.0, 590.0);
    assert!(!window.kernel().exists("details-popover"));
    assert!(focused(&window, "details"));
    clean(&window);
}

#[test]
fn a_combobox_inside_a_dialog_dismisses_only_the_inner_modal() {
    let mut window = Window::headless(|| {
        let dialog = state("dialog", || false);
        let value = state("value", || Some("berry".to_owned()));
        col().size_full().p_4().child(button("Open dialog").id("open-dialog").on_press(move || dialog.set(true))).child(show(
            move || dialog.get(),
            move || {
                col()
                    .id("dialog")
                    .role(Role::Dialog)
                    .label("Choose color")
                    .absolute()
                    .left(40.0)
                    .top(80.0)
                    .w(320.0)
                    .h(320.0)
                    .p_4()
                    .gap_3()
                    .bg(Token::Surface)
                    .modal(true, move || dialog.set(false))
                    .child(combobox("color", "Color", value, options, button))
                    .child(button("Close").id("close-dialog").on_press(move || dialog.set(false)))
            },
        ))
    });
    window.tap("open-dialog").unwrap();
    window.tap("color").unwrap();
    assert!(focused(&window, "color-search"));
    assert!(window.tap("close-dialog").is_err(), "the outer dialog is inert while the popup is open");
    window.press_key("escape").unwrap();
    assert!(window.kernel().exists("dialog"));
    assert!(!window.kernel().exists("color-popover"));
    assert!(focused(&window, "color"));
    window.press_key("enter").unwrap();
    window.press_key("down").unwrap();
    window.press_key("enter").unwrap();
    assert!(window.kernel().exists("dialog"));
    assert_eq!(window.kernel().value_of("color").as_deref(), Some("Coral"));
    assert!(focused(&window, "color"));
    window.press_key("escape").unwrap();
    assert!(!window.kernel().exists("dialog"));
    assert!(focused(&window, "open-dialog"));
    clean(&window);
}

#[test]
fn combobox_popup_is_clean_at_phone_and_desktop_sizes_in_both_themes() {
    for (width, height) in [(390.0, 844.0), (800.0, 600.0)] {
        for theme in [Theme::light(), Theme::dark()] {
            let mut window = Window::headless(choices);
            window.resize(width, height);
            window.app().set_theme(theme);
            window.tap("color").unwrap();
            window.type_text("b");
            clean(&window);
            window.press_key("escape").unwrap();
            clean(&window);
        }
    }
}
