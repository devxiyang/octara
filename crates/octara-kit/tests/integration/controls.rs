use super::*;
use octara_kit::Size as KitSize;

#[test]
fn touch_controls_preserve_hit_targets_and_mixed_checkbox_semantics() {
    let mut driver = HeadlessDriver::default();
    let checked = driver.app_mut().build(|b| b.signal(Checked::Mixed)).unwrap();
    let id = driver
        .app_mut()
        .open_window(WindowOptions { touch: true, ..Default::default() }, move |b| {
            let on = b.signal(false);
            row()
                .child(button("Small", KitSize::Sm, ButtonVariant::Secondary).test_id("small"))
                .child(button("Delete", KitSize::Lg, ButtonVariant::Destructive).test_id("danger"))
                .child(icon_button(octara_icons::X, "Close").test_id("icon"))
                .child(checkbox(checked.into()).test_id("check").label("Select all"))
                .child(switch(on.into()).test_id("switch").label("Enabled"))
        })
        .unwrap();
    let mut mounted = Mounted { driver, id };
    for name in ["small", "danger", "icon", "check", "switch"] {
        let rect = mounted.window().geometry(mounted.node(name)).unwrap().rect;
        assert!(rect.w >= 44.0 && rect.h >= 44.0, "{name}: {rect:?}");
    }
    let node = mounted.node("check");
    assert_eq!(mounted.tree().semantics(node).unwrap().checked, Some(Checked::Mixed));
    assert!(mounted.a11y("check", A11yAction::Press));
    assert_eq!(mounted.update(|w| w.get(checked)), Checked::Checked);
    assert_eq!(mounted.node("check"), node);
    assert!(mounted.a11y("check", A11yAction::Press));
    assert_eq!(mounted.update(|w| w.get(checked)), Checked::Unchecked);
}

#[test]
fn toggle_button_commits_its_pressed_binding() {
    let mut driver = HeadlessDriver::default();
    let pressed = driver.app_mut().build(|b| b.signal(false)).unwrap();
    let mut mounted =
        Mounted::new(driver, move |_| toggle_button("Pin", pressed.into(), KitSize::Sm, ButtonVariant::Ghost).test_id("toggle"));
    let node = mounted.node("toggle");
    assert_eq!(mounted.tree().semantics(node).unwrap().role, Some(Role::Button));
    assert!(mounted.a11y("toggle", A11yAction::Press));
    assert!(mounted.update(|w| w.get(pressed)));
    assert_eq!(mounted.tree().semantics(node).unwrap().checked, Some(Checked::Checked));
    assert!(mounted.a11y("toggle", A11yAction::Focus));
    assert!(mounted.key(Key::Space));
    assert!(!mounted.update(|w| w.get(pressed)));
}

#[test]
fn search_clears_and_password_reveal_retains_the_editor() {
    let mut driver = HeadlessDriver::default();
    let (query, password, submitted) =
        driver.app_mut().build(|b| (b.signal(String::new()), b.signal("test value".to_owned()), b.signal(String::new()))).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        col()
            .child(search_field(b, "query", "Find notes", query.into(), |editor| {
                editor.on_submit(move |w| {
                    let text = w.get(query);
                    w.set(submitted, text);
                })
            }))
            .child(password_field(b, "password", "Password", password.into(), |editor| editor))
    });
    assert!(mounted.tree().test_ids("query-clear").is_empty());
    assert!(mounted.a11y("query", A11yAction::Focus));
    mounted.commit("notes");
    assert!(mounted.key(Key::Enter));
    assert_eq!(mounted.update(|w| w.get(submitted)), "notes");
    let query_node = mounted.node("query");
    assert!(mounted.a11y("query-clear", A11yAction::Focus));
    assert!(mounted.key(Key::Space));
    assert!(mounted.tree().test_ids("query-clear").is_empty());
    assert!(mounted.update(|w| w.get(query)).is_empty());
    assert_eq!(mounted.node("query"), query_node);
    mounted.focus_is("query");

    let password_node = mounted.node("password");
    assert!(mounted.tree().editor(password_node).unwrap().is_secure());
    assert!(mounted.a11y("password", A11yAction::Focus));
    assert!(mounted.a11y("password-reveal", A11yAction::Press));
    mounted.focus_is("password");
    assert!(!mounted.tree().editor(password_node).unwrap().is_secure());
    let reveal = mounted.node("password-reveal");
    assert_eq!(mounted.tree().semantics(reveal).unwrap().label.as_deref(), Some("Hide password"));
    assert!(mounted.a11y("password-reveal", A11yAction::Press));
    assert!(mounted.tree().editor(password_node).unwrap().is_secure());
    assert_eq!(mounted.node("password-reveal"), reveal);
    assert_eq!(mounted.node("password"), password_node);
    assert_eq!(mounted.update(|w| w.get(password)), "test value");
}

