use octara::{HostCommand, HostReply, Input, InteractionEvent, PointerId, PointerKind, prelude::*};
use octara_inspect::HeadlessDriver;
use octara_pipeline::NativeEngineConfig;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

#[test]
fn initial_presentation_resolves_custom_tokens_during_mount() {
    use octara::model::diagnostics::codes;

    let mut driver = HeadlessDriver::new(Default::default());
    let app = driver.app_mut();
    let token = ColorToken::Custom("fixture-accent");
    let theme = Theme::dark().color(token, rgb(0x26478c));
    let window = AppDefinition::new()
        .window(WindowDefinition::new(WindowOptions::default(), move |_| el().bg(token)).presentation_with(move |_| WindowPresentation {
            title: "Custom".into(),
            theme: theme.clone(),
            ..Default::default()
        }))
        .launch(app, None)
        .unwrap()[0];
    assert_eq!(app.runtime().window(window).unwrap().presentation().title.as_ref(), "Custom");
    assert!(app.runtime_mut().take_diagnostics().iter().all(|diagnostic| diagnostic.code != &codes::MISSING_THEME_TOKEN));
}

#[test]
fn reactive_and_imperative_presentation_share_the_published_frame() {
    let mut driver = HeadlessDriver::new(NativeEngineConfig { supports_window_vibrancy: true, ..Default::default() });
    let app = driver.app_mut();
    let (title, dark, vibrant, status_bar) =
        app.build(|build| (build.signal("Initial"), build.signal(false), build.signal(false), build.signal(StatusBarStyle::Auto))).unwrap();
    let material = WindowMaterial::Vibrancy(VibrancyMaterial::Sidebar);
    let window = AppDefinition::new()
        .window(
            WindowDefinition::new(WindowOptions::default(), |_| el())
                .title_with(move |read| read.get(title))
                .theme_with(move |read| if read.get(dark) { Theme::dark() } else { Theme::light() })
                .material_with(move |read| if read.get(vibrant) { material } else { WindowMaterial::Opaque })
                .status_bar_with(move |read| read.get(status_bar)),
        )
        .launch(app, None)
        .unwrap()[0];
    let current = app.runtime().window(window).unwrap();
    assert_eq!(current.presentation().title.as_ref(), "Initial");
    assert_eq!(current.last_commit().unwrap().draw.as_ref().unwrap().background, current.tree().theme().get(ColorToken::Background));
    assert!(matches!(app.runtime_mut().take_host_commands().as_slice(), [
        HostCommand::Opened(opened),
        HostCommand::UpdateWindowPresentation {
            window: updated, title, color_scheme: ColorScheme::Light, material: WindowMaterial::Opaque, status_bar: StatusBarStyle::Auto,
        },
    ] if *opened == window && *updated == window && title.as_ref() == "Initial"));

    app.update_window(window, |write| {
        write.set_window_title("Intermediate");
        write.set(title, "Reactive");
        write.set(dark, true);
        write.set(vibrant, true);
        write.set(status_bar, StatusBarStyle::Hidden);
    })
    .unwrap();
    assert!(matches!(app.runtime_mut().take_host_commands().as_slice(), [
        HostCommand::UpdateWindowPresentation {
            window: updated, title, color_scheme: ColorScheme::Dark, material: updated_material, status_bar: StatusBarStyle::Hidden,
        },
    ] if *updated == window && title.as_ref() == "Reactive" && *updated_material == material));
    let current = app.runtime().window(window).unwrap();
    assert_eq!(current.presentation().theme, Theme::dark());
    assert_eq!(current.tree().theme(), &current.presentation().theme);
    assert_eq!(current.last_commit().unwrap().draw.as_ref().unwrap().background, Color::TRANSPARENT);

    let background = rgb(0x203757);
    let theme = Theme::dark().color(ColorToken::Background, background);
    app.update_window(window, |write| {
        write.set_window_theme(theme.clone());
        write.set_window_material(WindowMaterial::Opaque);
        write.set_window_title("Final");
    })
    .unwrap();
    let current = app.runtime().window(window).unwrap();
    assert_eq!(current.options().presentation.title.as_ref(), "Final");
    assert_eq!(current.tree().theme(), &theme);
    assert_eq!(current.last_commit().unwrap().draw.as_ref().unwrap().background, background);
    assert!(matches!(app.runtime_mut().take_host_commands().as_slice(), [
        HostCommand::UpdateWindowPresentation {
            window: updated, title, color_scheme: ColorScheme::Dark, material: WindowMaterial::Opaque, status_bar: StatusBarStyle::Hidden,
        },
    ] if *updated == window && title.as_ref() == "Final"));

    app.update_window(window, |write| {
        write.set_window_title("Temporary");
        write.set_window_title("Final");
    })
    .unwrap();
    assert!(app.runtime_mut().take_host_commands().is_empty());

    app.update_window(window, |write| write.set_window_status_bar(StatusBarStyle::Light)).unwrap();
    assert_eq!(app.window(window).unwrap().presentation().status_bar, StatusBarStyle::Light);
    assert!(matches!(app.runtime_mut().take_host_commands().as_slice(), [
        HostCommand::UpdateWindowPresentation { window: updated, status_bar: StatusBarStyle::Light, .. },
    ] if *updated == window));
    app.update_window(window, |write| write.set_window_status_bar(StatusBarStyle::Light)).unwrap();
    assert!(app.runtime_mut().take_host_commands().is_empty());
}

