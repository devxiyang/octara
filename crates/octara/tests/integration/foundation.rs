//! Public-core composition proof: this integration binary has no octara-kit dependency.
use octara::app::{Event, InputEvent};
use octara::prelude::*;
use octara_primitives::text::TextEdit;

const OPTIONS: [&str; 2] = ["北京", "上海"];

fn choose(value: Signal<String>, open: Signal<bool>, index: usize) {
    if !is_composing("query") {
        value.set(OPTIONS[index].into());
        open.set(false);
    }
}

#[ui]
fn custom_action(content: El) -> El {
    row().role(Role::Button).label("发送").focusable().h(44).px_3().gap_2().child(content)
}

#[ui]
fn foundation_view() -> View {
    let value = state("query", String::new);
    let open = state("suggestions", || false);
    let active = state("active", || 0usize);
    let sent = state("sent", || 0usize);
    let observed = state("observed", String::new);
    let invalid = memo(move || value.get().trim().is_empty());
    on_command("suggestions.open", move || open.set(true));
    col()
        .size_full()
        .p_4()
        .gap_2()
        .child(
            col()
                .id("query-field")
                .w_full()
                .gap_1()
                .child(text("搜索").id("query-label").min_h(32).on_press(|| focus("query")))
                .child(
                    input()
                        .id("query")
                        .label("搜索")
                        .placeholder("输入中文城市")
                        .w_full()
                        .h(44)
                        .required(true)
                        .invalid(invalid)
                        .description(move || if invalid.get() { "请输入城市".into() } else { "支持中文输入和城市建议".into() })
                        .value(value)
                        .expanded(open)
                        .active_descendant(move || open.get().then(|| format!("option-{}", active.get())))
                        .behavior(|b| {
                            b.on_input(move |s| {
                                value.set(s);
                                open.set(true);
                            })
                            .on_blur(move || open.set(false))
                            .on_key(move |event| {
                                if !event.modifiers.is_empty() {
                                    return false;
                                }
                                match event.key {
                                    Key::Down | Key::Up => {
                                        active.set((active.get() + 1) % OPTIONS.len());
                                        open.set(true);
                                        true
                                    }
                                    Key::Enter if open.get() => {
                                        choose(value, open, active.get());
                                        true
                                    }
                                    Key::Escape if open.get() => {
                                        open.set(false);
                                        true
                                    }
                                    _ => false,
                                }
                            })
                        })
                        .on_input(move |_| observed.set(value.get()))
                        .overlay(
                            move || open.get(),
                            Placement::BELOW.fit(true),
                            move || {
                                col()
                                    .id("suggestions")
                                    .role(Role::ListBox)
                                    .label("城市建议")
                                    .preserve_focus_on_press(true)
                                    .w(200)
                                    .bg(Token::Surface)
                                    .children(OPTIONS.into_iter().enumerate().map(move |(index, label)| {
                                        text(label)
                                            .id(format!("option-{index}"))
                                            .role(Role::ListBoxOption)
                                            .selected(move || active.get() == index)
                                            .h(36)
                                            .on_press(move || choose(value, open, index))
                                    }))
                            },
                        ),
                )
                .child(show(move || invalid.get(), || text("请输入城市").id("query-error").role(Role::Alert).live(Live::Polite))),
        )
        .child(
            custom_action(row().child(text("→").id("send-icon")).child(text(move || format!("发送 {}", sent.get())).id("send-caption")))
                .id("send")
                .disabled(invalid)
                .on_press(move || sent.update(|n| *n += 1)),
        )
        .child(text(move || sent.get().to_string()).id("sent"))
        .child(text(observed).id("observed"))
        .into()
}

fn native(window: &Window) -> octara::platform::TextInput {
    window.platform_window().as_any().downcast_ref::<octara::platform::HeadlessWindow>().unwrap().text_input.clone().unwrap()
}

