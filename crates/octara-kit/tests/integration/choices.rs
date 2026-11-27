use super::*;
use octara_kit::headless::{Choice, Collection, ComboboxState, Selection};

fn btn(label: impl IntoText) -> El {
    button(label, octara_kit::Size::Md, ButtonVariant::Secondary)
}

fn active_option(mounted: &Mounted, control: &str) -> Option<NodeId> {
    let semantics = mounted.tree().semantics(mounted.node(control)).unwrap();
    let name = semantics.relations.active_descendant.as_deref()?;
    mounted.tree().named_nodes(name).first().copied()
}

#[test]
fn nested_dialog_and_popover_restore_each_openers_focus() {
    let mut driver = HeadlessDriver::default();
    let open = driver.app_mut().build(|b| b.signal(false)).unwrap();
    let mut mounted =
        Mounted::new(driver, move |_| {
            col().child(btn("Open").test_id("open").on_press(move |w| w.set(open, true))).child(dialog(
                "dialog",
                "Dialog",
                open.into(),
                DialogOptions { description: "Description".into(), click_outside: false },
                |b| {
                    let shown = b.signal(false);
                    let draft = b.signal("Draft".to_owned());
                    popover(b, "choice", shown.into(), PopupOptions::default(), btn("More").autofocus(), move |_| {
                        col()
                            .child(text_field(draft.into()).test_id("editor").label("Editor").autofocus())
                            .child(btn("Portal owner").overlay(Placement::RIGHT, |_| true, |_| btn("Nested portal").test_id("nested")))
                    })
                },
            ))
        });
    assert!(mounted.a11y("open", A11yAction::Focus));
    assert!(mounted.a11y("open", A11yAction::Press));
    mounted.focus_is("choice");
    assert_eq!(mounted.tree().front_modal(), Some(mounted.node("dialog")));
    assert!(mounted.a11y("choice", A11yAction::Press));
    mounted.focus_is("editor");
    assert_eq!(mounted.tree().front_modal(), Some(mounted.node("dialog")));
    assert!(mounted.tree().is_in_interaction_scope(mounted.node("nested"), mounted.node("choice-popover")));
    assert!(mounted.a11y("nested", A11yAction::Focus));
    mounted.focus_is("nested");
    assert!(mounted.a11y("editor", A11yAction::Focus));
    mounted.mark("日");
    assert!(!mounted.key(Key::Escape));
    mounted.commit("日本");
    assert!(mounted.key(Key::Escape));
    mounted.focus_is("choice");
    assert!(mounted.key(Key::Escape));
    mounted.focus_is("open");
    assert!(mounted.tree().test_ids("dialog").is_empty());
}

#[test]
fn popup_tracks_trigger_width_and_keeps_the_callers_geometry_observer() {
    let mut driver = HeadlessDriver::default();
    let (open, width, observed) = driver.app_mut().build(|b| (b.signal(false), b.signal(122.0), b.signal(None))).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        popover(
            b,
            "popup",
            open.into(),
            PopupOptions::default()
                .placement(Placement::RIGHT)
                .match_trigger_width(true)
                .max_height(80)
                .surface_style(StylePatch::default().bg(Color::BLACK)),
            btn("More").style_with(move |r, p| p.w(r.get(width))).observe_geometry(observed),
            |_| col().h(200).shrink(0.0),
        )
    });
    assert!(mounted.a11y("popup", A11yAction::Press));
    let panel = mounted.node("popup-popover");
    assert_eq!(mounted.tree().overlay(panel).unwrap().placed.unwrap().side, Side::Right);
    assert_eq!(mounted.tree().visual(panel).background, Some(Color::BLACK));
    assert_eq!(mounted.window().geometry(panel).unwrap().rect.w, 122.0);
    assert_eq!(mounted.window().geometry(panel).unwrap().rect.h, 80.0);
    mounted.update(|w| w.set(width, 232.0));
    assert_eq!(mounted.update(|w| w.get(observed)).unwrap().rect.w, 232.0);
    assert_eq!(mounted.window().geometry(panel).unwrap().rect.w, 232.0);
    assert_eq!(mounted.node("popup-popover"), panel);
}

