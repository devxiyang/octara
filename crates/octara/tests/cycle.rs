use octara::model::geometry::{LayoutBox, Point, WindowViewport};
use octara::model::input::ContactTarget;
use octara::prelude::*;
use octara::runtime::{CommitOutput, HostEngine, ProjectError, ProjectFeedback};
use octara::tree::{NodeId, Tree, TreeDelta, TreeRev};
use octara::{EditorEvent, EditorSession, HostReply, Input, TextEdit};
use std::cell::Cell;
use std::rc::Rc;

#[derive(Default)]
struct Probe {
    fail: Cell<bool>,
    projects: Cell<usize>,
    commits: Cell<usize>,
    rollbacks: Cell<usize>,
    created: Cell<usize>,
    shortcut_modifier: Cell<Modifiers>,
    hit: Cell<Option<NodeId>>,
    reduced_motion_updates: Cell<usize>,
}

struct ProbeEngine(Rc<Probe>);

impl HostEngine for ProbeEngine {
    fn project(&mut self, _: &Tree, delta: &TreeDelta, _: WindowViewport, _: f64) -> Result<ProjectFeedback, ProjectError> {
        self.0.projects.set(self.0.projects.get() + 1);
        self.0.created.set(delta.created.len());
        if self.0.fail.get() {
            return Err(ProjectError("projection deliberately aborted".into()));
        }
        Ok(ProjectFeedback::default())
    }

    fn commit(&mut self) -> CommitOutput {
        let frame = self.0.commits.get() + 1;
        self.0.commits.set(frame);
        CommitOutput { frame: Some(frame as u64), ..Default::default() }
    }

    fn rollback(&mut self) {
        self.0.rollbacks.set(self.0.rollbacks.get() + 1);
    }

    fn hit_test(&self, tree: &Tree, _: Point) -> Option<ContactTarget> {
        self.0.hit.get().map(|node| ContactTarget { hit: tree.guard(node), action: None })
    }

    fn shortcut_modifier(&self) -> Modifiers {
        self.0.shortcut_modifier.get()
    }

    fn geometry(&self, _: NodeId) -> Option<LayoutBox> {
        None
    }

    fn set_reduced_motion(&mut self, _: bool) {
        self.0.reduced_motion_updates.set(self.0.reduced_motion_updates.get() + 1);
    }
}

fn app() -> (App, Rc<Probe>) {
    let probe = Rc::new(Probe::default());
    let engine = probe.clone();
    (App::with_namespace([11, 19], move |_, _| Box::new(ProbeEngine(engine.clone()))), probe)
}

