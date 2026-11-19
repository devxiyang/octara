//! Components: semantics, controlled values, interaction looks, select with its overlay.

use octara::app::Event;
use octara::prelude::*;
use octara_kit::*;

fn tok(t: Token) -> Color {
    Theme::light().get(t)
}

fn view() -> View {
    let count = state("count", || 0);
    let locked = state("locked", || false);
    let wifi = state("wifi", || false);
    let name = state("name", || "Ada".to_string());
    let size = state("size", || Some("2".to_owned()));
    let starred = state("starred", || false);
    col()
        .child(primary_button("Save").id("save").disabled(move || locked.get()).on_press(move || count.update(|n| *n += 1)))
        .child(button("Lock").id("lock").on_press(move || locked.update(|l| *l = !*l)))
        .child(text(move || format!("saved {}", count.get())).id("count"))
        .child(switch(wifi).id("wifi").label("Wi-Fi"))
        .child(
            // A model that is not a Signal: getter and setter.
            checkbox((move || starred.get(), move |v| starred.set(v))).id("star"),
        )
        .child(text_field(name).id("name").label("Name"))
        .child(text(move || format!("hello {}", name.get())).id("greeting"))
        .child(select(
            "size",
            "Size",
            size,
            || vec![Choice::new("1", "Small"), Choice::new("2", "Medium"), Choice::new("3", "Large")],
            button,
        ))
        .child(text(move || format!("size {}", size.get().unwrap_or_default())).id("size-out"))
        .into()
}

#[test]
fn buttons_have_roles_states_and_refuse_when_disabled() {
    let mut app = Window::headless(view);
    let k = app.kernel();
    assert_eq!(k.role_of("save"), Some(Role::Button));
    assert_eq!(k.visual_of("save").unwrap().background, Some(tok(Token::Accent)));
    drop(k);
    app.dispatch(Event::Hover("save".into(), true)).unwrap();
    assert_eq!(app.kernel().visual_of("save").unwrap().background, Some(tok(Token::AccentHover)));
    app.dispatch(Event::Press("save".into())).unwrap();
    app.dispatch(Event::Press("lock".into())).unwrap();
    assert_eq!(app.kernel().visual_of("save").unwrap().opacity, 0.5);
    assert!(app.dispatch(Event::Press("save".into())).is_err());
    assert_eq!(app.kernel().text_of("count").unwrap(), "saved 1");
}

#[test]
fn switch_and_checkbox_are_controlled() {
    let mut app = Window::headless(view);
    // The switch is a press area around its track; the thumb sits in the track.
    let track = |app: &Window| {
        let k = app.kernel();
        k.children(k.find("wifi").unwrap())[0]
    };
    let thumb_x = |app: &Window| {
        let k = app.kernel();
        k.rect(k.children(track(app))[0]).0
    };
    let off = thumb_x(&app);
    assert_eq!(app.kernel().role_of("wifi"), Some(Role::Switch));
    assert_eq!(app.kernel().checked_of("wifi"), Some(false));
    app.dispatch(Event::Toggle("wifi".into())).unwrap();
    assert_eq!(app.kernel().checked_of("wifi"), Some(true));
    assert_eq!(app.kernel().visual(track(&app)).background, Some(tok(Token::Accent)));
    assert!(thumb_x(&app) > off, "the thumb moves to the end");
    app.dispatch(Event::Toggle("star".into())).unwrap();
    assert_eq!(app.kernel().checked_of("star"), Some(true));
}

#[test]
fn text_field_is_bound_both_ways() {
    let mut app = Window::headless(view);
    assert_eq!(app.kernel().value_of("name").unwrap(), "Ada");
    app.dispatch(Event::Input("name".into(), "Grace".into())).unwrap();
    assert_eq!(app.kernel().text_of("greeting").unwrap(), "hello Grace");
    app.dispatch(Event::Focus("name".into())).unwrap();
    assert_eq!(app.kernel().visual_of("name").unwrap().border, tok(Token::Focus));
}