#[test]
fn outside_press_dismisses_only_the_popover_and_focuses_the_hit_control() {
    let mut driver = HeadlessDriver::default();
    let (dialog_open, popover_open) = driver.app_mut().build(|b| (b.signal(true), b.signal(false))).unwrap();
    let mut mounted = Mounted::new(driver, move |_| {
        dialog("dialog", "Dialog", dialog_open.into(), DialogOptions::default(), move |_| {
            col()
                .child(btn("Open popup").test_id("trigger").autofocus().on_press(move |w| w.set(popover_open, true)).overlay(
                    Placement::RIGHT,
                    move |r| r.get(popover_open),
                    move |_| {
                        col()
                            .layer(
                                LayerPolicy {
                                    focus_trap: FocusTrapFeatures::INITIAL_FOCUS
                                        | FocusTrapFeatures::RESTORE_FOCUS
                                        | FocusTrapFeatures::FOCUS_LOCK,
                                    ..LayerPolicy::non_modal()
                                },
                                move |w| w.set(popover_open, false),
                            )
                            .child(btn("Panel control").test_id("inside"))
                    },
                ))
                .child(btn("Other control").test_id("external"))
        })
    });
    assert!(mounted.a11y("trigger", A11yAction::Press));
    mounted.focus_is("inside");

    let hit = mounted.tree().guard(mounted.node("external"));
    let host_focus = Input::Host(HostReply::FocusChanged { target: Some(hit), visible: false });
    assert!(mounted.driver.dispatch(mounted.id, host_focus).unwrap().handled);
    mounted.focus_is("inside");

    let input = InteractionEvent::PointerDown {
        id: PointerId(1),
        kind: PointerKind::Mouse,
        target: octara::ContactTarget { hit, action: Some(hit) },
        pos: Point::default(),
        button: Button::Primary,
        mods: Modifiers::empty(),
    };
    assert!(mounted.driver.dispatch(mounted.id, Input::Interaction(input)).unwrap().handled);
    mounted.focus_is("external");
    assert!(!mounted.update(|w| w.get(popover_open)));
    assert!(mounted.update(|w| w.get(dialog_open)));
}

#[test]
fn grouped_popovers_keep_open_on_focus_to_a_sibling_trigger() {
    let mut driver = HeadlessDriver::default();
    let open = driver.app_mut().build(|b| b.signal(true)).unwrap();
    let group = LayerGroupId::new();
    let mut mounted = Mounted::new(driver, move |_| {
        col()
            .child(btn("First trigger").test_id("first").layer_group(group).overlay(
                Placement::RIGHT,
                move |r| r.get(open),
                move |_| {
                    btn("Panel control")
                        .test_id("panel")
                        .layer(LayerPolicy { group: Some(group), ..LayerPolicy::non_modal() }, move |w| w.set(open, false))
                },
            ))
            .child(btn("Second trigger").test_id("second").layer_group(group))
            .child(btn("Other control").test_id("external"))
    });
    assert!(mounted.a11y("panel", A11yAction::Focus));
    assert!(mounted.a11y("second", A11yAction::Focus));
    assert!(mounted.update(|w| w.get(open)));
    assert!(mounted.tree().is_in_interaction_scope(mounted.node("second"), mounted.node("panel")));
    assert!(!mounted.tree().is_logically_owned_by(mounted.node("second"), mounted.node("panel")));
    assert!(mounted.a11y("external", A11yAction::Focus));
    mounted.focus_is("external");
    assert!(!mounted.update(|w| w.get(open)));
}