#[test]
fn system_preferences_are_initialized_and_shared_reactively() {
    let (mut app, probe) = app();
    let initial = SystemPreferences { color_scheme: ColorScheme::Dark, reduced_motion: true };
    app.runtime_mut().set_system_preferences(initial).unwrap();
    let preferences = app.build(|build| build.system_preferences()).unwrap();
    let first = app
        .open_window(WindowOptions::default(), move |build| {
            assert_eq!(build.sample(preferences), initial);
            el()
        })
        .unwrap();
    let second = app
        .open_window(WindowOptions::default(), |build| {
            let preferences = build.system_preferences();
            text(move |read: &mut ReadCx<'_>| if read.get(preferences).reduced_motion { "reduced" } else { "full" }).test_id("motion")
        })
        .unwrap();
    assert_eq!(probe.reduced_motion_updates.get(), 2);

    let next = SystemPreferences::default();
    let receipt = app.dispatch(first, Input::Host(HostReply::SystemPreferencesChanged(next))).unwrap();
    assert!(receipt.delivered && receipt.handled);
    assert_eq!(app.update(|write| write.get(preferences)).unwrap(), next);
    let tree = app.window(second).unwrap().runtime().tree();
    let motion_label = tree.test_ids("motion")[0];
    assert_eq!(tree.text(motion_label).unwrap().text.as_ref(), "full");
    assert_eq!(probe.reduced_motion_updates.get(), 4);

    let projects = probe.projects.get();
    app.dispatch(second, Input::Host(HostReply::SystemPreferencesChanged(next))).unwrap();
    assert_eq!(probe.projects.get(), projects);
}

#[test]
fn close_requests_can_be_vetoed_confirmed_or_allowed() {
    let (mut app, _) = app();
    let pending = app.build(|build| build.signal(None)).unwrap();
    let cleanup = Rc::new(Cell::new(false));
    let [guarded, default] = AppDefinition::new()
        .window(
            WindowDefinition::new(WindowOptions::default(), {
                let cleanup = cleanup.clone();
                move |build| {
                    build.on_cleanup(move |_| cleanup.set(true));
                    el()
                }
            })
            .on_close_request(move |write| {
                write.set(pending, write.window_id());
                false
            }),
        )
        .window(WindowDefinition::new(WindowOptions::default(), |_| el()))
        .launch(&mut app, None)
        .unwrap()
        .try_into()
        .unwrap();
    app.runtime_mut().take_host_commands();

    let receipt = app.dispatch(guarded, Input::Host(HostReply::CloseRequested)).unwrap();
    assert!(receipt.delivered && receipt.handled);
    assert!(app.window(guarded).is_some());
    assert!(!cleanup.get());
    assert!(app.runtime_mut().take_host_commands().is_empty());
    app.update(|write| {
        let id = write.get(pending).unwrap();
        assert_eq!(id, guarded);
        write.close_window(id);
    })
    .unwrap();
    assert!(cleanup.get());
    assert!(app.window(guarded).is_none());
    assert!(matches!(app.runtime_mut().take_host_commands().as_slice(), [octara::HostCommand::Closed(id)] if *id == guarded));

    app.dispatch(default, Input::Host(HostReply::CloseRequested)).unwrap();
    assert!(app.window(default).is_none());

    let opted_in = app.open_window(WindowOptions::default(), |_| el()).unwrap();
    app.window(opted_in).unwrap().on_close_request(|_| true);
    app.dispatch(opted_in, Input::Host(HostReply::CloseRequested)).unwrap();
    assert!(app.window(opted_in).is_none());
}

#[test]
fn utility_styles_and_builder_overrides_resolve_inside_theme_scopes() {
    use octara::model::style::LayoutLength::Px;

    const UTILITIES: TwStyle = tw!("flex items-center justify-between gap-2 px-[13px] py-0.5 bg-surface/80 text-sm text-muted \
         rounded-md border border-border/60 hover:bg-surface-hover focus-within:border-focus dark:bg-slate-900/80 \
         max-w-3xs md:max-w-7xl font-extrabold leading-snug shadow-2xs overflow-y-auto");
    let (mut app, _) = app();
    let mut options = WindowOptions::default();
    options.viewport.size.width = 767.0;
    let at_md = WindowViewport { size: Size { width: 768.0, ..options.viewport.size }, ..options.viewport };
    let dark = Theme::dark();
    let light = Theme::light();
    let id = app
        .open_window(options, |_| {
            col()
                .theme_scope(Theme::light())
                .child(
                    col().theme_scope(Theme::dark()).child(el().tw(UTILITIES).test_id("dark")).child(
                        input()
                            .value("text")
                            .tw(tw!("rounded-full border-t-2 border-r border-b-4 border-l-0 transition-colors \
                                 focus-within:ring-2 focus-within:ring-offset-2 focus-within:ring-focus md:pointer-events-auto"))
                            .pointer_events_none()
                            .light(|s| s.opacity(0.2))
                            .dark(|s| s.opacity(0.9))
                            .hover(|s| s.opacity(0.8))
                            .pressed(|s| s.opacity(0.7))
                            .focus_visible(|s| s.opacity(0.6))
                            .focus_within(|s| s.border_color(ColorToken::Focus))
                            .disabled_style(|s| s.opacity(0.5))
                            .group_hover("card", |s| s.opacity(0.4))
                            .test_id("editor"),
                    ),
                )
                .child(el().theme_patch(ThemePatch { spacing: Some(10.0), ..ThemePatch::default() }).tw(UTILITIES).test_id("light"))
        })
        .unwrap();
    let tree = app.window(id).unwrap().runtime().tree();
    let dark_node = tree.test_ids("dark")[0];
    let light_node = tree.test_ids("light")[0];
    let editor = tree.test_ids("editor")[0];
    let style = tree.visual(dark_node);
    assert_eq!(style.background, Some(palette::SLATE.s900.with_alpha(0.8)));
    assert_eq!(style.font_size, 14.0);
    assert_eq!(style.line_height, 14.0 * 1.375);
    assert_eq!(style.font_weight, 800);
    assert_eq!(style.radius, [6.0; 4]);
    let layout = tree.layout_style(dark_node);
    assert_eq!(layout.padding.left, Px(13.0));
    assert_eq!(layout.padding.top, Px(2.0));
    assert_eq!(layout.max_size.width, Some(Px(256.0)));
    assert_eq!(layout.overflow_y, Overflow::Auto);
    assert!(tree.scroll(dark_node).is_some());
    assert_eq!(tree.layout_style(light_node).padding.top, Px(5.0));
    assert_eq!(tree.visual(light_node).background, Some(light.get(ColorToken::Surface).with_alpha(0.8)));
    assert_eq!(tree.visual(editor).opacity, 0.9);
    assert_eq!(tree.visual(editor).radius, [f32::INFINITY; 4]);
    assert_eq!(tree.visual(editor).motion.color, Curve::Fade(0.15));
    assert_eq!(tree.layout_style(editor).border, Edges { top: Px(2.0), right: Px(1.0), bottom: Px(4.0), left: Px(0.0) });
    assert_eq!(tree.visual(editor).ring, None);
    assert!(tree.node(editor).unwrap().flags.contains(octara::tree::NodeFlags::POINTER_EVENTS_NONE));
    app.dispatch(id, Input::Host(HostReply::ViewportChanged(at_md))).unwrap();
    app.update_window(id, |w| w.focus("editor")).unwrap();
    let tree = app.window(id).unwrap().runtime().tree();
    assert_eq!(tree.layout_style(dark_node).max_size.width, Some(Px(1280.0)));
    assert_eq!(tree.visual(editor).border, dark.get(ColorToken::Focus));
    assert_eq!(tree.visual(editor).ring, Some(Ring { width: 2.0, offset: 2.0, color: dark.get(ColorToken::Focus) }));
    assert!(!tree.node(editor).unwrap().flags.contains(octara::tree::NodeFlags::POINTER_EVENTS_NONE));
}

#[test]
fn rich_editor_binding_settles_value_and_ranges_in_the_same_revision() {
    use std::sync::Arc;

    let (mut app, _) = app();
    let value = app.build(|build| build.signal("tag".to_owned())).unwrap();
    let id = app
        .open_window(WindowOptions::default(), move |_| {
            col()
                .child(
                    input()
                        .value(value)
                        .rich_text(EditorRichText {
                            atoms: Arc::from([AtomicRange { range: 0..3, kind: "tag".into() }]),
                            ..Default::default()
                        })
                        .test_id("static"),
                )
                .child(
                    input()
                        .bind_value(value)
                        .rich_text(move |read: &mut ReadCx<'_>| {
                            let end = read.get(value).len();
                            EditorRichText { atoms: Arc::from([AtomicRange { range: 0..end, kind: "tag".into() }]), ..Default::default() }
                        })
                        .test_id("dynamic"),
                )
        })
        .unwrap();
    app.update_window(id, |write| write.focus("dynamic")).unwrap();
    let tree = app.window(id).unwrap().runtime().tree();
    let static_editor = tree.editor(tree.test_ids("static")[0]).unwrap();
    assert_eq!(static_editor.atoms()[0].range, 0..3);

    let active = EditorSession::active(tree).unwrap();
    let (session, expected_rev) = (active.id(), active.editor().document_rev());
    app.dispatch(
        id,
        Input::Editor(EditorEvent::ProposeEdit {
            session,
            expected_rev,
            edit: TextEdit::Insert { text: "!".into(), replacement: Some(3..3) },
        }),
    )
    .unwrap();

    let tree = app.window(id).unwrap().runtime().tree();
    let active = EditorSession::active(tree).unwrap();
    assert_eq!(active.id(), session);
    assert_eq!(active.editor().document_rev(), expected_rev + 1);
    assert_eq!(active.editor().value(), "tag!");
    assert_eq!(active.editor().atoms()[0].range, 0..4);
    assert_eq!(tree.editor(tree.test_ids("static")[0]).unwrap().atoms()[0].range, 0..3);
}