#[test]
fn select_opens_a_listbox_and_picks() {
    let mut app = Window::headless(view);
    let k = app.kernel();
    assert_eq!(k.role_of("size"), Some(Role::ComboBox));
    assert_eq!(k.value_of("size").unwrap(), "Medium");
    assert!(!k.exists("size-list"));
    drop(k);
    app.dispatch(Event::Press("size".into())).unwrap();
    let k = app.kernel();
    assert_eq!(k.expanded_of("size"), Some(true));
    assert_eq!(k.role_of("size-list"), Some(Role::ListBox));
    assert!(k.state_of("size-option-2").unwrap().contains(State::SELECTED));
    let (_, ty, _, th) = k.rect(k.find("size").unwrap());
    let (_, ly, _, _) = k.rect(k.find("size-popover").unwrap());
    assert_eq!(ly, ty + th + 4.0, "opens below the trigger");
    drop(k);
    app.app().hot_reload();
    assert!(app.kernel().exists("size-list"), "open state survives hot reload");
    app.dispatch(Event::Press("size-option-3".into())).unwrap();
    let k = app.kernel();
    assert_eq!(k.text_of("size-out").unwrap(), "size 3");
    assert_eq!(k.value_of("size").unwrap(), "Large");
    assert!(!k.exists("size-list"), "picking closes it");
    assert_eq!(k.expanded_of("size"), Some(false));
}

#[test]
fn no_diagnostics() {
    let mut app = Window::headless(view);
    app.dispatch(Event::Press("size".into())).unwrap();
    app.app().hot_reload();
    let d = app.app().diagnostics();
    assert!(d.is_empty(), "{d:?}");
}

#[test]
fn select_works_from_the_keyboard() {
    let mut app = Window::headless(view);
    app.dispatch(Event::Focus("size".into())).unwrap();
    app.press_key("enter").unwrap();
    assert!(app.kernel().exists("size-list"));
    let focused = |app: &Window, id: &str| app.kernel().state_of(id).unwrap().contains(State::FOCUSED);
    let active = |app: &Window| app.inspect().find("size-list").unwrap().active_descendant.clone();
    assert!(focused(&app, "size-list"));
    assert_eq!(active(&app).as_deref(), Some("size-option-2"));
    app.press_key("down").unwrap();
    assert!(focused(&app, "size-list"));
    assert_eq!(active(&app).as_deref(), Some("size-option-3"));
    app.press_key("enter").unwrap();
    assert_eq!(app.kernel().text_of("size-out").unwrap(), "size 3");
    assert!(!app.kernel().exists("size-list"));
    assert!(focused(&app, "size"), "focus goes back to the select");
    // Escape closes it without picking; a press outside does too.
    app.press_key("space").unwrap();
    app.press_key("up").unwrap();
    app.press_key("escape").unwrap();
    assert!(!app.kernel().exists("size-list"));
    assert_eq!(app.kernel().text_of("size-out").unwrap(), "size 3");
    app.press_key("enter").unwrap();
    app.click_at(790.0, 590.0);
    assert!(!app.kernel().exists("size-list"));
    // The list floats on a shadow.
    app.press_key("enter").unwrap();
    let popup = app.kernel().find("size-popover").unwrap();
    assert!(app.display_list().items().iter().any(|i| matches!(i, octara_primitives::paint::Item::Shadow(s) if s.source == popup)));
}

/// A kit component is a leaf: its elements' source is where the app called it, and the
/// component it records is the one the app wrote (`primary_button`, not the `button` inside).
#[test]
fn kit_components_point_at_the_app() {
    let w = Window::headless(|| col().child(primary_button("Save").id("save")));
    let snap = w.inspect();
    let save = snap.find("save").unwrap();
    let made = save.component.as_ref().expect("made by the kit");
    assert_eq!((made.name.as_str(), made.module.as_str()), ("primary_button", "octara_kit"));
    assert_eq!(save.source.as_deref(), Some(made.called_at.as_str()), "the app's line");
    assert!(made.called_at.contains("components.rs:"), "{}", made.called_at);
}

