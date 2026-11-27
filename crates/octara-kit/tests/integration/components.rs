use super::*;
use headless::Choice;

#[test]
fn controls_update_bindings_before_application_handlers() {
    let mut driver = HeadlessDriver::default();
    let (checked, switched, draft, observed) =
        driver.app_mut().build(|b| (b.signal(false), b.signal(false), b.signal(String::new()), b.signal(Vec::<String>::new()))).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        let accepted_draft = b.binding(move |r| r.get(draft), move |w, value: String| w.set(draft, format!("[{value}]")));
        col()
            .child(checkbox(checked.into()).test_id("check").label("Check").on_toggle(move |w| {
                let accepted = w.get(checked);
                w.update(observed, |out| out.push(accepted.to_string()));
            }))
            .child(switch(switched.into()).test_id("switch").label("Switch").on_toggle(move |w| {
                let accepted = w.get(switched);
                w.update(observed, |out| out.push(accepted.to_string()));
            }))
            .child(text_field(accepted_draft).test_id("draft").label("Draft").on_input(move |w, _| {
                let accepted = w.get(draft);
                w.update(observed, |out| out.push(accepted));
            }))
            .child(icon_button(octara_icons::COPY, "Copy").test_id("copy"))
            .child(button("Confirm", octara_kit::Size::Md, ButtonVariant::Primary).test_id("primary").on_press(|_| {}))
            .child(
                button("Disabled", octara_kit::Size::Md, ButtonVariant::Secondary)
                    .test_id("disabled")
                    .disabled(true)
                    .on_press(|_| panic!("disabled button activated")),
            )
    });
    assert!(mounted.a11y("check", A11yAction::Press));
    assert!(mounted.a11y("switch", A11yAction::Focus));
    assert!(mounted.key(Key::Space));
    assert!(mounted.a11y("draft", A11yAction::Focus));
    mounted.commit("日本語");
    assert_eq!(mounted.update(|w| w.get(observed)), ["true", "true", "[日本語]"]);
    assert_eq!(mounted.tree().semantics(mounted.node("check")).unwrap().checked, Some(Checked::Checked));
    assert_eq!(mounted.tree().semantics(mounted.node("copy")).unwrap().tooltip.as_deref(), Some("Copy"));
    assert_eq!(mounted.tree().provenance(mounted.node("primary")).unwrap().component.unwrap().name, "button");
    assert!(!mounted.a11y("disabled", A11yAction::Press));
}

#[test]
fn field_validation_preserves_the_editor_and_combines_errors() {
    let mut driver = HeadlessDriver::default();
    let (draft, label, info) = driver
        .app_mut()
        .build(|b| {
            (
                b.signal("bad".to_owned()),
                b.signal("Quantity".to_owned()),
                b.signal(FieldInfo { description: "Whole units".into(), error: Some("Review".into()), required: true }),
            )
        })
        .unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        let editor = number_field(b, draft.into(), 0.0, 10.0, 0.5);
        field(b, "amount", label, editor, info)
    });
    let amount = mounted.node("amount");
    assert!(mounted.a11y("amount-label", A11yAction::Press));
    mounted.focus_is("amount");
    let sem = mounted.tree().semantics(amount).unwrap();
    assert_eq!(sem.description.as_deref(), Some("Whole units\nReview"));
    assert_eq!(sem.relations.labelled_by, mounted.tree().provenance(mounted.node("amount-label")).unwrap().id);
    assert_eq!(sem.required, Some(true));
    assert_eq!(sem.invalid, Some(true));
    assert!(mounted.a11y(
        "amount",
        A11yAction::SetTextSelection { anchor: TextPosition { node: amount, offset: 0 }, focus: TextPosition { node: amount, offset: 3 } },
    ));
    mounted.mark("日");
    assert!(!mounted.key(Key::Up));
    mounted.update(|w| {
        w.set(label, "Weight".into());
        w.update(info, |info| info.error = None);
    });
    assert_eq!(mounted.node("amount"), amount);
    assert!(mounted.tree().editor(amount).unwrap().composition().is_some());
    assert_eq!(mounted.tree().semantics(amount).unwrap().invalid, Some(true));
    assert!(mounted.tree().test_ids("amount-error").is_empty());
    mounted.commit("2.25");
    assert_eq!(mounted.update(|w| w.get(draft)), "2.25");
    assert!(mounted.key(Key::Enter));
    assert_eq!(mounted.update(|w| w.get(draft)), "2.5");
    assert_eq!(mounted.tree().semantics(amount).unwrap().invalid, Some(false));
    assert!(mounted.a11y("amount", A11yAction::SetNumericValue(12.0)));
    assert_eq!(mounted.update(|w| w.get(draft)), "10");
}