#[test]
fn typography_utilities_inherit_and_builder_overrides_reset_them() {
    let (mut app, _) = app();
    let id = app
        .open_window(WindowOptions::default(), |_| {
            col()
                .tw(tw!("tracking-wider italic underline line-through tabular-nums"))
                .text_size(20.0)
                .child(text("inherit").text_size(30.0).test_id("inherit-typography"))
                .child(
                    text("override")
                        .italic(false)
                        .underline(false)
                        .strike(false)
                        .letter_spacing(-2.0)
                        .tabular_nums(false)
                        .test_id("override-typography"),
                )
                .child(text("reset").tw(tw!("not-italic no-underline tracking-[-1.5px] normal-nums")).test_id("reset-typography"))
        })
        .unwrap();
    let tree = app.window(id).unwrap().runtime().tree();
    let style = |name| tree.visual(tree.test_ids(name)[0]).text_style();
    let inherited = style("inherit-typography");
    assert!(inherited.italic && inherited.underline && inherited.strike && inherited.tabular_nums);
    assert_eq!(inherited.letter_spacing, 1.0);
    for (name, spacing) in [("override-typography", -2.0), ("reset-typography", -1.5)] {
        let style = style(name);
        assert!(!style.italic && !style.underline && !style.strike && !style.tabular_nums);
        assert_eq!(style.letter_spacing, spacing);
    }
}