#[test]
fn an_icon_button_is_named_by_its_label_and_tooltip() {
    let pressed = state_cell();
    let p = pressed.clone();
    let mut app = Window::headless(move || {
        let p = p.clone();
        row().child(icon_button(octara_icons::COPY, "Copy").id("copy").on_press(move || p.set(p.get() + 1)))
    });
    let k = app.kernel();
    let n = k.find("copy").unwrap();
    assert_eq!((k.role(n), k.label(n)), (Role::Button, Some("Copy")));
    assert_eq!(k.tooltip(n), Some("Copy"));
    drop(k);
    app.tap("copy").unwrap();
    assert_eq!(pressed.get(), 1);
}

fn state_cell() -> std::rc::Rc<std::cell::Cell<u32>> {
    std::rc::Rc::default()
}

#[test]
fn a_spinner_steps_round_on_the_clock() {
    let app = Window::headless(|| row().child(spinner().id("busy")).child(spinner().id("busy2")));
    assert!(app.app().diagnostics().is_empty(), "two spinners side by side: {:?}", app.app().diagnostics());
    let k = app.kernel();
    let n = k.find("busy").unwrap();
    assert_eq!((k.role(n), k.label(n)), (Role::ProgressIndicator, Some("Loading")));
    let before = format!("{:?}", k.canvas(n));
    drop(k);
    app.app().advance(std::time::Duration::from_millis(100));
    let k = app.kernel();
    assert_ne!(format!("{:?}", k.canvas(k.find("busy").unwrap())), before, "the arc moved");
    drop(k);
    assert!(app.app().run_until_idle(std::time::Duration::from_secs(1)).is_ok(), "it does not keep the app busy");
}

#[test]
fn chips_and_callouts() {
    let removed = state_cell();
    let r = removed.clone();
    let mut app = Window::headless(move || {
        let r = r.clone();
        col()
            .child(chip("draft").id("tag"))
            .child(removable_chip("report.pdf", |remove| remove.id("remove-file").on_press(move || r.set(r.get() + 1))).id("file"))
            .child(
                callout(PromptLevel::Critical, "The reply failed", Some("The connection closed."))
                    .id("error")
                    .child(button("Retry").id("retry")),
            )
            .child(callout(PromptLevel::Info, "Saved", None).id("info"))
    });
    let k = app.kernel();
    let file = k.find("file").unwrap();
    let b = *k.children(file).last().unwrap();
    assert_eq!(k.label(b), Some("Remove report.pdf"));
    assert_eq!(k.test_id(b), Some("remove-file"));
    drop(k);
    app.tap("remove-file").unwrap();
    assert_eq!(removed.get(), 1);
    let k = app.kernel();
    assert_eq!(k.role_of("error"), Some(Role::Alert));
    assert_eq!(k.role_of("info"), Some(Role::Group));
    let error = k.find("error").unwrap();
    assert_eq!(k.test_id(*k.children(error).last().unwrap()), Some("retry"), "actions go after the text");
}

#[test]
fn a_collapsible_opens_and_closes_its_content() {
    let mut app = Window::headless(|| {
        let open = state("open", || false);
        col()
            .child(collapsible("thinking", open, text("Thought for 3 s"), || text("Step one.").id("step")))
            .child(button("Open").id("open").on_press(move || open.set(true)))
    });
    let k = app.kernel();
    assert_eq!(k.role_of("thinking-trigger"), Some(Role::Button));
    assert_eq!(k.expanded_of("thinking-trigger"), Some(false));
    assert!(k.find("step").is_none(), "closed: its content is not built");
    drop(k);
    app.tap("thinking-trigger").unwrap();
    assert!(app.kernel().find("step").is_some());
    app.tap("thinking-trigger").unwrap();
    assert!(app.kernel().find("step").is_none());
    app.tap("open").unwrap();
    assert!(app.kernel().find("step").is_some(), "the caller holds whether it is open");
}