#[test]
fn editors_follow_pointer_focus_and_composites_follow_descendant_focus() {
    let mut mounted = Mounted::new(HeadlessDriver::default(), |b| {
        let value = b.signal("initial".to_owned());
        col()
            .child(text_field(value.into()).test_id("input"))
            .child(text_area(value.into()).test_id("textarea"))
            .child(search_field(b, "search", "Search", value.into(), |editor| editor).test_id("search-group"))
            .child(password_field(b, "password", "Password", value.into(), |editor| editor).test_id("password-group"))
            .child(button("Save", KitSize::Md, ButtonVariant::Secondary).test_id("save").on_press(|_| {}))
    });
    let tap = |mounted: &mut Mounted, name| {
        let rect = mounted.window().geometry(mounted.node(name)).unwrap().rect;
        let pos = point(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
        mounted.pointer(name, AgentAction::Tap { pos, modifiers: Modifiers::empty() });
    };
    let appearance = |mounted: &Mounted, name| mounted.tree().visual(mounted.node(name));
    let focus = Theme::default().get(ColorToken::Focus);
    let border = Theme::default().get(ColorToken::Border);
    tap(&mut mounted, "input");
    mounted.focus_is("input");
    assert_eq!(appearance(&mounted, "input").border, focus);
    assert!(appearance(&mounted, "input").ring.is_some());
    assert_eq!(appearance(&mounted, "textarea").border, border);
    tap(&mut mounted, "textarea");
    mounted.focus_is("textarea");
    assert_eq!(appearance(&mounted, "textarea").border, focus);
    assert_eq!(appearance(&mounted, "input").border, border);

    assert!(mounted.a11y("search-clear", A11yAction::Focus));
    assert_eq!(appearance(&mounted, "search-group").border, focus);
    assert!(appearance(&mounted, "search-group").ring.is_some());
    assert!(mounted.a11y("password-reveal", A11yAction::Focus));
    assert_eq!(appearance(&mounted, "password-group").border, focus);
    assert_eq!(appearance(&mounted, "search-group").border, border);
    assert!(mounted.key(Key::Tab));
    mounted.focus_is("save");
    assert!(appearance(&mounted, "save").ring.is_some());
    assert_eq!(appearance(&mounted, "password-group").border, border);
    tap(&mut mounted, "save");
    mounted.focus_is("save");
    assert!(appearance(&mounted, "save").ring.is_none());
}

#[test]
fn fieldset_disables_contained_fields_and_keeps_their_ids_and_relations() {
    let mut driver = HeadlessDriver::default();
    let (disabled, value, info) = driver
        .app_mut()
        .build(|b| {
            (
                b.signal(false),
                b.signal(String::new()),
                b.signal(FieldInfo { description: "Enter a display name".to_owned(), ..Default::default() }),
            )
        })
        .unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        fieldset(b, "profile", "Profile", disabled, move |b| field(b, "name", "Name", text_field(value.into()), info))
    });
    let name = mounted.node("name");
    let legend = mounted.node("profile-legend");
    let group = mounted.tree().semantics(mounted.node("profile")).unwrap();
    assert_eq!(group.role, Some(Role::Group));
    assert_eq!(group.relations.labelled_by, mounted.tree().provenance(legend).unwrap().id);
    assert!(mounted.tree().semantics(name).unwrap().relations.described_by.is_some());
    assert!(mounted.a11y("name-label", A11yAction::Press));
    mounted.focus_is("name");
    mounted.update(|w| w.set(disabled, true));
    assert!(!mounted.tree().is_interactive(name));
    assert!(!mounted.a11y("name", A11yAction::Focus));
    mounted.update(|w| w.set(disabled, false));
    assert_eq!(mounted.node("name"), name);
    assert!(mounted.a11y("name", A11yAction::Focus));
}