#[test]
fn tabs_rove_without_activation_and_radio_groups_activate() {
    let mut driver = HeadlessDriver::default();
    let (selection, radio, choices) = driver
        .app_mut()
        .build(|b| {
            (
                b.signal(Some("a".to_owned())),
                b.signal(Some("b".to_owned())),
                b.list_from([
                    ("a".to_owned(), Choice::new("A")),
                    ("disabled".to_owned(), Choice::new("Unavailable").disabled(true)),
                    ("b".to_owned(), Choice::new("B")),
                ]),
            )
        })
        .unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        let segments = b.list_from([("a".to_owned(), Choice::new("A")), ("b".to_owned(), Choice::new("B"))]);
        col()
            .child(button("Before", octara_kit::Size::Md, ButtonVariant::Secondary).test_id("before").on_press(|_| {}))
            .child(tabs(
                b,
                "tabs",
                "Tabs",
                selection.into(),
                choices.read_only(),
                TabsOptions { activation: headless::tabs::Activation::Manual, ..Default::default() },
                |_, key, _| text(key.clone()),
            ))
            .child(segmented(b, "segments", "Segments", radio.into(), segments.read_only()))
    });
    assert!(mounted.a11y("before", A11yAction::Focus));
    assert!(mounted.key(Key::Tab));
    mounted.focus_is("tabs-tab-a");
    assert!(mounted.key(Key::Right));
    mounted.focus_is("tabs-tab-b");
    assert_eq!(mounted.update(|w| w.get(selection)).as_deref(), Some("a"));
    assert!(mounted.tree().is_shown(mounted.node("tabs-panel-a")));
    assert!(!mounted.tree().is_shown(mounted.node("tabs-panel-b")));
    assert!(mounted.key(Key::Enter));
    assert_eq!(mounted.update(|w| w.get(selection)).as_deref(), Some("b"));
    assert!(mounted.tree().is_shown(mounted.node("tabs-panel-b")));
    assert!(!mounted.tree().is_shown(mounted.node("tabs-panel-a")));
    let retained = mounted.node("tabs-tab-b");
    mounted.update(|w| {
        let key = "b".to_owned();
        w.move_row(choices, &key, 0);
        w.update_row(choices, &key, |row| row.label = "Renamed".into());
    });
    assert_eq!(mounted.node("tabs-tab-b"), retained);
    assert_eq!(mounted.tree().text(retained).unwrap().text.as_ref(), "Renamed");
    assert!(mounted.key(Key::Tab));
    mounted.focus_is("segments-option-b");
    assert!(mounted.key(Key::Left));
    mounted.focus_is("segments-option-a");
    assert_eq!(mounted.update(|w| w.get(radio)).as_deref(), Some("a"));
}

#[test]
fn control_labels_center_within_their_vertical_bounds() {
    let mounted = Mounted::new(HeadlessDriver::default(), |b| {
        let active = b.signal(Some("a"));
        let options = b.list_from([("a", Choice::new("Label"))]);
        col()
            .child(text("Label").text_size(FontSizeToken::Sm).text_weight(500).bg(ColorToken::Background).test_id("control-label"))
            .child(text("Label").text_size(FontSizeToken::Xs).text_weight(600).bg(ColorToken::Background).test_id("badge-label"))
            .child(button("Label", octara_kit::Size::Md, ButtonVariant::Secondary).min_h(60).test_id("button"))
            .child(tabs(b, "tabs", "Tabs", active.into(), options.read_only(), TabsOptions::default(), |_, _, _| el()))
            .child(segmented(b, "segments", "Segments", active.into(), options.read_only()))
            .child(badge("Label", Tone::Success).min_h(60).test_id("badge"))
    });
    let draw = mounted.window().last_commit().unwrap().draw.as_deref().unwrap();
    assert!(mounted.tree().text(mounted.node("button")).is_none(), "the button styles a flex container");
    let baseline_from_center = |name| {
        let node = mounted.node(name);
        let rect = draw.quads().find(|quad| quad.source == node).unwrap().rect;
        let run = draw.glyph_runs().find(|run| mounted.tree().is_within(run.source, node)).unwrap();
        run.placed().next().unwrap().y - (rect.y + rect.h * 0.5)
    };
    for (name, label) in
        [("button", "control-label"), ("tabs-tab-a", "control-label"), ("segments-option-a", "control-label"), ("badge", "badge-label")]
    {
        let offset = baseline_from_center(name);
        let expected = baseline_from_center(label);
        assert!((offset - expected).abs() < 0.01, "{name} baseline is {offset}px from center, expected {expected}px");
    }
}

