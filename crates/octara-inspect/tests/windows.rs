use octara::{HostCommand, HostReply, Input, InteractionEvent, PointerId, PointerKind, prelude::*};
use octara_inspect::HeadlessDriver;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

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
                text(move |read: &mut ReadCx<'_>| format!("{}", read.get(geometry).size.width)).test_id("width")
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
    let usable = Size::new(full.size.width, full.size.height - 270.0);
    app.dispatch(main, Input::Host(HostReply::ViewportChanged(WindowViewport { size: usable, scale: full.scale, safe_area: None })))
        .unwrap();
    let zoomed = WindowGeometry { maximized: true, fullscreen: false, ..full };
    app.dispatch(startup_max, Input::Host(HostReply::GeometryChanged(zoomed))).unwrap();
    let (geometry_signal, chrome_signal, viewport_signal) = metrics.get().unwrap();
    assert_eq!(
        app.update_window(main, |write| (write.get(geometry_signal), write.get(chrome_signal), write.get(viewport_signal))).unwrap(),
        (full, chrome, usable)
    );
    let main_tree = app.runtime().window(main).unwrap().tree();
    let width_text = main_tree.text(main_tree.test_ids("width")[0]).unwrap();
    assert_eq!(width_text.text.as_ref(), "1400");
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
        HostCommand::SetWindowTitle { window: title_window, title },
        HostCommand::SetWindowGeometry { window: size_window, geometry },
        HostCommand::MinimizeWindow { window: minimized },
        HostCommand::ToggleMaximizeWindow { window: zoomed },
        HostCommand::FocusWindow { window: focused },
    ] if *title_window == main && title == "Renamed" && *size_window == main && *geometry == request && *minimized == main && *zoomed == main && *focused == panel));
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