#[test]
fn combobox_retains_rows_and_search_while_tracking_the_active_option() {
    let mut driver = HeadlessDriver::default();
    let (selected, options) = driver
        .app_mut()
        .build(|b| {
            (
                b.signal(Some("a".to_owned())),
                b.list_from([
                    ("a".to_owned(), Choice::new("Alpha")),
                    ("b".to_owned(), Choice::new("Beta").disabled(true)),
                    ("c".to_owned(), Choice::new("Cat")),
                ]),
            )
        })
        .unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        b.provide(KitLocale::zh_cn());
        combobox(
            "pick",
            "Pick",
            Selection::Single(selected.into()),
            options.read_only(),
            ComboboxOptions::default()
                .popup(PopupOptions::default().width(210))
                .placeholder("Find a choice")
                .trigger_style(StylePatch::default().w(120))
                .input_style(StylePatch::default().text_size(16.0)),
            btn,
            choice_row,
        )
    });
    assert!(mounted.a11y("pick", A11yAction::Focus));
    assert!(mounted.a11y("pick", A11yAction::Press));
    mounted.focus_is("pick-search");
    let search = mounted.node("pick-search");
    assert_eq!(mounted.window().geometry(mounted.node("pick")).unwrap().rect.w, 120.0);
    assert_eq!(mounted.window().geometry(mounted.node("pick-popover")).unwrap().rect.w, 210.0);
    assert_eq!(mounted.tree().semantics(search).unwrap().label.as_deref(), Some("搜索Pick"));
    assert_eq!(&*mounted.tree().editor(search).unwrap().placeholder, "Find a choice");
    assert_eq!(mounted.tree().visual(search).font_size, 16.0);
    let retained = mounted.node("pick-option-a");
    assert_eq!(active_option(&mounted, "pick-search"), Some(mounted.node("pick-option-a")));
    assert!(mounted.key(Key::Down));
    assert_eq!(active_option(&mounted, "pick-search"), Some(mounted.node("pick-option-c")));
    assert_eq!(mounted.update(|w| w.get(selected)).as_deref(), Some("a"));
    mounted.update(|w| w.move_row(options, &"c".to_owned(), 0));
    assert_eq!(mounted.node("pick-search"), search);
    assert_eq!(mounted.node("pick-option-a"), retained);
    mounted.update(|w| w.update_row(options, &"c".to_owned(), |choice| choice.disabled = true));
    assert_eq!(active_option(&mounted, "pick-search"), None);
    mounted.commit("ALP");
    assert!(mounted.tree().test_ids("pick-option-c").is_empty());
    assert!(mounted.key(Key::Enter));
    mounted.focus_is("pick");
    assert_eq!(mounted.update(|w| w.get(selected)).as_deref(), Some("a"));
    assert!(mounted.a11y("pick", A11yAction::Press));
    assert_eq!(mounted.tree().editor(mounted.node("pick-search")).unwrap().value(), "");
}

#[test]
fn select_reveals_focused_options_outside_the_initial_window() {
    let mut driver = HeadlessDriver::default();
    let selected = driver.app_mut().build(|b| b.signal(Some("0".to_owned()))).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        let options = b.list_from((0..180).map(|i| (i.to_string(), Choice::new(format!("Choice {i}")))));
        select(
            "pick",
            "Pick",
            Selection::Single(selected.into()),
            options.read_only(),
            PickerOptions::default(),
            PopupOptions::default().width(120).min_width(140).max_height(180),
            btn,
            choice_row,
        )
    });
    assert!(mounted.a11y("pick", A11yAction::Press));
    mounted.focus_is("pick-list");
    let popup = mounted.window().geometry(mounted.node("pick-popover")).unwrap().rect;
    assert_eq!(popup.w, 140.0);
    assert_eq!(popup.h, 180.0);
    assert!(mounted.tree().test_ids("pick-option-179").is_empty());
    assert!(mounted.key(Key::End));
    assert_eq!(active_option(&mounted, "pick-list"), Some(mounted.node("pick-option-179")));
    assert!(mounted.tree().scroll(mounted.node("pick-list")).unwrap().offset.y > 100.0);
    mounted.node("pick-option-179");
    assert!(mounted.key(Key::Enter));
    assert_eq!(mounted.update(|w| w.get(selected)).as_deref(), Some("179"));
}