#[test]
fn headless_records_badge_and_attention_commands() {
    let mut driver = HeadlessDriver::new(Default::default());
    let app = driver.app_mut();
    let window = app.open_window(WindowOptions::default(), |_| el()).unwrap();
    app.runtime_mut().take_host_commands();
    app.update_window(window, |write| {
        write.set_app_badge(Some("1".into()));
        write.set_app_badge(Some("2".into()));
        write.request_user_attention(UserAttention::Informational);
        write.request_user_attention(UserAttention::Critical);
    })
    .unwrap();
    assert!(matches!(app.runtime_mut().take_host_commands().as_slice(), [
        HostCommand::RequestUserAttention { window: Some(first), attention: UserAttention::Informational },
        HostCommand::RequestUserAttention { window: Some(second), attention: UserAttention::Critical },
        HostCommand::SetAppBadge { badge: Some(badge) },
    ] if *first == window && *second == window && badge.as_ref() == "2"));
    app.update(|write| write.set_app_badge(None)).unwrap();
    assert!(matches!(app.runtime_mut().take_host_commands().as_slice(), [HostCommand::SetAppBadge { badge: None }]));
}

#[test]
fn titlebar_hit_testing_leaves_child_and_ancestor_controls_available() {
    let mut driver = HeadlessDriver::new(Default::default());
    let app = driver.app_mut();
    let presses = Rc::new(Cell::new(0));
    let drags = Rc::new(Cell::new(0));
    let window = app
        .open_window(WindowOptions::default(), {
            let presses = presses.clone();
            let drags = drags.clone();
            move |_| {
                col()
                    .child(
                        row()
                            .window_drag_region(true)
                            .h(36)
                            .child(el().test_id("blank").w(80).h(36))
                            .child(el().w(80).h(36).on_press(move |_| presses.set(presses.get() + 1)).child(text("Tab")))
                            .child(input().w(100).h(36).test_id("editor"))
                            .child(el().w(60).h(36).on_drag(move |_, _| drags.set(drags.get() + 1))),
                    )
                    .child(row().w(100).h(36).focusable().test_id("ancestor").child(el().window_drag_region(true).w(80).h(36)))
            }
        })
        .unwrap();
    app.runtime_mut().take_host_commands();
    let pointer = PointerId(1);
    let mods = Modifiers::empty();
    let down = |app: &mut App, x, y| {
        let pos = Point::new(x, y);
        let target = app.runtime().window(window).unwrap().hit_test(pos).expect("real native hit-test");
        app.dispatch(
            window,
            Input::Interaction(InteractionEvent::PointerDown {
                id: pointer,
                kind: PointerKind::Mouse,
                target,
                pos,
                button: Button::Primary,
                mods,
            }),
        )
        .unwrap();
    };
    let up = |app: &mut App, x, y| {
        let pos = Point::new(x, y);
        let target = app.runtime().window(window).unwrap().hit_test(pos);
        app.dispatch(window, Input::Interaction(InteractionEvent::PointerUp { id: pointer, target, pos, mods })).unwrap();
    };

    down(app, 10.0, 10.0);
    assert!(matches!(app.runtime_mut().take_host_commands().as_slice(), [HostCommand::StartWindowDrag { window: id }] if *id == window));
    up(app, 10.0, 10.0);
    app.advance_to(Duration::from_millis(120)).unwrap();
    down(app, 11.0, 10.0);
    assert!(
        matches!(app.runtime_mut().take_host_commands().as_slice(), [HostCommand::ToggleMaximizeWindow { window: id }] if *id == window)
    );
    up(app, 11.0, 10.0);

    down(app, 85.0, 8.0);
    up(app, 85.0, 8.0);
    assert_eq!(presses.get(), 1);
    down(app, 178.0, 12.0);
    up(app, 178.0, 12.0);
    let tree = app.runtime().window(window).unwrap().tree();
    assert_eq!(tree.focus(), tree.test_ids("editor").first().copied());
    down(app, 265.0, 12.0);
    app.dispatch(window, Input::Interaction(InteractionEvent::PointerMove { id: pointer, pos: Point::new(283.0, 12.0), mods })).unwrap();
    up(app, 283.0, 12.0);
    assert_eq!(drags.get(), 2);
    down(app, 10.0, 42.0);
    up(app, 10.0, 42.0);
    let tree = app.runtime().window(window).unwrap().tree();
    assert_eq!(tree.focus(), tree.test_ids("ancestor").first().copied());
    assert!(app.runtime_mut().take_host_commands().is_empty());

    down(app, 10.0, 10.0);
    up(app, 50.0, 10.0);
    down(app, 10.0, 10.0);
    assert!(matches!(
        app.runtime_mut().take_host_commands().as_slice(),
        [HostCommand::StartWindowDrag { .. }, HostCommand::StartWindowDrag { .. }]
    ));
}