#[test]
fn dropzone_routes_drop_and_application_chooser_action() {
    let mut driver = HeadlessDriver::default();
    let seen = driver.app_mut().build(|b| b.signal(Vec::new())).unwrap();
    let mut mounted = Mounted::new(driver, move |_| {
        file_dropzone(
            "files",
            "Add an attachment",
            move |w, files| {
                assert_eq!(files, [DroppedFile::Name("dropped.txt".to_owned())]);
                w.update(seen, |seen| seen.push("dropped"));
            },
            move |w| w.update(seen, |seen| seen.push("choose")),
        )
    });
    let node = mounted.node("files");
    let target = mounted.tree().guard(node);
    let rect = mounted.window().geometry(node).unwrap().rect;
    let pos = point(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    assert_eq!(mounted.window().file_drop_target(pos), Some(target));
    let drop = HostReply::FilesDropped { target, files: vec![DroppedFile::Name("dropped.txt".to_owned())] };
    assert!(mounted.driver.dispatch(mounted.id, Input::Host(drop)).unwrap().handled);
    assert!(mounted.a11y("files", A11yAction::Press));
    assert_eq!(mounted.update(|w| w.get(seen)), ["dropped", "choose"]);
}

#[test]
fn status_components_publish_labels_and_removable_chips_activate() {
    let mut driver = HeadlessDriver::default();
    let removed = driver.app_mut().build(|b| b.signal(false)).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        col()
            .child(badge("Ready", Tone::Success).test_id("badge"))
            .child(avatar("Ada Lovelace", None).test_id("avatar"))
            .child(skeleton().test_id("placeholder"))
            .child(callout(Tone::Danger, "Could not save", Some("Server unreachable")).test_id("notice"))
            .child(removable_chip(b, "Attachment", |target| target.test_id("remove").on_press(move |w| w.set(removed, true))))
    });
    assert_eq!(mounted.tree().text(mounted.node("badge")).unwrap().text.as_ref(), "Ready");
    let avatar = mounted.node("avatar");
    assert_eq!(mounted.tree().semantics(avatar).unwrap().label.as_deref(), Some("Ada Lovelace"));
    let [initials] = mounted.tree().children(avatar) else { panic!("expected one avatar visual") };
    assert_eq!(mounted.tree().text(*initials).unwrap().text.as_ref(), "AL");
    let placeholder = mounted.node("placeholder");
    assert!(mounted.tree().node(placeholder).unwrap().flags.contains(octara::tree::NodeFlags::HIDDEN));
    let focus = mounted.tree().focus();
    mounted.a11y("placeholder", A11yAction::Focus);
    assert_eq!(mounted.tree().focus(), focus);
    let notice = mounted.tree().semantics(mounted.node("notice")).unwrap();
    assert_eq!((notice.role, notice.live), (Some(Role::Alert), Some(Live::Polite)));
    let theme = Theme::default();
    for (name, tone) in [("badge", ColorToken::Success), ("notice", ColorToken::Danger)] {
        let color = theme.get(tone);
        let visual = mounted.tree().visual(mounted.node(name));
        assert_eq!(visual.background, Some(color.multiply_alpha(0.10)));
        assert_eq!(visual.border, color.multiply_alpha(0.25));
    }
    assert_eq!(mounted.tree().semantics(mounted.node("remove")).unwrap().label.as_deref(), Some("Remove Attachment"));
    assert!(mounted.a11y("remove", A11yAction::Press));
    assert!(mounted.update(|w| w.get(removed)));
}

#[test]
fn localized_labels_follow_context_across_child_scopes_and_dialogs() {
    let mounted = Mounted::new(HeadlessDriver::default(), |b| {
        b.provide(KitLocale::zh_cn());
        let query = b.signal("笔记".to_owned());
        let open = b.signal(true);
        col()
            .child(search_field(b, "query", "查找笔记", query.into(), |editor| editor))
            .child(removable_chip(b, "附件", |remove| remove.test_id("remove")))
            .child(spinner(b).test_id("loading"))
            .child(scoped(move |b| {
                b.provide(KitLocale { close: "完成", ..KitLocale::zh_cn() });
                dialog("summary", "摘要", open.into(), DialogOptions::default(), |b| breadcrumb(b, [text("首页")]).test_id("breadcrumbs"))
            }))
    });
    let label = |name| mounted.tree().semantics(mounted.node(name)).unwrap().label.as_deref().unwrap();
    assert_eq!(mounted.tree().editor(mounted.node("query")).unwrap().placeholder.as_ref(), "搜索");
    assert_eq!(label("query"), "查找笔记");
    assert_eq!(label("query-clear"), "清除搜索");
    assert_eq!(label("remove"), "移除附件");
    assert_eq!(label("loading"), "正在加载");
    assert_eq!(label("summary-close"), "完成");
    assert_eq!(label("breadcrumbs"), "面包屑导航");
    assert_eq!(mounted.tree().visual(mounted.node("summary")).background, Some(Theme::default().get(ColorToken::SurfaceElevated)));
}