#[test]
fn inline_combobox_respects_editor_composition_and_preserves_submit_callbacks() {
    let mut driver = HeadlessDriver::default();
    let (draft, chosen, submits) =
        driver.app_mut().build(|b| (b.signal(String::new()), b.signal(None::<String>), b.signal(0_u32))).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        let suggestions = b.list_from([
            ("a".to_owned(), Choice::new("Alpha").disabled(true)),
            ("b".to_owned(), Choice::new("Beta")),
            ("c".to_owned(), Choice::new("Cat")),
        ]);
        let collection = Collection::choices(b, suggestions);
        let available = b.memo(|_| true);
        let state = ComboboxState::with_collection(b, collection, Selection::Single(chosen.into()), draft, available);
        combobox_input(
            b,
            "draft",
            "Suggestions",
            state,
            input().label("Draft").autofocus().submit_on_enter().on_submit(move |w| w.update(submits, |n| *n += 1)),
            ComboboxOptions::default().picker(PickerOptions::Standard),
            choice_row,
        )
    });
    mounted.focus_is("draft");
    assert!(mounted.key(Key::Down));
    assert_eq!(active_option(&mounted, "draft"), Some(mounted.node("draft-option-b")));
    mounted.mark("日");
    assert!(!mounted.key(Key::Down));
    assert!(mounted.a11y("draft-option-c", A11yAction::Press));
    assert_eq!(mounted.update(|w| w.get(chosen)), None);
    mounted.commit("日本");
    assert!(mounted.key(Key::Down));
    assert!(mounted.key(Key::Enter));
    mounted.focus_is("draft");
    assert_eq!(mounted.update(|w| (w.get(chosen), w.get(submits))), (Some("c".into()), 0));
    assert!(mounted.key(Key::Enter));
    assert_eq!(mounted.update(|w| (w.get(draft), w.get(submits))), ("日本".into(), 1));
}

fn action(id: &str, label: &str) -> MenuItem {
    MenuItem::Action { id: id.into(), label: label.into(), enabled: true, checked: None, shortcut: None }
}

#[test]
fn command_menu_runs_after_focus_restoration() {
    let mut driver = HeadlessDriver::default();
    let (open, draft, fired) = driver.app_mut().build(|b| (b.signal(false), b.signal(String::new()), b.signal(false))).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        col().child(text_field(draft.into()).test_id("editor").label("Editor").on_command("copy", move |w| w.set(fired, true))).child(
            menu_button(b, "actions", open.into(), PopupOptions::default(), btn("Actions"), |_| Menu {
                name: "Actions".into(),
                items: vec![MenuItem::Submenu { menu: Menu { name: "Edit".into(), items: vec![action("copy", "Copy")] }, enabled: true }],
            }),
        )
    });
    assert!(mounted.a11y("editor", A11yAction::Focus));
    assert!(mounted.a11y("actions", A11yAction::Press));
    mounted.focus_is("actions-submenu-0");
    assert!(mounted.key(Key::Right));
    mounted.focus_is("actions-submenu-0-action-copy");
    assert!(mounted.key(Key::Left));
    mounted.focus_is("actions-submenu-0");
    assert!(mounted.key(Key::Right));
    assert!(mounted.key(Key::Enter));
    mounted.focus_is("editor");
    assert!(mounted.update(|w| w.get(fired)));
    assert!(!mounted.update(|w| w.get(open)));
}

#[test]
fn menu_typeahead_reads_one_snapshot_per_opening() {
    let mut driver = HeadlessDriver::default();
    let (open, label) = driver.app_mut().build(|b| (b.signal(false), b.signal("Copy".to_owned()))).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        menu_button(b, "actions", open.into(), PopupOptions::default(), btn("Actions"), move |r| Menu {
            name: "Actions".into(),
            items: vec![action("open", "Open"), action("copy", &r.get(label)), action("cut", "Cut")],
        })
    });
    assert!(mounted.a11y("actions", A11yAction::Focus));
    assert!(mounted.key(Key::Up));
    mounted.focus_is("actions-action-cut");
    assert!(mounted.key(Key::Char('c')));
    mounted.focus_is("actions-action-copy");
    assert!(mounted.key(Key::Char('u')));
    mounted.focus_is("actions-action-cut");
    mounted.update(|w| w.set(label, "Duplicate".into()));
    let row = mounted.node("actions-action-copy");
    assert_eq!(mounted.tree().semantics(row).unwrap().label.as_deref(), Some("Copy"));
    assert!(mounted.key(Key::Escape));
    mounted.update(|w| w.set(open, true));
    mounted.focus_is("actions-action-open");
    let refreshed = mounted.node("actions-action-copy");
    assert_ne!(refreshed, row);
    assert_eq!(mounted.tree().semantics(refreshed).unwrap().label.as_deref(), Some("Duplicate"));
    assert!(mounted.key(Key::Escape));
    assert!(mounted.key(Key::Down));
    mounted.focus_is("actions-action-open");
}