#[test]
fn window_feedback_and_restart_state_are_independent_by_key() {
    let mut driver = HeadlessDriver::new(Default::default());
    let app = driver.app_mut();
    app.runtime_mut().set_state_schema("windows");
    let metrics = Rc::new(Cell::new(None));
    let normal = WindowGeometry { position: Some(Point::new(40.0, 70.0)), size: Size::new(420.0, 290.0), scale: 2.0, ..Default::default() };
    let full = WindowGeometry { position: Some(Point::new(0.0, 0.0)), size: Size::new(1400.0, 920.0), fullscreen: true, ..normal };
    let inspector =
        WindowGeometry { position: Some(Point::new(190.0, 80.0)), size: Size::new(300.0, 200.0), scale: 1.0, ..Default::default() };
    let chrome = WindowChrome {
        titlebar_height: 38.0,
        leading_inset: 89.0,
        traffic_light_rect: Some(Rect::new(14.0, 9.0, 75.0, 16.0)),
        ..Default::default()
    };
    let options = |key: &str| WindowOptions { persistence_key: Some(key.into()), ..Default::default() };
    let main = app
        .open_window(options("main"), {
            let metrics = metrics.clone();
            move |build| {
                metrics.set(Some((build.window_geometry(), build.window_chrome(), build.viewport())));
                let geometry = build.window_geometry();
                let viewport = build.viewport();
                col()
                    .child(text(move |read: &mut ReadCx<'_>| format!("{}", read.get(geometry).size.width)).test_id("width"))
                    .child(text(move |read: &mut ReadCx<'_>| format!("{}", read.get(viewport).keyboard_inset)).test_id("keyboard"))
            }
        })
        .unwrap();
    let panel = app.open_window(options("panel"), |_| el()).unwrap();
    let startup_max = app.open_window(WindowOptions { maximized: true, ..options("startup-max") }, |_| el()).unwrap();
    app.runtime_mut().take_host_commands();
    app.dispatch(main, Input::Host(HostReply::GeometryChanged(normal))).unwrap();
    app.dispatch(main, Input::Host(HostReply::ChromeChanged(chrome))).unwrap();
    app.dispatch(panel, Input::Host(HostReply::GeometryChanged(inspector))).unwrap();
    app.dispatch(main, Input::Host(HostReply::GeometryChanged(full))).unwrap();
    let viewport = WindowViewport { size: full.size, scale: full.scale, safe_area: None, keyboard_inset: 270.0 };
    app.dispatch(main, Input::Host(HostReply::ViewportChanged(viewport))).unwrap();
    let zoomed = WindowGeometry { maximized: true, fullscreen: false, ..full };
    app.dispatch(startup_max, Input::Host(HostReply::GeometryChanged(zoomed))).unwrap();
    let (geometry_signal, chrome_signal, viewport_signal) = metrics.get().unwrap();
    assert_eq!(
        app.update_window(main, |write| (write.get(geometry_signal), write.get(chrome_signal), write.get(viewport_signal))).unwrap(),
        (full, chrome, viewport)
    );
    assert_eq!(viewport.visible_bounds(), Rect::new(0.0, 0.0, full.size.width, full.size.height - 270.0));
    let main_tree = app.runtime().window(main).unwrap().tree();
    let width_text = main_tree.text(main_tree.test_ids("width")[0]).unwrap();
    assert_eq!(width_text.text.as_ref(), "1400");
    assert_eq!(main_tree.text(main_tree.test_ids("keyboard")[0]).unwrap().text.as_ref(), "270");
    app.dispatch(main, Input::Host(HostReply::ViewportChanged(WindowViewport { keyboard_inset: 0.0, ..viewport }))).unwrap();
    let tree = app.runtime().window(main).unwrap().tree();
    assert_eq!(tree.text(tree.test_ids("keyboard")[0]).unwrap().text.as_ref(), "0");
    assert_eq!(app.runtime().window(panel).unwrap().window_geometry(), inspector);

    let request = WindowGeometryRequest { size: Some(Size::new(400.0, 300.0)), ..Default::default() };
    app.update_window(main, |write| {
        write.set_window_title("Renamed");
        write.set_window_geometry(request);
        write.minimize_window();
        write.toggle_maximize_window();
        write.focus_window(panel);
    })
    .unwrap();
    let commands = app.runtime_mut().take_host_commands();
    assert!(matches!(commands.as_slice(), [
        HostCommand::SetWindowGeometry { window: size_window, geometry },
        HostCommand::MinimizeWindow { window: minimized },
        HostCommand::ToggleMaximizeWindow { window: zoomed },
        HostCommand::FocusWindow { window: focused },
        HostCommand::UpdateWindowPresentation { window: title_window, title, .. },
    ] if *title_window == main && title.as_ref() == "Renamed" && *size_window == main && *geometry == request && *minimized == main && *zoomed == main && *focused == panel));
    app.close_window(main).unwrap();
    let saved: RestartState = serde_json::from_slice(&serde_json::to_vec(&app.export_persisted()).unwrap()).unwrap();
    assert_eq!(saved.window_geometry["main"], full);
    assert_eq!(saved.normal_window_geometry["main"], normal);
    assert_eq!(saved.normal_window_geometry["startup-max"].size, WindowOptions::default().viewport.size);

    let mut restarted = HeadlessDriver::new(Default::default());
    let app = restarted.app_mut();
    assert!(!app.import_persisted(saved.clone()));
    app.runtime_mut().set_state_schema("windows");
    assert!(app.import_persisted(saved));
    let reopened_panel = app.open_window(options("panel"), |_| el()).unwrap();
    let reopened_main = app.open_window(options("main"), |_| el()).unwrap();
    let reopened_max = app.open_window(options("startup-max"), |_| el()).unwrap();
    assert_eq!(app.runtime().window(reopened_panel).unwrap().window_geometry(), inspector);
    let restored = app.runtime().window(reopened_main).unwrap();
    assert_eq!(restored.window_geometry(), full);
    assert_eq!(restored.options().position, normal.position);
    assert_eq!(restored.options().viewport.size, normal.size);
    let restored_max = app.runtime().window(reopened_max).unwrap();
    assert_eq!(restored_max.window_geometry(), zoomed);
    assert!(restored_max.options().maximized);
    assert_eq!(restored_max.options().viewport.size, WindowOptions::default().viewport.size);
}