#[test]
fn custom_control_roots_work_across_pointer_keyboard_semantics_and_validation() {
    let mut app = Window::headless(foundation_view);
    let before = app.inspect();
    let send_target = before.find("send").unwrap().target.clone();
    let send = app.kernel().find("send").unwrap();
    let query_id = app.kernel().find("query").unwrap();
    let ax = app.kernel().accessibility_tree(None, 1.0);
    for (id, name) in [(send, "发送"), (query_id, "搜索")] {
        let node = &ax.nodes.iter().find(|(target, _)| *target == octara::kernel::a11y::a11y_id(id)).unwrap().1;
        assert_eq!(node.label(), Some(name));
    }
    let query = before.find("query").unwrap();
    let target = query.target.clone();
    assert_eq!(query.name.as_deref(), Some("搜索"));
    assert_eq!((query.required, query.invalid), (Some(true), Some(true)));
    assert_eq!(query.description.as_deref(), Some("请输入城市"));
    assert!(app.dispatch(Event::Press("send".into())).is_err());
    assert!(!app.event(InputEvent::Accessibility {
        target: octara::kernel::a11y::a11y_id(send),
        action: octara::accesskit::Action::Click,
        data: None,
    }));
    app.tap("query-label").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("query"));
    app.type_text("中文");
    assert_eq!(app.kernel().text_of("observed").as_deref(), Some("中文"), "application callbacks follow component binding");
    let after = app.inspect();
    assert_eq!(after.find("query").unwrap().target, target);
    assert_eq!(after.find("query").unwrap().invalid, Some(false));
    assert!(after.find("query-error").is_none());
    app.press_key("escape").unwrap();
    app.tap("send-caption").unwrap();
    app.dispatch(Event::Focus("send".into())).unwrap();
    app.press_key("enter").unwrap();
    app.dispatch(Event::Press("send".into())).unwrap();
    assert!(app.event(InputEvent::Accessibility {
        target: octara::kernel::a11y::a11y_id(send),
        action: octara::accesskit::Action::Click,
        data: None,
    }));
    assert_eq!(app.kernel().text_of("sent").as_deref(), Some("4"));
    assert_eq!(app.inspect().find("send").unwrap().target, send_target, "visual label updates keep the action root");
    assert!(app.layout_issues().is_empty(), "{:?}", app.layout_issues());
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn suggestions_preserve_the_real_editor_and_yield_to_chinese_composition() {
    let mut app = Window::headless(foundation_view);
    app.tap("query-label").unwrap();
    let initial = native(&app);
    let target = app.inspect().find("query").unwrap().target.clone();
    app.invoke("suggestions.open").unwrap();
    let mark = TextEdit::Mark { text: "nihao".into(), selection: Some(5..5), replacement: None };
    assert!(app.apply_text_edit(initial.id, &initial.document.unwrap(), &mark));
    let draft = native(&app).document.unwrap();
    let active = app.inspect().find("query").unwrap().active_descendant.clone();
    app.press_key("down").unwrap();
    assert_eq!(app.inspect().find("query").unwrap().active_descendant, active, "IME owns navigation first");
    app.tap("option-1").unwrap();
    assert_eq!(native(&app).document, Some(draft.clone()), "pointer choices cannot commit over preedit");
    assert_eq!(app.inspect().focus.as_deref(), Some("query"));
    let commit = TextEdit::Insert { text: "你好".into(), replacement: None };
    assert!(app.apply_text_edit(initial.id, &draft, &commit));
    assert_eq!(app.kernel().value_of("query").as_deref(), Some("你好"));
    for width in [260.0, 480.0] {
        app.resize(width, 360.0);
        let snapshot = app.inspect();
        assert_eq!(snapshot.find("query").unwrap().target, target);
        let panel = snapshot.find("suggestions").unwrap().bounds;
        assert!(panel[0] >= 0.0 && panel[0] + panel[2] <= width);
    }
    app.press_key("down").unwrap();
    assert_eq!(app.inspect().find("query").unwrap().active_descendant.as_deref(), Some("option-1"));
    app.press_key("enter").unwrap();
    assert_eq!(app.kernel().value_of("query").as_deref(), Some("上海"));
    assert_eq!(app.inspect().focus.as_deref(), Some("query"));
    assert!(app.inspect().find("suggestions").is_none());
    app.invoke("suggestions.open").unwrap();
    app.tap("option-0").unwrap();
    assert_eq!(app.kernel().value_of("query").as_deref(), Some("北京"));
    assert_eq!(app.inspect().find("query").unwrap().target, target);
    assert!(app.layout_issues().is_empty(), "{:?}", app.layout_issues());
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}
