#![cfg(not(target_arch = "wasm32"))]

use std::time::Duration;

use octara::{
    ContactTarget, HostReply, Input, InteractionEvent, NodeId, PointerId, PointerKind, TargetGuard, Tree, TurnReceipt, WindowRuntime,
    prelude::*,
};
use octara_headless::{
    Control,
    close::close_button,
    collection::Choice,
    dialog::DialogState,
    menu::{ContextMenuState, MenuSection, MenuState},
    popover::{PopoverGroup, PopoverState},
};
use octara_inspect::{AgentAction, AgentCommand, AgentRequest, HeadlessDriver, protocol::Delivery};

struct Mounted {
    driver: HeadlessDriver,
    id: WindowId,
}

impl Mounted {
    fn new<V: IntoView>(mut driver: HeadlessDriver, view: impl FnOnce(&mut BuildCx<'_>) -> V + 'static) -> Self {
        let viewport = WindowViewport { size: Size::new(340.0, 240.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
        let id = driver.app_mut().open_window(WindowOptions { viewport, ..Default::default() }, view).unwrap();
        Self { driver, id }
    }

    fn window(&self) -> &WindowRuntime {
        self.driver.app().runtime().window(self.id).unwrap()
    }

    fn tree(&self) -> &Tree {
        self.window().tree()
    }

    fn node(&self, name: &str) -> NodeId {
        let [node] = self.tree().test_ids(name) else { panic!("expected one node named {name}") };
        *node
    }

    fn guard(&self, name: &str) -> TargetGuard {
        self.tree().guard(self.node(name))
    }

    fn focus_is(&self, name: &str) {
        assert_eq!(self.tree().focus(), Some(self.node(name)), "expected focus on {name}");
    }

    fn send(&mut self, input: Input) -> TurnReceipt {
        self.driver.dispatch(self.id, input).unwrap()
    }

    fn a11y(&mut self, name: &str, action: A11yAction) {
        assert!(self.send(Input::Host(HostReply::AccessibilityAction { target: self.guard(name), action })).handled);
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        assert!(self.send(Input::Interaction(InteractionEvent::Key(KeyEvent::new(key, modifiers)))).handled);
    }

    fn click(&mut self, pos: Point, button: Button) {
        let target = self.window().hit_test(pos).expect("hit target");
        let id = PointerId(1);
        let down = InteractionEvent::PointerDown { id, kind: PointerKind::Mouse, target, pos, button, mods: Modifiers::empty() };
        assert!(self.send(Input::Interaction(down)).delivered);
        let up = InteractionEvent::PointerUp { id, target: self.window().hit_test(pos), pos, mods: Modifiers::empty() };
        assert!(self.send(Input::Interaction(up)).delivered);
    }

    fn rect(&self, name: &str) -> Rect {
        self.window().geometry(self.node(name)).unwrap().rect
    }
}

#[test]
fn nested_popover_escape_outside_press_and_logical_scope() {
    let mut mounted = Mounted::new(HeadlessDriver::default(), |build| {
        let dialog = DialogState::new(build, Control::Uncontrolled(false));
        dialog.root(
            text("Open dialog").test_id("opener").role(Role::Button).focusable().on_press(move |write| dialog.set_open(write, true)),
            move |build| {
                let group = PopoverGroup::new();
                let popover = PopoverState::new(build, Control::Uncontrolled(false))
                    .in_group(group)
                    .focus_trap(FocusTrapFeatures::INITIAL_FOCUS | FocusTrapFeatures::RESTORE_FOCUS | FocusTrapFeatures::FOCUS_LOCK);
                let sibling = PopoverState::new(build, Control::Uncontrolled(false)).in_group(group);
                let trigger = popover.button(text("Open popover").w(100).h(32).test_id("trigger").autofocus());
                dialog.panel(
                    col()
                        .test_id("dialog")
                        .position(Position::Absolute)
                        .left(20)
                        .top(20)
                        .w(130)
                        .child(dialog.title(text("Dialog title").test_id("title")))
                        .child(dialog.description(text("Dialog description").test_id("description")))
                        .child(popover.overlay(trigger, Placement::RIGHT, None, move |_build| {
                            popover.panel(
                                col()
                                    .w(100)
                                    .test_id("popover")
                                    .child(text("Inside").h(32).test_id("inside").focusable().autofocus())
                                    .child(text("Portal owner").h(32).overlay_part(
                                        Placement::RIGHT,
                                        |_| true,
                                        |_| close_button(text("Nested portal").w(80).h(30).test_id("nested")),
                                    )),
                            )
                        }))
                        .child(sibling.button(text("Sibling trigger").w(100).h(32).test_id("sibling")))
                        .child(text("Other control").w(100).h(32).test_id("external").focusable().on_press(|_| {})),
                )
            },
        )
    });
    mounted.a11y("opener", A11yAction::Focus);
    mounted.a11y("opener", A11yAction::Press);
    mounted.focus_is("trigger");
    assert_eq!(mounted.tree().front_modal(), Some(mounted.node("dialog")));
    assert_eq!(mounted.tree().overlay(mounted.node("dialog")).unwrap().mount.unwrap().owner, mounted.node("opener"));

    mounted.a11y("trigger", A11yAction::Press);
    mounted.focus_is("inside");
    let panel = mounted.node("popover");
    assert_eq!(mounted.tree().overlay(panel).unwrap().mount.unwrap().owner, mounted.node("trigger"));
    assert!(mounted.tree().is_in_interaction_scope(mounted.node("nested"), panel));
    assert!(mounted.tree().is_in_interaction_scope(mounted.node("sibling"), panel));
    mounted.a11y("nested", A11yAction::Focus);
    mounted.focus_is("nested");
    mounted.a11y("sibling", A11yAction::Focus);
    assert!(mounted.tree().is_shown(panel));
    mounted.focus_is("sibling");

    mounted.a11y("inside", A11yAction::Focus);
    mounted.key(Key::Escape, Modifiers::empty());
    assert!(mounted.tree().test_ids("popover").is_empty());
    assert_eq!(mounted.tree().front_modal(), Some(mounted.node("dialog")));
    mounted.focus_is("trigger");

    mounted.a11y("trigger", A11yAction::Press);
    mounted.focus_is("inside");
    let external = mounted.rect("external");
    let panel = mounted.rect("dialog");
    let pos = point(panel.x + external.x + 10.0, panel.y + external.y + 10.0);
    assert_eq!(mounted.window().hit_test(pos).unwrap().hit.node(), Some(mounted.node("external")));
    mounted.click(pos, Button::Primary);
    assert!(mounted.tree().test_ids("popover").is_empty());
    assert_eq!(mounted.tree().front_modal(), Some(mounted.node("dialog")));
    mounted.focus_is("external");

    mounted.a11y("trigger", A11yAction::Focus);
    mounted.a11y("trigger", A11yAction::Press);
    mounted.a11y("nested", A11yAction::Focus);
    mounted.a11y("nested", A11yAction::Press);
    assert!(mounted.tree().test_ids("popover").is_empty());
    mounted.focus_is("trigger");

    mounted.key(Key::Escape, Modifiers::empty());
    assert!(mounted.tree().test_ids("dialog").is_empty());
    mounted.focus_is("opener");
}

#[test]
fn reopening_an_exiting_popover_rejects_old_input_targets_and_completion() {
    let mut driver = HeadlessDriver::default();
    let (open, presses) = driver.app_mut().build(|build| (build.signal(false), build.signal(0))).unwrap();
    let mut mounted =
        Mounted::new(driver, move |build| {
            let popover = PopoverState::new(build, open).focus_trap(FocusTrapFeatures::INITIAL_FOCUS | FocusTrapFeatures::RESTORE_FOCUS);
            popover.overlay(popover.button(text("Open").w(100).h(30).test_id("opener")), Placement::BELOW, None, move |_| {
                popover.panel(
                    col().w(120).test_id("panel").style(StyleProp::OpacityMotion(Curve::Fade(0.5))).bg(rgb(0x123456)).child(
                        text("Action").h(30).test_id("action").focusable().on_press(move |write| write.update(presses, |n| *n += 1)),
                    ),
                )
            })
        });
    mounted.a11y("opener", A11yAction::Focus);
    mounted.a11y("opener", A11yAction::Press);
    mounted.focus_is("action");
    let old = mounted.guard("action");
    let old_panel = mounted.node("panel");
    mounted.driver.app_mut().update(|write| write.set(open, false)).unwrap();
    let ticket = mounted.tree().pending_exit(old_panel).expect("panel is still fading");
    assert!(mounted.tree().node(old.node().unwrap()).is_some());
    mounted.focus_is("opener");

    let stale = mounted.driver.execute(AgentRequest::new(
        Some(mounted.id),
        AgentCommand::Act { target: old, action: AgentAction::Accessibility { action: A11yAction::Press } },
    ));
    assert_eq!(stale.code.as_deref(), Some("STALE_TARGET"));
    assert_eq!(stale.delivery, Delivery::Refused);
    let down = InteractionEvent::PointerDown {
        id: PointerId(1),
        kind: PointerKind::Mouse,
        target: ContactTarget { hit: old, action: Some(old) },
        pos: point(20.0, 50.0),
        button: Button::Primary,
        mods: Modifiers::empty(),
    };
    assert!(!mounted.send(Input::Interaction(down)).delivered);

    mounted.driver.app_mut().update(|write| write.set(open, true)).unwrap();
    let replacement = mounted.guard("action");
    assert_ne!(old, replacement);
    assert!(!mounted.send(Input::Host(HostReply::ExitFinished(ticket))).delivered);
    mounted.driver.advance_to(Duration::from_secs(1)).unwrap();
    mounted.focus_is("action");
    assert_eq!(mounted.guard("action"), replacement);
    assert!(mounted.tree().node(old_panel).is_none());
    mounted.a11y("action", A11yAction::Press);
    assert_eq!(mounted.driver.app_mut().update(|write| write.get(presses)).unwrap(), 1);
}

#[test]
fn context_menu_uses_pointer_position_and_keyboard_owner_geometry() {
    let mut mounted = Mounted::new(HeadlessDriver::default(), |build| {
        let rows = build.list_from([("customize", Choice::new("Customize"))]);
        let menu = MenuState::new(build, Control::Uncontrolled(false), rows);
        let context = ContextMenuState::new(build, menu);
        context.trigger(text("Canvas").w(200).h(90).test_id("source"), Placement::BELOW.gap(0.0), None, move |_| {
            let action = menu.item(text("Customize").h(30).test_id("action"), &"customize");
            menu.items(col().w(100).test_id("menu").child(action))
        })
    });
    mounted.a11y("source", A11yAction::Focus);
    let cursor = point(105.0, 40.0);
    mounted.click(cursor, Button::Secondary);
    mounted.focus_is("action");
    let pointer_panel = mounted.rect("menu");
    assert_eq!((pointer_panel.x, pointer_panel.y), (cursor.x, cursor.y));
    let cursor = point(140.0, 20.0);
    mounted.click(cursor, Button::Secondary);
    let moved_panel = mounted.rect("menu");
    assert_eq!((moved_panel.x, moved_panel.y), (cursor.x, cursor.y));
    mounted.key(Key::Escape, Modifiers::empty());
    mounted.focus_is("source");

    for (key, modifiers) in [(Key::F(10), Modifiers::SHIFT), (Key::Menu, Modifiers::empty())] {
        mounted.key(key, modifiers);
        mounted.focus_is("action");
        assert_eq!(mounted.rect("menu").y, mounted.rect("source").bottom());
        mounted.key(Key::Escape, Modifiers::empty());
        mounted.focus_is("source");
    }
    mounted.a11y("source", A11yAction::ShowContextMenu);
    mounted.focus_is("action");
    assert_eq!(mounted.rect("menu").y, mounted.rect("source").bottom());
}

#[test]
fn menu_keyboard_navigation_and_submenu_restore_focus() {
    let mut driver = HeadlessDriver::default();
    let checked = driver.app_mut().build(|build| build.signal(Checked::Mixed)).unwrap();
    let mut mounted = Mounted::new(driver, move |build| {
        let rows = build.list_from([
            ("check", Choice::new("Check")),
            ("disabled", Choice::new("Disabled").disabled(true)),
            ("mounted-disabled", Choice::new("More disabled")),
            ("more", Choice::new("More")),
            ("radio", Choice::new("Radio")),
            ("last-disabled", Choice::new("Last disabled")),
        ]);
        let menu = MenuState::new(build, Control::Uncontrolled(false), rows);
        menu.overlay(menu.button(text("Menu").w(100).h(30).test_id("button")), Placement::BELOW, None, move |build| {
            let nested = build.list_from([("action", Choice::new("Action"))]);
            let child = MenuState::new(build, Control::Uncontrolled(false), nested);
            let section = MenuSection::new();
            let selected = build.signal(None);
            menu.items(
                col().w(120).test_id("menu").child(
                    menu.section(
                        col()
                            .child(menu.heading(text("Choices"), section))
                            .child(menu.checkbox_item(text("Check").h(25).test_id("check"), &"check", checked))
                            .child(menu.item(text("Disabled").h(25).test_id("disabled"), &"disabled"))
                            .child(menu.item(text("More Disabled").h(25).disabled(true), &"mounted-disabled"))
                            .child(menu.separator(el().h(1)))
                            .child(menu.submenu(text("More").h(25).test_id("more"), &"more", child, |_, child| {
                                let action = child.item(text("Action").h(25).test_id("action"), &"action");
                                child.items(col().w(100).test_id("submenu").child(action))
                            }))
                            .child(menu.radio_item(text("Radio").h(25).test_id("radio"), &"radio", selected))
                            .child(menu.item(text("Last Disabled").h(25).disabled(true), &"last-disabled")),
                        section,
                    ),
                ),
            )
        })
    });
    mounted.a11y("button", A11yAction::Focus);
    mounted.key(Key::Up, Modifiers::empty());
    mounted.focus_is("radio");
    mounted.key(Key::Home, Modifiers::empty());
    mounted.focus_is("check");
    mounted.key(Key::Down, Modifiers::empty());
    mounted.focus_is("more");
    mounted.key(Key::Right, Modifiers::empty());
    mounted.focus_is("action");
    mounted.key(Key::Left, Modifiers::empty());
    mounted.focus_is("more");
    assert!(mounted.tree().test_ids("submenu").is_empty());
    mounted.key(Key::Enter, Modifiers::empty());
    mounted.focus_is("action");
    mounted.key(Key::Enter, Modifiers::empty());
    mounted.focus_is("button");
    assert!(mounted.tree().test_ids("menu").is_empty());

    mounted.key(Key::Down, Modifiers::empty());
    mounted.focus_is("check");
    mounted.key(Key::Char('m'), Modifiers::empty());
    mounted.focus_is("more");
    mounted.key(Key::Home, Modifiers::empty());
    mounted.key(Key::Char('r'), Modifiers::empty());
    mounted.focus_is("radio");
    mounted.key(Key::Char('c'), Modifiers::empty());
    mounted.key(Key::Home, Modifiers::empty());
    mounted.key(Key::Enter, Modifiers::empty());
    mounted.focus_is("button");
    assert_eq!(mounted.driver.app_mut().update(|write| write.get(checked)).unwrap(), Checked::Checked);
}

#[test]
fn submenu_dismissal_clears_open_bindings_and_blocks_vetoed_reopen() {
    let mut driver = HeadlessDriver::default();
    let (root_open, child_open, leaf_open, veto, leaf_binding) = driver
        .app_mut()
        .build(|build| {
            let root = build.signal(false);
            let child = build.signal(false);
            let leaf = build.signal(false);
            let veto = build.signal(false);
            let leaf_binding = build.binding(
                move |read| read.get(leaf),
                move |write, open| {
                    if open || !write.get(veto) {
                        write.set(leaf, open);
                    }
                },
            );
            (root, child, leaf, veto, leaf_binding)
        })
        .unwrap();
    let mut mounted = Mounted::new(driver, move |build| {
        let rows = build.list_from([("child", Choice::new("Child"))]);
        let menu = MenuState::new(build, root_open, rows);
        let child_rows = build.list_from([("leaf", Choice::new("Leaf"))]);
        let child = MenuState::new(build, child_open, child_rows);
        menu.overlay(menu.button(text("Menu").w(100).h(30).test_id("root-button")), Placement::BELOW, None, move |build| {
            let leaf_rows = build.list_from([("action", Choice::new("Action"))]);
            let leaf = MenuState::new(build, leaf_binding, leaf_rows);
            menu.items(col().w(90).test_id("root-menu").child(menu.submenu(
                text("Child").h(25).test_id("child-trigger"),
                &"child",
                child,
                move |_, child| {
                    child.items(col().w(90).test_id("child-menu").child(child.submenu(
                        text("Leaf").h(25).test_id("leaf-trigger"),
                        &"leaf",
                        leaf,
                        move |_, leaf| {
                            leaf.items(
                                col().w(90).test_id("leaf-menu").child(leaf.item(text("Action").h(25).test_id("leaf-action"), &"action")),
                            )
                        },
                    )))
                },
            )))
        })
    });
    let none = Modifiers::empty();
    mounted.a11y("root-button", A11yAction::Focus);
    mounted.key(Key::Down, none);
    mounted.key(Key::Right, none);
    mounted.key(Key::Right, none);
    mounted.focus_is("leaf-action");
    mounted.key(Key::Enter, none);
    let opened = mounted.driver.app_mut().update(|write| (write.get(root_open), write.get(child_open), write.get(leaf_open))).unwrap();
    assert_eq!(opened, (false, false, false));
    mounted.focus_is("root-button");

    mounted.key(Key::Down, none);
    mounted.focus_is("child-trigger");
    assert!(mounted.tree().test_ids("child-menu").is_empty());
    mounted.key(Key::Right, none);
    mounted.key(Key::Right, none);
    mounted.driver.app_mut().update(|write| write.set(root_open, false)).unwrap();
    let opened = mounted.driver.app_mut().update(|write| (write.get(child_open), write.get(leaf_open))).unwrap();
    assert_eq!(opened, (false, false));

    mounted.key(Key::Down, none);
    mounted.key(Key::Right, none);
    mounted.key(Key::Right, none);
    mounted
        .driver
        .app_mut()
        .update(|write| {
            write.set(veto, true);
            write.set(child_open, false);
        })
        .unwrap();
    assert!(mounted.driver.app_mut().update(|write| write.get(leaf_open)).unwrap());
    mounted.focus_is("child-trigger");
    mounted.key(Key::Escape, none);
    mounted.key(Key::Down, none);
    mounted.key(Key::Right, none);
    mounted.focus_is("leaf-trigger");
    assert!(mounted.tree().test_ids("leaf-menu").is_empty());
    assert_eq!(mounted.tree().semantics(mounted.node("leaf-trigger")).unwrap().expanded, Some(false));
    mounted.key(Key::Right, none);
    mounted.focus_is("leaf-action");
}