#[test]
fn menu_aligns_checked_unchecked_and_plain_actions() {
    let mut driver = HeadlessDriver::default();
    let open = driver.app_mut().build(|b| b.signal(true)).unwrap();
    let mounted = Mounted::new(driver, move |b| {
        menu_button(b, "actions", open.into(), PopupOptions::default().width(130), btn("Actions"), |_| Menu {
            name: "Actions".into(),
            items: vec![
                MenuItem::Action { id: "checked".into(), label: "Checked".into(), enabled: true, checked: Some(true), shortcut: None },
                MenuItem::Action { id: "unchecked".into(), label: "Unchecked".into(), enabled: true, checked: Some(false), shortcut: None },
                action("plain", "Plain"),
            ],
        })
    });
    let tree = mounted.tree();
    assert_eq!(mounted.window().geometry(mounted.node("actions-menu")).unwrap().rect.w, 130.0);
    for (id, checked) in [("checked", Some(true)), ("unchecked", Some(false)), ("plain", None)] {
        let row = mounted.node(&format!("actions-action-{id}"));
        let [mark, label] = tree.children(row) else { panic!("expected a mark slot and label") };
        assert_eq!(tree.icon(*mark).is_some(), checked == Some(true));
        assert_eq!(mounted.window().geometry(*mark).unwrap().rect.w, 14.0);
        assert_eq!(mounted.window().geometry(*label).unwrap().rect.x, 30.0);
        let expected = if checked.is_some() { Role::MenuItemCheckbox } else { Role::MenuItem };
        assert_eq!(tree.semantics(row).unwrap().role, Some(expected));
    }
}

#[test]
fn multiselect_combobox_keeps_custom_options_open() {
    let mut driver = HeadlessDriver::default();
    let selected = driver.app_mut().build(|b| b.signal(Vec::<u32>::new())).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        let choices = b.list_from([(12, Choice::new("Same")), (84, Choice::new("Same"))]);
        combobox(
            "cities",
            "Cities",
            Selection::Multiple(selected.into()),
            choices.read_only(),
            ComboboxOptions::default().picker(PickerOptions::Standard),
            btn,
            |_, key, _| row().child(text(format!("Code {key}")).test_id(format!("code-{key}"))),
        )
    });
    assert!(mounted.a11y("cities", A11yAction::Focus));
    assert!(mounted.a11y("cities", A11yAction::Press));
    mounted.focus_is("cities-search");
    assert!(mounted.a11y("cities-option-12", A11yAction::Press));
    assert!(mounted.key(Key::Down));
    assert!(mounted.key(Key::Enter));
    assert_eq!(mounted.update(|w| w.get(selected)), vec![12, 84]);
    mounted.node("code-84");
    mounted.focus_is("cities-search");
    assert!(mounted.a11y("cities-option-12", A11yAction::Press));
    assert_eq!(mounted.update(|w| w.get(selected)), vec![84]);
    assert!(mounted.key(Key::Escape));
    mounted.focus_is("cities");
}

#[test]
fn command_palette_searches_and_escapes_as_one_layer() {
    let mut driver = HeadlessDriver::default();
    let (open, chosen) = driver.app_mut().build(|b| (b.signal(false), b.signal(None::<String>))).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        let commands = b.list_from([("copy".to_owned(), Choice::new("Copy")), ("save".to_owned(), Choice::new("Save"))]);
        col().child(btn("Commands").test_id("commands").on_press(move |w| w.set(open, true))).child(command_palette(
            "palette",
            "Command palette",
            open.into(),
            commands.read_only(),
            move |w, key| {
                w.set(chosen, Some(key));
            },
        ))
    });
    assert!(mounted.a11y("commands", A11yAction::Focus));
    assert!(mounted.a11y("commands", A11yAction::Press));
    mounted.focus_is("palette-search");
    assert_eq!(active_option(&mounted, "palette-search"), Some(mounted.node("palette-option-copy")));
    mounted.commit("sa");
    assert_eq!(active_option(&mounted, "palette-search"), Some(mounted.node("palette-option-save")));
    mounted.mark("日");
    assert!(!mounted.key(Key::Enter));
    assert_eq!(mounted.update(|w| w.get(chosen)), None);
    mounted.commit("");
    assert!(mounted.key(Key::Enter));
    assert_eq!(mounted.update(|w| w.get(chosen)), Some("save".into()));
    mounted.focus_is("commands");
    assert!(mounted.a11y("commands", A11yAction::Press));
    assert!(mounted.key(Key::Escape));
    mounted.focus_is("commands");
    assert!(mounted.tree().test_ids("palette").is_empty());
}