#[test]
fn a_list_selects_and_moves_by_arrow_keys() {
    let mut app = Window::headless(|| {
        let selected = state("selected", || 0u32);
        list().id("chats").child(list_header("Today").id("today")).children((0..3u32).map(move |i| {
            list_item(text(format!("Chat {i}")))
                .id(format!("chat-{i}"))
                .selected(move || selected.get() == i)
                .on_press(move || selected.set(i))
                .on_focus(move || selected.set(i))
        }))
    });
    let k = app.kernel();
    assert_eq!(k.role_of("chats"), Some(Role::ListBox));
    assert_eq!(k.role_of("today"), Some(Role::Heading));
    assert_eq!(k.role_of("chat-1"), Some(Role::ListBoxOption));
    assert!(k.state_of("chat-0").unwrap().contains(State::SELECTED));
    drop(k);
    app.tap("chat-1").unwrap();
    assert!(app.kernel().state_of("chat-1").unwrap().contains(State::SELECTED));
    app.press_key("down").unwrap();
    let k = app.kernel();
    assert!(k.state_of("chat-2").unwrap().contains(State::FOCUSED), "the arrow moved focus down");
    assert!(k.state_of("chat-2").unwrap().contains(State::SELECTED), "and selection followed it");
    drop(k);
    for key in ["down", "left"] {
        app.press_key(key).unwrap();
        assert_eq!(app.inspect().focus.as_deref(), Some("chat-2"), "{key} keeps focus at the last list item");
    }
}

#[test]
fn a_split_resizes_its_leading_pane() {
    let mut app = Window::headless(|| {
        let width = state("width", || 240.0f32);
        split("panes", width, el().id("side").size_full(), el().id("main").size_full())
            .child(text(move || format!("{}", width.get())).id("w"))
    });
    let pane = |app: &Window, id: &str| {
        let k = app.kernel();
        k.rect(k.find(id).unwrap()).2
    };
    assert_eq!(pane(&app, "side"), 240.0);
    let id = "panes-divider";
    assert_eq!(app.kernel().role_of(id), Some(Role::Splitter));
    app.drag(id, 60.0, 0.0).unwrap();
    assert_eq!(pane(&app, "side"), 300.0, "dragged 60 to the right");
    app.drag(id, -400.0, 0.0).unwrap();
    assert_eq!(pane(&app, "side"), 120.0, "no narrower than 120");
    app.tap(id).unwrap();
    app.press_key("right").unwrap();
    assert_eq!(pane(&app, "side"), 128.0);
}

#[test]
fn a_splitter_uses_accepted_model_values_and_keeps_its_component_handlers() {
    let mut window = Window::headless(|| {
        let width = state("width", || 240.0f32);
        let reject = state("reject", || false);
        let drags = state("drags", || 0);
        let keys = state("keys", || 0);
        on_command("reject", move || reject.set(true));
        on_command("external", move || width.set(200.0));
        row()
            .size_full()
            .child(
                splitter(
                    "grip",
                    (
                        move || width.get(),
                        move |next: f32| {
                            if !reject.get() {
                                width.set(next.clamp(180.0, 300.0));
                            }
                        },
                    ),
                )
                .on_drag(move |_| drags.update(|n| *n += 1))
                .on_key(move |_| {
                    keys.update(|n| *n += 1);
                    true
                }),
            )
            .child(text(move || drags.get().to_string()).id("drags"))
            .child(text(move || keys.get().to_string()).id("keys"))
    });
    let value = |window: &Window| window.kernel().value_of("grip").unwrap();
    window.drag("grip", 100.0, 0.0).unwrap();
    assert_eq!(value(&window), "300");
    window.drag("grip", -10.0, 0.0).unwrap();
    assert_eq!(value(&window), "290", "new drags start from the accepted width, not the last proposal");
    assert_ne!(window.kernel().text_of("drags").as_deref(), Some("0"));
    window.tap("grip").unwrap();
    window.press_key("shift+left").unwrap();
    assert_eq!(value(&window), "250");
    assert_eq!(window.kernel().text_of("keys").as_deref(), Some("0"), "resize keys are consumed by the component");
    window.press_key("home").unwrap();
    assert_eq!(window.kernel().text_of("keys").as_deref(), Some("1"));
    window.invoke("external").unwrap();
    window.press_key("right").unwrap();
    assert_eq!(value(&window), "208", "keyboard uses the current model value");
    window.invoke("reject").unwrap();
    window.drag("grip", 50.0, 0.0).unwrap();
    window.press_key("right").unwrap();
    assert_eq!(value(&window), "208", "rejected proposals never become displayed state");
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}