#[test]
fn removing_a_radio_keeps_the_callers_selection() {
    let mut driver = HeadlessDriver::default();
    let (selection, choices) = driver
        .app_mut()
        .build(|b| (b.signal(Some("a".to_owned())), b.list_from([("a".to_owned(), Choice::new("A")), ("b".to_owned(), Choice::new("B"))])))
        .unwrap();
    let mut mounted = Mounted::new(driver, move |_| radio_group("plan", "Plan", selection.into(), choices.read_only()));
    let first = mounted.node("plan-option-a");
    let second = mounted.node("plan-option-b");
    let key = "a".to_owned();
    let removed = mounted.update(|w| w.remove_row(choices, &key)).unwrap();
    assert!(mounted.tree().test_ids("plan-option-a").is_empty());
    assert_eq!(mounted.node("plan-option-b"), second);
    assert_eq!(mounted.tree().semantics(second).unwrap().checked, Some(Checked::Unchecked));
    assert_eq!(mounted.update(|w| w.get(selection)).as_deref(), Some("a"));
    mounted.update(|w| w.insert_row(choices, 0, key, removed));
    let revived = mounted.node("plan-option-a");
    assert_ne!(revived, first);
    assert_eq!(mounted.tree().semantics(revived).unwrap().checked, Some(Checked::Checked));
}

#[test]
fn offset_slider_uses_pointer_local_coordinates_and_the_same_steps_as_a11y() {
    let mut driver = HeadlessDriver::default();
    let value = driver.app_mut().build(|b| b.signal(0.0)).unwrap();
    let mut mounted = Mounted::new(driver, move |b| col().p(20).child(row().p(35).child(slider(b, "level", value.into(), 0.0, 10.0, 0.5))));
    let at = Point::new(155.0, 69.0);
    assert!(mounted.pointer("level", AgentAction::Tap { pos: at, modifiers: Modifiers::empty() }));
    assert_eq!(mounted.update(|w| w.get(value)), 5.0);
    assert!(mounted.a11y("level", A11yAction::SetNumericValue(7.24)));
    assert_eq!(mounted.update(|w| w.get(value)), 7.0);
    assert!(mounted.key(Key::Left));
    assert_eq!(mounted.update(|w| w.get(value)), 6.5);
    let id = PointerId(20);
    mounted.pointer(
        "level",
        AgentAction::PointerDown { id, pointer: PointerKind::Mouse, pos: at, button: Button::Primary, modifiers: Modifiers::empty() },
    );
    mounted.pointer("level", AgentAction::PointerMove { id, pos: Point::new(225.0, at.y), modifiers: Modifiers::empty() });
    assert!(mounted.update(|w| w.get(value)) > 6.5);
    mounted.pointer("level", AgentAction::PointerCancel { id });
    assert_eq!(mounted.update(|w| w.get(value)), 6.5);
    use headless::range::{Range, parse_number};
    assert_eq!(parse_number(" 0.375 ").map(|value| Range::new(0.0, 2.0, 0.25).normalize_text(value)).as_deref(), Some("0.5"));
    assert_eq!(parse_number("NaN"), None);
    assert_eq!(parse_number("-"), None);
}