#[test]
fn context_menu_uses_keyboard_trigger_and_dispatches_command() {
    let mut driver = HeadlessDriver::default();
    let (open, fired) = driver.app_mut().build(|b| (b.signal(false), b.signal(false))).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        context_menu(
            b,
            "document",
            open.into(),
            PopupOptions::default(),
            btn("Document").on_command("copy", move |w| w.set(fired, true)),
            |_| Menu { name: "Actions".into(), items: vec![action("copy", "Copy")] },
        )
    });
    assert!(mounted.a11y("document", A11yAction::Focus));
    assert!(mounted.key(Key::Menu));
    mounted.focus_is("document-action-copy");
    assert!(mounted.key(Key::Enter));
    mounted.focus_is("document");
    assert!(mounted.update(|w| w.get(fired)));
    assert!(!mounted.update(|w| w.get(open)));
}

#[test]
fn toast_viewport_renders_and_dismisses_queue_items() {
    let mut driver = HeadlessDriver::default();
    let queue = driver.app_mut().build(ToastQueue::<String>::new).unwrap();
    let mut mounted = Mounted::new(driver, move |_| col().size(rel(1.0)).child(toast_viewport("notifications", queue)));
    let first = mounted.update(|w| queue.push(w, "Saved".to_owned(), ToastOptions { live: Live::Assertive, duration: None }));
    let second = mounted.update(|w| queue.push(w, "Updated".to_owned(), ToastOptions::default()));
    assert!(mounted.a11y(&format!("notifications-{first}-dismiss"), A11yAction::Press));
    assert!(!mounted.update(|w| queue.dismiss(w, first)));
    mounted.node(&format!("notifications-{second}"));
    mounted.driver.advance_to(Duration::from_secs(5)).unwrap();
    assert!(mounted.tree().test_ids(&format!("notifications-{second}")).is_empty());
}

#[test]
fn tooltip_on_kit_button_uses_the_shared_window_layer() {
    let driver = HeadlessDriver::default();
    let mut mounted = Mounted::new(driver, |_| btn("Save").test_id("save").tooltip("Save the current draft. ".repeat(8)));
    let anchor = mounted.node("save");
    let bounds = mounted.window().geometry(anchor).unwrap().rect;
    let event = InteractionEvent::PointerMove {
        id: PointerId(1),
        pos: Point::new(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0),
        mods: Modifiers::empty(),
    };
    mounted.driver.dispatch(mounted.id, Input::Interaction(event)).unwrap();
    mounted.driver.advance_to(Duration::from_millis(450)).unwrap();
    let tooltips: Vec<_> =
        mounted.tree().overlay_nodes().filter(|&node| mounted.tree().semantics(node).unwrap().role == Some(Role::Tooltip)).collect();
    let [tooltip] = tooltips.as_slice() else { panic!("expected one tooltip") };
    assert_eq!(mounted.tree().logical_parent(*tooltip), Some(anchor));
    assert!(mounted.window().geometry(*tooltip).unwrap().rect.w <= 280.0);
}

#[test]
fn alert_dialog_labels_panel_and_inherits_close_handle() {
    let mut driver = HeadlessDriver::default();
    let open = driver.app_mut().build(|b| b.signal(true)).unwrap();
    let mut mounted = Mounted::new(driver, move |_| {
        alert_dialog("confirm", "Delete draft?", open.into(), "The draft will be removed.", |_| {
            headless::close::close_button(btn("Cancel").test_id("cancel").autofocus())
        })
    });
    let alert = mounted.tree().semantics(mounted.node("confirm")).unwrap();
    assert_eq!(alert.role, Some(Role::AlertDialog));
    let title = alert.relations.labelled_by.as_deref().unwrap();
    let description = alert.relations.described_by.as_deref().unwrap();
    assert_eq!(mounted.tree().named_nodes(title), &[mounted.node("confirm-title")]);
    assert_eq!(mounted.tree().named_nodes(description), &[mounted.node("confirm-description")]);
    assert!(mounted.a11y("cancel", A11yAction::Press));
    assert!(!mounted.update(|w| w.get(open)));
}