#[test]
fn mount_200_static_nodes_commits_exactly_one_transaction() {
    let (mut app, probe) = app();
    let id = app
        .open_window(WindowOptions::default(), |_| {
            let mut root = el().p(16).test_id("mount");
            for _ in 0..199 {
                root = root.child(el().p(4));
            }
            root
        })
        .unwrap();
    let window = app.window(id).unwrap();
    let tree = window.runtime().tree();
    let [root] = tree.test_ids("mount") else { panic!("mounted root is uniquely indexed") };
    assert_eq!(tree.subtree(*root).count(), 200);
    assert_eq!(window.runtime().transaction_count(), 1);
    assert_eq!(tree.revision(), TreeRev(1));
    assert_eq!(probe.projects.get(), 1);
    assert_eq!(probe.commits.get(), 1);
    // The host's first delta also includes the two built-in roots.
    assert_eq!(probe.created.get(), 202);
    println!("200 authored nodes: {} transaction, {} projection", window.runtime().transaction_count(), probe.projects.get());
}

#[test]
fn a_projection_error_rolls_back_and_retries_the_retained_delta() {
    let (mut app, probe) = app();
    let title = app.build(|cx| cx.signal(String::from("initial"))).unwrap();
    let id = app
        .open_window(WindowOptions::default(), move |_| el().test_id("title").child(move |read: &mut ReadCx<'_>| read.get(title)))
        .unwrap();
    let before = app.window(id).unwrap().runtime().observation_revision();
    probe.fail.set(true);
    assert!(app.update(|write| write.set(title, "updated".to_owned())).is_err());
    assert_eq!(probe.rollbacks.get(), 1);
    assert_eq!(probe.commits.get(), 1);
    assert_eq!(app.window(id).unwrap().runtime().observation_revision(), before);
    probe.fail.set(false);
    app.runtime_mut().settle().unwrap();
    assert_eq!(probe.commits.get(), 2);
    assert!(app.window(id).unwrap().runtime().settled().is_some());
}

#[test]
fn native_commands_resolve_live_test_ids_in_the_current_window() {
    use octara::model::identity::NativeId;
    use serde_json::json;

    let (mut app, _) = app();
    let id = app
        .open_window(WindowOptions::default(), |_| {
            el().child(native_view("test.view").test_id("web"))
                .child(text("not an island").test_id("text"))
                .child(native_view("test.view").id("authoring-only"))
                .child(native_view("test.view").test_id("ambiguous"))
                .child(native_view("test.view").test_id("ambiguous"))
        })
        .unwrap();
    let second = app.open_window(WindowOptions::default(), |_| native_view("test.view").test_id("web")).unwrap();
    let send = |write: &mut WriteCx<'_>| write.send_native_command("web", "evaluate_js", json!({"script": "42"}));
    assert!(!app.update(send).unwrap());
    assert!(!app.update_window(id, send).unwrap());

    let tree = app.runtime().window(id).unwrap().tree();
    let node = tree.test_ids("web")[0];
    let target = tree.guard(node);
    app.dispatch(id, Input::Host(HostReply::NativeLive { target, live: true })).unwrap();
    assert!(!app.update_window(second, send).unwrap());
    assert!(
        app.update_window(id, |write| {
            for target in ["missing", "text", "authoring-only", "ambiguous"] {
                assert!(!write.send_native_command(target, "reload", json!(null)));
            }
            send(write)
        })
        .unwrap()
    );
    assert!(app.runtime_mut().take_host_commands().into_iter().any(|command| {
        matches!(command, octara::HostCommand::NativeCommand { window, command }
            if window == id && command.id == NativeId::from(node) && command.name == "evaluate_js" && command.data == json!({"script": "42"}))
    }));

    app.update_window(id, |write| {
        assert!(send(write));
        write.close_window(id);
    })
    .unwrap();
    assert!(
        !app.runtime_mut().take_host_commands().into_iter().any(|command| matches!(command, octara::HostCommand::NativeCommand { .. }))
    );
}

#[test]
fn native_commands_from_watches_and_tasks_publish_only_after_settle() {
    use serde_json::Value;
    use std::time::Duration;

    let (mut app, probe) = app();
    let requested = app.build(|build| build.signal(false)).unwrap();
    let id = app
        .open_window(WindowOptions::default(), move |build| {
            build.watch(
                move |read| read.get(requested),
                |write, requested, _| {
                    if *requested {
                        assert!(write.send_native_command("web", "go_back", Value::Null));
                        write.spawn("later native command", |task| async move {
                            task.sleep(Duration::from_millis(1)).await;
                            assert_eq!(task.write(|write| write.send_native_command("web", "go_forward", Value::Null)), Some(true));
                        });
                    }
                },
            );
            native_view("test.view").test_id("web")
        })
        .unwrap();
    let tree = app.runtime().window(id).unwrap().tree();
    let target = tree.guard(tree.test_ids("web")[0]);
    app.dispatch(id, Input::Host(HostReply::NativeLive { target, live: true })).unwrap();

    let commands = |app: &mut App| {
        app.runtime_mut()
            .take_host_commands()
            .into_iter()
            .filter_map(|command| match command {
                octara::HostCommand::NativeCommand { window, command } => Some((window, command.name)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    app.update(|write| write.set(requested, true)).unwrap();
    assert_eq!(commands(&mut app), [(id, "go_back".into())]);
    app.advance_to(Duration::from_millis(1)).unwrap();
    assert_eq!(commands(&mut app), [(id, "go_forward".into())]);

    probe.fail.set(true);
    app.runtime_mut().invalidate_projection(id).unwrap();
    assert!(app.update_window(id, |write| assert!(write.send_native_command("web", "reload", Value::Null))).is_err());
    assert!(commands(&mut app).is_empty());
    probe.fail.set(false);
    app.runtime_mut().settle().unwrap();
    assert!(commands(&mut app).is_empty());
}

fn menu(name: &str, enabled: bool) -> Menu {
    Menu {
        name: name.into(),
        items: vec![MenuItem::Action { id: "save".into(), label: "Save".into(), enabled, checked: None, shortcut: None }],
    }
}

fn published_menus(app: &mut App) -> Vec<Vec<Menu>> {
    app.runtime_mut()
        .take_host_commands()
        .into_iter()
        .filter_map(|command| match command {
            octara::HostCommand::Menus(menus) => Some(menus),
            _ => None,
        })
        .collect()
}

#[test]
fn window_menus_publish_on_settle_and_follow_the_active_window() {
    let (mut app, probe) = app();
    let enabled = app.build(|b| b.signal(true)).unwrap();
    let ids = AppDefinition::new()
        .window(
            WindowDefinition::new(WindowOptions::default(), move |_| {
                text(move |read: &mut ReadCx<'_>| if read.get(enabled) { "Ready" } else { "Waiting" })
            })
            .menus(move |read| vec![menu("First", read.get(enabled))])
            .menus(|_| vec![menu("Extra", true)]),
        )
        .window(WindowDefinition::new(WindowOptions::default(), |build| {
            build.menus(|_| vec![menu("Second", true)]);
            el()
        }))
        .launch(&mut app, None)
        .unwrap();

    assert_eq!(published_menus(&mut app), [vec![menu("First", true), menu("Extra", true)]]);
    probe.fail.set(true);
    assert!(app.update(|write| write.set(enabled, false)).is_err());
    assert!(published_menus(&mut app).is_empty());
    probe.fail.set(false);
    app.runtime_mut().settle().unwrap();
    assert_eq!(published_menus(&mut app), [vec![menu("First", false), menu("Extra", true)]]);

    app.dispatch(ids[1], octara::Input::Interaction(octara::InteractionEvent::WindowActive(true))).unwrap();
    assert_eq!(published_menus(&mut app), [vec![menu("Second", true)]]);
    app.close_window(ids[1]).unwrap();
    assert_eq!(published_menus(&mut app), [vec![menu("First", false), menu("Extra", true)]]);
    app.close_window(ids[0]).unwrap();
    assert_eq!(published_menus(&mut app), [Vec::<Menu>::new()]);
}

#[test]
fn window_shortcuts_use_the_host_modifier_and_retire_with_the_scope() {
    for (primary, other) in [(Modifiers::META, Modifiers::CTRL), (Modifiers::CTRL, Modifiers::META)] {
        let (mut app, probe) = app();
        probe.shortcut_modifier.set(primary);
        let (mounted, hits) = app.build(|b| (b.signal(true), b.signal(0))).unwrap();
        let [id] = AppDefinition::new()
            .window(
                WindowDefinition::new(WindowOptions::default(), move |_| {
                    col().child(input().autofocus()).child(show(
                        move |r| r.get(mounted),
                        move |b| {
                            b.on_chord("mod+k".parse().unwrap(), move |w| w.update(hits, |value| *value += 10));
                            text("Shortcut scope")
                        },
                    ))
                })
                .on_chord("mod+k".parse().unwrap(), move |w| w.update(hits, |value| *value += 100))
                .on_chord("mod+k".parse().unwrap(), move |w| w.update(hits, |value| *value += 1)),
            )
            .launch(&mut app, None)
            .unwrap()[..]
        else {
            panic!("one launched window")
        };

        let key = |modifier| octara::Input::Interaction(octara::InteractionEvent::Key(KeyEvent::new(Key::Char('K'), modifier)));
        assert!(!app.dispatch(id, key(other)).unwrap().handled);
        assert!(app.dispatch(id, key(primary)).unwrap().handled);
        assert_eq!(app.update(|w| w.get(hits)).unwrap(), 10);

        app.update(|w| w.set(mounted, false)).unwrap();
        assert!(app.dispatch(id, key(primary)).unwrap().handled);
        assert_eq!(app.update(|w| w.get(hits)).unwrap(), 11);
    }
}

#[test]
fn file_drop_accepts_the_nearest_receiver_and_rejects_invalid_delivery() {
    let (mut app, probe) = app();
    let (shown, disabled, modal, received) =
        app.build(|b| (b.signal(true), b.signal(false), b.signal(false), b.signal(Vec::<DroppedFile>::new()))).unwrap();
    let id = app
        .open_window(WindowOptions::default(), move |_| {
            col()
                .test_id("outer-zone")
                .on_file_drop(|_, _| panic!("outer receiver received a drop intended for the inner receiver"))
                .child(show(
                    move |r| r.get(shown),
                    move |_| {
                        col()
                            .test_id("inner-zone")
                            .disabled(disabled)
                            .on_file_drop(move |w, files| w.set(received, files.to_vec()))
                            .child(text("Drop files here").test_id("zone-content"))
                    },
                ))
                .portal(move |r| r.get(modal), |_| el().layer(LayerPolicy::modal(), |_| {}))
        })
        .unwrap();
    let other = app.open_window(WindowOptions::default(), |_| el()).unwrap();
    let window = app.window(id).unwrap();
    let tree = window.runtime().tree();
    probe.hit.set(Some(tree.test_ids("zone-content")[0]));
    let target = tree.guard(tree.test_ids("inner-zone")[0]);
    let outer = tree.guard(tree.test_ids("outer-zone")[0]);
    let at = point(10.0, 10.0);
    assert_eq!(window.runtime().file_drop_target(at), Some(target));

    let files = vec![DroppedFile::Path("/local/note.txt".into()), DroppedFile::Name("図.png".into())];
    let drop = || Input::Host(HostReply::FilesDropped { target, files: files.clone() });
    let delivered = app.dispatch(id, drop()).unwrap();
    assert!(delivered.delivered && delivered.handled);
    assert_eq!(app.update(|w| w.get(received)).unwrap(), files);

    let empty = app.dispatch(id, Input::Host(HostReply::FilesDropped { target, files: vec![] })).unwrap();
    assert!(!empty.delivered && !empty.handled);
    assert!(!app.dispatch(other, drop()).unwrap().delivered);

    app.update(|w| w.set(disabled, true)).unwrap();
    assert_eq!(app.window(id).unwrap().runtime().file_drop_target(at), Some(outer));
    assert!(!app.dispatch(id, drop()).unwrap().delivered);

    app.update(|w| {
        w.set(disabled, false);
        w.set(modal, true);
    })
    .unwrap();
    assert_eq!(app.window(id).unwrap().runtime().file_drop_target(at), None);
    assert!(!app.dispatch(id, drop()).unwrap().delivered);

    app.update(|w| {
        w.set(modal, false);
        w.set(shown, false);
    })
    .unwrap();
    app.update(|w| w.set(shown, true)).unwrap();
    assert_eq!(app.window(id).unwrap().runtime().file_drop_target(at), None);
    assert!(!app.dispatch(id, drop()).unwrap().delivered);
    assert_eq!(app.update(|w| w.get(received)).unwrap(), files);
}