#[test]
fn collapsible_progress_and_split_use_controlled_state_and_clock() {
    let mut driver = HeadlessDriver::default();
    let (open, width, value) = driver.app_mut().build(|b| (b.signal(false), b.signal(180.0), b.signal(None::<f64>))).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        col()
            .child(collapsible(b, "details", open.into(), "Details", |_| text("Body")))
            .child(split(b, "panes", width.into(), "First", "Second").h(80.0))
            .child(progress(b, value, 10.0).test_id("progress"))
            .child(list().child(list_item("Current").selected(true)))
            .child(callout(Tone::Warning, "Warning", Some("Detail")))
    });
    assert!(mounted.a11y("details-trigger", A11yAction::Press));
    let mounted_content = mounted.node("details-content");
    assert!(mounted.a11y("details-trigger", A11yAction::Press));
    assert!(mounted.tree().test_ids("details-content").is_empty());
    assert!(mounted.a11y("details-trigger", A11yAction::Press));
    assert_ne!(mounted.node("details-content"), mounted_content);
    assert!(mounted.a11y("panes-divider", A11yAction::Focus));
    assert!(mounted.key(Key::Right));
    assert_eq!(mounted.update(|w| w.get(width)), 188.0);
    let before = mounted.window().last_commit().unwrap().draw.clone().unwrap();
    mounted.driver.advance_to(Duration::from_millis(100)).unwrap();
    let after = mounted.window().last_commit().unwrap().draw.as_deref().unwrap();
    assert_ne!(before.paths().collect::<Vec<_>>(), after.paths().collect::<Vec<_>>());
    let fixed = mounted.node("progress");
    assert_eq!(mounted.tree().semantics(fixed).unwrap().numeric_range.unwrap().value, None);
    mounted.update(|w| w.set(value, Some(20.0)));
    assert_eq!(mounted.node("progress"), fixed);
    assert_eq!(mounted.tree().semantics(fixed).unwrap().numeric_range.unwrap().value, Some(10.0));
}

#[test]
fn data_widgets_and_breadcrumbs_project_their_actual_semantics() {
    use headless::{
        Control, DataTableState, Selection, TreeViewState,
        data_table::{Column, ColumnWidth, SortMode},
        tree_view::TreeItem,
    };

    let mut mounted = Mounted::new(HeadlessDriver::default(), |b| {
        let selected = b.signal(None);
        let items = b.list_from([("branch", TreeItem::new("Branch")), ("leaf", TreeItem::new("Leaf").under("branch"))]);
        let tree = TreeViewState::new(b, items, Control::Uncontrolled(vec!["branch"]), Selection::Single(selected.into()));
        let table = DataTableState::new(
            b,
            [("value", Column::new("Value").sortable().resizable(ColumnWidth::new(100.0, 80.0, 200.0)))],
            SortMode::Single,
            Control::Uncontrolled(Vec::new()),
            None,
        );
        let values = b.list_from([(1_u32, 30), (2, 10), (3, 20)]);
        col()
            .child(breadcrumb(b, [text("Project"), text("Review")]).test_id("crumbs"))
            .child(separator().test_id("separator"))
            .child(tree_view(tree, None).test_id("tree"))
            .child(
                data_table(
                    b,
                    table,
                    values.read_only(),
                    None,
                    |_, left, right| left.cmp(right),
                    |_, _, key, value| text(bind(move |r| r.try_get(value).unwrap_or_default().to_string())).test_id(format!("cell-{key}")),
                )
                .test_id("table"),
            )
    });
    let role = |app: &Mounted, name| app.tree().semantics(app.node(name)).unwrap().role;
    assert_eq!(role(&mounted, "crumbs"), Some(Role::Navigation));
    assert_eq!(role(&mounted, "separator"), Some(Role::Separator));
    assert_eq!(role(&mounted, "tree"), Some(Role::Tree));
    assert_eq!(role(&mounted, "table"), Some(Role::Table));
    let subtree = mounted.tree().subtree(mounted.node("tree"));
    let labels: Vec<_> = subtree
        .filter_map(|node| {
            let slot = mounted.tree().semantics(node)?;
            (slot.role == Some(Role::TreeItem)).then(|| slot.label.as_deref().unwrap())
        })
        .collect();
    assert_eq!(labels, ["Branch", "Leaf"]);
    let positions = |app: &Mounted| {
        ["cell-1", "cell-2", "cell-3"].map(|id| {
            let cell = app.node(id);
            let row = app.tree().physical_parent(cell).unwrap();
            app.window().geometry(row).unwrap().rect.y
        })
    };
    let initial = positions(&mounted);
    assert!(initial[0] < initial[1] && initial[1] < initial[2]);
    let sort = mounted
        .tree()
        .subtree(mounted.node("table"))
        .find(|&node| mounted.tree().semantics(node).is_some_and(|slot| slot.label.as_deref() == Some("Sort by Value")))
        .unwrap();
    let target = mounted.tree().guard(sort);
    let reply = HostReply::AccessibilityAction { target, action: A11yAction::Press };
    assert!(mounted.driver.dispatch(mounted.id, Input::Host(reply)).unwrap().handled);
    let sorted = positions(&mounted);
    assert!(sorted[1] < sorted[2] && sorted[2] < sorted[0]);
}
