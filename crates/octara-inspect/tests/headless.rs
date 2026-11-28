#![cfg(all(feature = "native", not(target_arch = "wasm32")))]

#[path = "headless/scrolling.rs"]
mod scrolling;

#[path = "headless/native_views.rs"]
mod native_views;

#[path = "headless/airspace.rs"]
mod airspace;

#[path = "headless/text_selection.rs"]
mod text_selection;

#[path = "headless/tooltip.rs"]
mod tooltip;

#[path = "headless/gestures.rs"]
mod gestures;

use octara::{
    prelude::*,
    runtime::{HandlerEvent, HandlerOutcome, HostCommand, Input},
    tree::{arbiter::InteractionPayload, overlay::LayerPolicy},
};
use octara_inspect::{
    AgentAction, AgentCommand, AgentReceipt, AgentRequest, HeadlessDriver, TargetRef,
    protocol::{Delivery, EditorSnapshot, TreeSnapshot},
    replay::{Channel, Event, Replay, Step},
};
use octara_model::{
    ask::{ServiceId, ServiceOutcome, ServiceRequest},
    capture::CaptureSource,
    input::{EditorEvent, HostReply, InteractionEvent, PointerId, PointerKind, TextEdit},
    motion::Curve,
    semantics::{A11yAction, TextPosition},
    style::{Overflow, StyleProp},
};
use octara_pipeline::NativeEngineConfig;

fn driver() -> HeadlessDriver {
    HeadlessDriver::with_namespace(
        [11, 29],
        NativeEngineConfig { check_layout_cache: true, check_paint_cache: true, check_a11y_cache: true, ..Default::default() },
    )
}

fn options() -> WindowOptions {
    WindowOptions {
        viewport: WindowViewport { size: Size::new(260.0, 180.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 },
        ..Default::default()
    }
}

fn run(driver: &mut HeadlessDriver, id: WindowId, command: AgentCommand) -> AgentReceipt {
    let request = AgentRequest::new(Some(id), command);
    let receipt = driver.execute(request.clone());
    octara_inspect::conformance::verify_receipt(&request, &receipt).unwrap();
    receipt
}

fn observe(driver: &mut HeadlessDriver, id: WindowId) -> TreeSnapshot {
    let receipt = run(driver, id, AgentCommand::Observe);
    assert!(receipt.ok && receipt.progress.settled, "{receipt:?}");
    receipt.snapshot.unwrap()
}

fn target(driver: &HeadlessDriver, id: WindowId, test_id: &str) -> TargetRef {
    let tree = driver.app().runtime().window(id).unwrap().tree();
    let [node] = tree.test_ids(test_id) else { panic!("expected a single node for {test_id}") };
    tree.guard(*node)
}

#[test]
fn authored_external_texture_updates_the_retained_image_without_loading_pixels() {
    use octara_model::paint::Item;
    let mut driver = driver();
    let first = ExternalTextureHandle::new(ExternalTextureKind::Metal, (40, 20), ());
    let texture = driver.app_mut().build(|b| b.signal(first.clone())).unwrap();
    let window = driver
        .app_mut()
        .open_window(options(), move |_| {
            image_source(move |r: &mut ReadCx<'_>| ImageSource::External(r.get(texture)))
                .w(100)
                .h(80)
                .object_fit(ObjectFit::Contain)
                .test_id("external")
        })
        .unwrap();
    let sample = |driver: &HeadlessDriver| {
        let runtime = driver.app().runtime().window(window).unwrap();
        let [node] = runtime.tree().test_ids("external") else { panic!("expected the external image node") };
        let slot = runtime.tree().media_source(*node).unwrap();
        let list = runtime.last_commit().unwrap().draw.as_deref().unwrap();
        let mut draws = list.items().into_iter().filter_map(|item| match item {
            Item::ExternalTexture(draw) => Some(draw),
            Item::Image(_) => panic!("external image became a CPU image"),
            _ => None,
        });
        let draw = draws.next().expect("external image reaches the scene");
        assert_eq!(draw.source, *node);
        assert!(draws.next().is_none());
        (slot.state.clone(), draw)
    };
    driver.advance_to(std::time::Duration::from_millis(1)).unwrap();
    let (state, initial) = sample(&driver);
    assert_eq!(state, ImageState::Ready { natural_size: (40, 20) });
    assert_eq!(initial.texture, first);
    assert_eq!(initial.rect, Rect::new(0.0, 15.0, 100.0, 50.0));

    let next = ExternalTextureHandle::new(ExternalTextureKind::Metal, (40, 20), ());
    driver.app_mut().update(|w| w.set(texture, next.clone())).unwrap();
    driver.advance_to(std::time::Duration::from_millis(2)).unwrap();
    assert_eq!(sample(&driver).1.texture, next);
}

#[test]
fn locale_and_text_scale_reach_logical_portals_and_update_native_layout() {
    let mut driver = driver();
    let window = driver
        .app_mut()
        .open_window(options(), |_| {
            row()
                .child(el().portal(
                    |_| true,
                    |_| {
                        col().child(
                            text("View 42")
                                .test_id("portal-text")
                                .ms(12)
                                .ps(6)
                                .border_e(2)
                                .text_size(16)
                                .line_height(LineHeight::Len(px(20.0))),
                        )
                    },
                ))
                .child(
                    el().text_direction(Direction::Ltr).portal(|_| true, |_| col().child(text("Fixed direction").test_id("fixed").ms(7))),
                )
        })
        .unwrap();
    let portal = target(&driver, window, "portal-text").node().unwrap();
    let fixed = target(&driver, window, "fixed").node().unwrap();
    let tree = driver.app().runtime().window(window).unwrap().tree();
    assert_eq!(tree.visual(portal).direction, Direction::Ltr);
    assert_eq!(tree.layout_style(portal).margin.left, Some(LayoutLength::Px(12.0)));

    let prefs = SystemPreferences { locale: "fa-IR".into(), text_scale: 1.5, ..SystemPreferences::default() };
    driver.dispatch(window, Input::Host(HostReply::SystemPreferencesChanged(prefs))).unwrap();
    let runtime = driver.app().runtime().window(window).unwrap();
    let tree = runtime.tree();
    let layout = runtime.geometry(portal).unwrap();
    assert_eq!(tree.visual(portal).direction, Direction::Rtl);
    assert_eq!(tree.visual(portal).font_size, 24.0);
    assert_eq!(tree.visual(portal).line_height, 30.0);
    assert_eq!(tree.layout_style(portal).margin.right, Some(LayoutLength::Px(12.0)));
    assert_eq!((layout.padding.left, layout.padding.right), (0.0, 6.0));
    assert_eq!((layout.border.left, layout.border.right), (2.0, 0.0));
    assert_eq!(tree.visual(fixed).direction, Direction::Ltr);
    assert_eq!(tree.layout_style(fixed).margin.left, Some(LayoutLength::Px(7.0)));
}

#[test]
fn tri_state_and_description_are_observed_on_the_authoring_node() {
    let mut driver = driver();
    let checked = driver.app_mut().build(|b| b.signal(Checked::Mixed)).unwrap();
    let window = driver
        .app_mut()
        .open_window(options(), move |_| {
            col()
                .child(el().test_id("check").role(Role::CheckBox).checked(checked).described_by("description"))
                .child(text("Changes apply to all rows").test_id("description").role(Role::Tooltip))
        })
        .unwrap();
    let check = target(&driver, window, "check");
    let description = target(&driver, window, "description");
    for value in [Checked::Mixed, Checked::Checked, Checked::Unchecked] {
        driver.app_mut().update(|w| w.set(checked, value)).unwrap();
        let snapshot = observe(&mut driver, window);
        let node = snapshot.nodes.iter().find(|node| node.target == check).unwrap();
        assert_eq!(node.checked, Some(value));
        assert_eq!(node.relations.described_by, Some(description));
        assert_eq!(node.role, Some(Role::CheckBox));
    }
}

fn act(driver: &mut HeadlessDriver, id: WindowId, target: TargetRef, action: AgentAction) -> AgentReceipt {
    run(driver, id, AgentCommand::Act { target, action })
}

fn accessible(driver: &mut HeadlessDriver, id: WindowId, target: TargetRef, action: A11yAction) {
    let receipt = act(driver, id, target, AgentAction::Accessibility { action });
    assert!(receipt.ok, "{receipt:?}");
    assert_eq!(receipt.handled, Some(true), "{receipt:?}");
}

fn active_editor(driver: &mut HeadlessDriver, id: WindowId) -> EditorSnapshot {
    observe(driver, id).editor.expect("active editor")
}

#[test]
fn reopening_an_exiting_overlay_rejects_old_agent_targets_and_completion() {
    let mut driver = driver();
    let open = driver.app_mut().build(|b| b.signal(false)).unwrap();
    let id = driver
        .app_mut()
        .open_window(options(), move |_| {
            el().w(80).h(30).focusable().test_id("exit-opener").overlay(
                Placement::BELOW,
                move |r| r.get(open),
                move |_| {
                    col()
                        .w(120)
                        .h(50)
                        .style(StyleProp::OpacityMotion(Curve::Fade(0.5)))
                        .bg(rgb(0x123456))
                        .layer(LayerPolicy::modal(), move |w| w.set(open, false))
                        .child(el().h(30).focusable().autofocus().test_id("exit-target"))
                        .test_id("exit-panel")
                },
            )
        })
        .unwrap();
    let opener = target(&driver, id, "exit-opener");
    accessible(&mut driver, id, opener, A11yAction::Focus);
    driver.app_mut().update(|w| w.set(open, true)).unwrap();
    let old = target(&driver, id, "exit-target");
    let old_panel = target(&driver, id, "exit-panel").node().unwrap();
    assert_eq!(observe(&mut driver, id).focus, Some(old));

    driver.app_mut().update(|w| w.set(open, false)).unwrap();
    let state = driver.app().runtime().window(id).unwrap();
    let ticket = state.tree().pending_exit(old_panel).expect("panel is still fading");
    assert!(state.tree().node(old.node().unwrap()).is_some());
    assert_eq!(observe(&mut driver, id).focus, Some(opener));
    let stale = act(&mut driver, id, old, AgentAction::Accessibility { action: A11yAction::Focus });
    assert_eq!(stale.code.as_deref(), Some("STALE_TARGET"));
    assert_eq!(stale.delivery, Delivery::Refused);

    driver.app_mut().update(|w| w.set(open, true)).unwrap();
    let replacement = target(&driver, id, "exit-target");
    assert_ne!(old, replacement);
    let stale_completion = driver.app_mut().dispatch(id, Input::Host(HostReply::ExitFinished(ticket))).unwrap();
    assert!(!stale_completion.delivered);
    assert_eq!(observe(&mut driver, id).focus, Some(replacement));
    assert!(driver.app().runtime().window(id).unwrap().tree().node(old_panel).is_none());
}

#[test]
fn cjk_ime_keeps_the_renamed_file_session_across_reorder_and_rejects_retired_target() {
    let mut driver = driver();
    let list = driver.app_mut().build(|cx| cx.list::<u64, String>()).unwrap();
    driver
        .app_mut()
        .update(|w| {
            w.push_row(list, 1, "alpha".into());
            w.push_row(list, 2, "beta".into());
        })
        .unwrap();
    let id = driver
        .app_mut()
        .open_window(options(), move |_| {
            col().test_id("files").child(each(list, move |_, key, name| {
                let key = *key;
                col().h(40).test_id(format!("row-{key}")).child(
                    input()
                        .test_id(format!("rename-{key}"))
                        .value(bind(move |r| r.try_get(name).unwrap_or_default()))
                        .on_input(move |w, value| w.update_row(list, &key, |name| *name = value.into())),
                )
            }))
        })
        .unwrap();

    let rename = target(&driver, id, "rename-1");
    accessible(&mut driver, id, rename, A11yAction::Focus);
    let initial = active_editor(&mut driver, id);
    let composition = EditorEvent::Edit {
        session: initial.session,
        expected_rev: initial.document_rev,
        edit: octara::TextEdit::Mark {
            text: "日ほ".into(),
            selection: Some(std::ops::Range { start: 6, end: 3 }),
            replacement: Some(0..5),
            clauses: Default::default(),
        },
    };
    let receipt = act(&mut driver, id, rename, AgentAction::Editor { event: composition.into() });
    assert_eq!(receipt.handled, Some(true), "{receipt:?}");
    let composed = active_editor(&mut driver, id);
    assert_eq!(composed.marked, Some(0..6));
    driver.app_mut().update(|w| w.move_row(list, &1, 1)).unwrap();
    assert_eq!(target(&driver, id, "rename-1"), rename);
    assert_eq!(active_editor(&mut driver, id), composed);

    let commit = EditorEvent::Edit {
        session: composed.session,
        expected_rev: composed.document_rev,
        edit: octara::TextEdit::Insert { text: "日本語".into(), replacement: None },
    };
    let receipt = act(&mut driver, id, rename, AgentAction::Editor { event: commit.clone().into() });
    assert_eq!(receipt.handled, Some(true), "{receipt:?}");
    let renamed = active_editor(&mut driver, id);
    assert_eq!(renamed.value.as_deref(), Some("日本語"));
    assert!(renamed.marked.is_none());

    let old_revision = act(&mut driver, id, rename, AgentAction::Editor { event: commit.into() });
    assert!(!old_revision.ok);
    assert_eq!(old_revision.code.as_deref(), Some("STALE_TARGET"));
    assert_eq!(old_revision.delivery, Delivery::Refused);

    driver.app_mut().update(|w| w.remove_row(list, &1)).unwrap();
    driver.app_mut().update(|w| w.push_row(list, 1, "new file".into())).unwrap();
    let replacement = target(&driver, id, "rename-1");
    assert_ne!(replacement, rename);
    let stale = act(&mut driver, id, rename, AgentAction::Accessibility { action: A11yAction::Focus });
    assert!(!stale.ok);
    assert_eq!(stale.code.as_deref(), Some("STALE_TARGET"));
    assert_eq!(stale.delivery, Delivery::Refused);
}

#[test]
fn touch_scroll_consumption_flows_through_panel_list_and_panel_remainder() {
    let mut driver = driver();
    let (panel, post) = driver.app_mut().build(|cx| (cx.signal(24_f32), cx.signal(0_f32))).unwrap();
    let options = WindowOptions { touch: true, ..options() };
    let id = driver
        .app_mut()
        .open_window(options, move |_| {
            col().h(124).child(el().style_with(move |r, style| style.h(f64::from(r.get(panel))))).child(
                col()
                    .test_id("list")
                    .style_with(move |r, style| style.h(124.0 - f64::from(r.get(panel))))
                    .overflow_y(Overflow::Scroll)
                    .shrink(0.0)
                    .handle(HandlerKind::TouchScrollPre, move |w, event| {
                        let HandlerEvent::Interaction { payload: InteractionPayload::TouchScroll { available }, .. } = *event else {
                            unreachable!()
                        };
                        let previous = w.get(panel);
                        let used = available.y.clamp(0.0, previous);
                        w.set(panel, previous - used);
                        HandlerOutcome { consumed: used != 0.0, scroll: Vec2::new(0.0, used) }
                    })
                    .handle(HandlerKind::TouchScrollPost, move |w, event| {
                        let HandlerEvent::Interaction { payload: InteractionPayload::TouchScroll { available }, .. } = *event else {
                            unreachable!()
                        };
                        w.update(post, |total| *total += available.y);
                        HandlerOutcome { consumed: true, scroll: available }
                    })
                    .child(el().test_id("content").h(180).shrink(0.0)),
            )
        })
        .unwrap();
    let down_at = Point::new(30.0, 105.0);
    let pointer = PointerId(8);
    let state = driver.app().runtime().window(id).unwrap();
    let hit = state.hit_test(down_at).unwrap();
    let receiver = hit.action.unwrap_or(hit.hit);
    for action in [
        AgentAction::PointerDown {
            id: pointer,
            pointer: PointerKind::Touch,
            pos: down_at,
            button: Button::Primary,
            modifiers: Modifiers::empty(),
        },
        AgentAction::PointerMove { id: pointer, pos: Point::new(30.0, 5.0), modifiers: Modifiers::empty() },
        AgentAction::PointerUp { id: pointer, pos: Point::new(30.0, 5.0), modifiers: Modifiers::empty() },
    ] {
        let receipt = act(&mut driver, id, receiver, action);
        assert!(receipt.ok, "{receipt:?}");
    }
    let (panel, post) = driver.app_mut().update(|w| (w.get(panel), w.get(post))).unwrap();
    let state = driver.app().runtime().window(id).unwrap();
    let scroll = state.tree().scroll(state.tree().test_ids("list")[0]).unwrap().offset.y;
    assert_eq!(panel, 0.0);
    assert_eq!(scroll, 56.0);
    assert_eq!(post, 20.0);
    assert_eq!(24.0 + scroll + post, 100.0);
}

#[test]
fn modal_traps_focus_and_outside_touch_restores_opener() {
    let mut driver = driver();
    let open = driver.app_mut().build(|cx| cx.signal(false)).unwrap();
    let id = driver
        .app_mut()
        .open_window(options(), move |_| {
            col()
                .child(el().w(120).h(30).focusable().test_id("opener").on_press(move |w| w.set(open, true)).overlay(
                    Placement::BELOW,
                    move |r| r.get(open),
                    move |_| {
                        col()
                            .test_id("dialog")
                            .w(120)
                            .h(90)
                            .layer(LayerPolicy::modal(), move |w| w.set(open, false))
                            .child(input().h(30).autofocus().test_id("dialog-editor"))
                            .child(el().h(30).focusable().test_id("dialog-close"))
                    },
                ))
                .child(el().w(120).h(30).focusable().test_id("background"))
        })
        .unwrap();
    let opener = target(&driver, id, "opener");
    let background = target(&driver, id, "background");
    accessible(&mut driver, id, opener, A11yAction::Focus);
    accessible(&mut driver, id, opener, A11yAction::Press);
    let dialog_editor = target(&driver, id, "dialog-editor");
    let dialog_close = target(&driver, id, "dialog-close");
    let snapshot = observe(&mut driver, id);
    assert_eq!(snapshot.focus, Some(dialog_editor));
    assert!(snapshot.modal.is_some());
    let rejected = act(&mut driver, id, background, AgentAction::Accessibility { action: A11yAction::Focus });
    assert!(!rejected.ok);
    assert_eq!(rejected.delivery, Delivery::Refused);
    for focus in [dialog_close, dialog_editor] {
        let tab = AgentAction::Key { event: KeyEvent::new(Key::Tab, Modifiers::empty()) };
        assert!(act(&mut driver, id, snapshot.target, tab).ok);
        assert_eq!(observe(&mut driver, id).focus, Some(focus));
    }
    let escape = AgentAction::Key { event: KeyEvent::new(Key::Escape, Modifiers::empty()) };
    assert!(act(&mut driver, id, snapshot.target, escape).ok);
    assert_eq!(observe(&mut driver, id).focus, Some(opener));

    accessible(&mut driver, id, opener, A11yAction::Press);
    let pos = Point::new(230.0, 150.0);
    let runtime = driver.app().runtime().window(id).unwrap();
    let hit = runtime.hit_test(pos).unwrap();
    driver
        .dispatch(
            id,
            Input::Interaction(InteractionEvent::PointerDown {
                id: PointerId(19),
                kind: PointerKind::Touch,
                target: hit,
                pos,
                button: Button::Primary,
                mods: Modifiers::empty(),
            }),
        )
        .unwrap();
    let snapshot = observe(&mut driver, id);
    assert_eq!(snapshot.focus, Some(opener));
    assert!(snapshot.modal.is_none());
}

#[test]
fn virtual_reveal_and_offscreen_copy_preserve_exact_utf8_and_document_order() {
    let mut driver = driver();
    let list = driver.app_mut().build(|cx| cx.list::<u64, String>()).unwrap();
    driver
        .app_mut()
        .update(|w| {
            for key in 0..80 {
                w.push_row(list, key, format!("文{key:02}"));
            }
        })
        .unwrap();
    let id = driver
        .app_mut()
        .open_window(options(), move |_| {
            col()
                .test_id("document")
                .h(60)
                .overflow_y(Overflow::Scroll)
                .selectable(true)
                .child(virtual_each_with_copy(
                    list,
                    VirtualOptions { estimated_row_height: 20.0, overscan: 0, ..Default::default() },
                    |_, key, item| text(bind(move |r| r.try_get(item).unwrap_or_default())).h(20).test_id(format!("line-{key}")),
                    |_, value| value.clone(),
                ))
                .on_command("reveal-42", |w| w.reveal_row("document", 42))
                .on_command("reveal-40", |w| w.reveal_row("document", 40))
                .on_command("reveal-75", |w| w.reveal_row("document", 75))
        })
        .unwrap();
    let before = observe(&mut driver, id);
    assert!(!before.nodes.iter().any(|node| node.test_id.as_deref() == Some("line-42")));
    for key in [42, 40] {
        let receipt = act(&mut driver, id, before.target, AgentAction::MenuCommand { name: format!("reveal-{key}") });
        assert_eq!(receipt.handled, Some(true), "{receipt:?}");
        assert!(target(&driver, id, &format!("line-{key}")).node().is_some());
    }
    let document = target(&driver, id, "document");
    let line = |driver: &HeadlessDriver, key| target(driver, id, &format!("line-{key}")).node().unwrap();
    let anchor = TextPosition { node: line(&driver, 42), offset: 3 };
    let focus = TextPosition { node: line(&driver, 40), offset: 3 };
    accessible(&mut driver, id, document, A11yAction::SetTextSelection { anchor, focus });
    let reveal = act(&mut driver, id, before.target, AgentAction::MenuCommand { name: "reveal-75".into() });
    assert_eq!(reveal.handled, Some(true), "{reveal:?}");
    let runtime = driver.app().runtime().window(id).unwrap();
    assert!(runtime.tree().test_ids("line-41").is_empty(), "the intermediate row must actually be unmounted");
    assert!(runtime.tree().scroll(document.node().unwrap()).unwrap().offset.y >= 1400.0);
    assert_eq!(line(&driver, 40), focus.node);
    assert_eq!(line(&driver, 42), anchor.node);
    accessible(&mut driver, id, document, A11yAction::CopyTextSelection);
    let clipboard = driver.take_host_commands().into_iter().find_map(|command| match command {
        HostCommand::Service { request: ServiceRequest::WriteClipboard { items }, .. } => Some(items),
        _ => None,
    });
    assert_eq!(clipboard.unwrap(), [ClipboardItem::Text("40\n文41\n文".into())]);
}

fn editor_window(driver: &mut HeadlessDriver, secure: bool) -> WindowId {
    let id = driver
        .app_mut()
        .open_window(options(), move |cx| {
            let value = cx.signal("initial".to_owned());
            col().child(input().secure(secure).value(value).test_id("field").on_input(move |w, text| w.set(value, text.into())))
        })
        .unwrap();
    let field = target(driver, id, "field");
    accessible(driver, id, field, A11yAction::Focus);
    id
}

fn record_input(replay: &mut Replay, driver: &mut HeadlessDriver, id: WindowId, input: Input) {
    replay.record_input(driver.app(), id, &input);
    driver.dispatch(id, input).unwrap();
}

#[test]
fn four_channel_recording_replays_to_identical_receipts_and_pixels() {
    let mut original = driver();
    let id = editor_window(&mut original, false);
    let mut recording = Replay::new([11, 29]);
    record_input(&mut recording, &mut original, id, Input::Interaction(InteractionEvent::WindowActive(true)));
    let editor = active_editor(&mut original, id);
    let event = EditorEvent::Edit {
        session: editor.session,
        expected_rev: editor.document_rev,
        edit: TextEdit::Insert { text: "日本".into(), replacement: Some(0..7) },
    };
    record_input(&mut recording, &mut original, id, Input::Editor(event));
    let viewport = WindowViewport { size: Size::new(170.0, 90.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
    record_input(&mut recording, &mut original, id, Input::Host(HostReply::ViewportChanged(viewport)));
    let request = AgentRequest::new(Some(id), AgentCommand::Capture { source: CaptureSource::Painted, settle_ms: 7 });
    recording.record_request(original.app(), &request);
    let expected = original.execute(request);
    assert!(expected.ok && expected.progress.settled, "{expected:?}");
    let frame = expected.capture.as_ref().unwrap();
    assert_eq!((frame.picture.width(), frame.picture.height()), (170, 90));
    assert_eq!(expected.progress.committed_frame, Some(frame.sample.revision));
    assert!(expected.progress.presented_frame.is_none());

    let serialized = serde_json::to_vec(&recording).unwrap();
    let recording: Replay = serde_json::from_slice(&serialized).unwrap();
    let mut fresh = driver();
    assert_eq!(editor_window(&mut fresh, false), id);
    let steps = fresh.replay(&recording).unwrap();
    assert_eq!(steps.last(), Some(&Step::Agent(Box::new(expected))));
    assert_eq!(observe(&mut original, id), observe(&mut fresh, id));
    assert_eq!(fresh.app().runtime().now().as_millis(), 7);
}

#[test]
fn replay_redacts_password_keys_and_edits_and_private_platform_data() {
    let mut driver = driver();
    let id = editor_window(&mut driver, true);
    let session = active_editor(&mut driver, id);
    assert!(session.secure && session.value.is_none() && session.selection.is_none());
    let mut recording = Replay::new([11, 29]);
    let editor = EditorEvent::Edit {
        session: session.session,
        expected_rev: session.document_rev,
        edit: octara::TextEdit::Insert { text: "secure-recording-token".into(), replacement: None },
    };
    recording.record_input(driver.app(), id, &Input::Editor(editor.clone()));
    recording.record_input(
        driver.app(),
        id,
        &Input::Interaction(InteractionEvent::Key(KeyEvent::new(Key::Char('🦑'), Modifiers::empty()))),
    );
    let items = vec![
        ClipboardItem::Text("private clipboard token".into()),
        ClipboardItem::Files(vec!["/private/clipboard/token".into()]),
        ClipboardItem::Image { media_type: "image/png".into(), bytes: vec![111, 247, 32, 19] },
    ];
    recording.record_input(
        driver.app(),
        id,
        &Input::Host(HostReply::ServiceReply { id: ServiceId(1), outcome: ServiceOutcome::Clipboard(items.clone()) }),
    );
    let command = HostCommand::Service { window: id, id: ServiceId(1), request: ServiceRequest::WriteClipboard { items: items.clone() } };
    let paste = HostReply::Paste(PasteEvent { items });
    recording.record_input(driver.app(), id, &Input::Host(paste.clone()));
    recording.record_input(
        driver.app(),
        id,
        &Input::Host(HostReply::ServiceReply {
            id: ServiceId(2),
            outcome: ServiceOutcome::Clipboard(vec![ClipboardItem::Files(vec!["/private/path/token".into()])]),
        }),
    );
    let window = target(&driver, id, "field");
    recording.record_input(
        driver.app(),
        id,
        &Input::Host(HostReply::FilesDropped {
            target: window,
            files: vec![DroppedFile::Path("/private/drop/token".into()), DroppedFile::Name("private browser filename".into())],
        }),
    );
    recording.record_request(
        driver.app(),
        &AgentRequest::new(Some(id), AgentCommand::Act { target: window, action: AgentAction::Editor { event: editor.into() } }),
    );

    let channels = recording
        .entries()
        .iter()
        .map(|entry| match entry.event {
            Event::Redacted(channel) => channel,
            ref event => panic!("private input entered the recording: {event:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        channels,
        [Channel::Editor, Channel::Interaction, Channel::Host, Channel::Host, Channel::Host, Channel::Host, Channel::Agent]
    );
    let serialized = serde_json::to_string(&recording).unwrap();
    let debug = format!("{paste:?}\n{command:?}").replace(' ', "");
    for token in [
        "secure-recording-token",
        "🦑",
        "private clipboard token",
        "/private/clipboard/token",
        "[111,247,32,19]",
        "/private/path/token",
        "/private/drop/token",
        "private browser filename",
    ] {
        assert!(!serialized.contains(token));
        assert!(!debug.contains(&token.replace(' ', "")));
    }
    let replayed = driver.replay(&recording).unwrap();
    assert_eq!(replayed, channels.into_iter().map(Step::Redacted).collect::<Vec<_>>());
}

#[test]
fn request_receipts_reject_cross_window_guards_and_report_current_progress() {
    let mut driver = driver();
    let one = editor_window(&mut driver, false);
    let two = editor_window(&mut driver, false);
    let victim = target(&driver, one, "field");
    let one_progress = run(&mut driver, one, AgentCommand::Observe).progress;
    let list = driver.execute(AgentRequest::new(None, AgentCommand::ListWindows));
    assert_eq!(list.windows.unwrap().len(), 2);
    let refused = act(&mut driver, two, victim, AgentAction::Accessibility { action: A11yAction::Focus });
    assert!(!refused.ok);
    let json = serde_json::to_value(&refused).unwrap();
    assert_eq!(json["protocol"], "octara.inspect/2");
    assert_eq!(json["delivery"], "refused");
    assert_eq!(json["code"], "STALE_TARGET");
    assert_eq!(run(&mut driver, one, AgentCommand::Observe).progress, one_progress);
    assert!(run(&mut driver, two, AgentCommand::SetPicking { enabled: true }).ok);
    assert!(observe(&mut driver, two).picking);
    assert!(!observe(&mut driver, one).picking);
    driver.app_mut().close_window(one).unwrap();
    let stale = driver.execute(AgentRequest::new(
        None,
        AgentCommand::Act { target: victim, action: AgentAction::Accessibility { action: A11yAction::Focus } },
    ));
    assert_eq!(stale.code.as_deref(), Some("STALE_TARGET"));
}

#[test]
fn per_corner_utilities_compose_and_descendant_z_indices_stay_local() {
    let mut driver = driver();
    let window = driver
        .app_mut()
        .open_window(options(), |_| {
            let bounds = tw!("absolute top-0 left-0 w-20 h-16");
            el().child(el().tw(bounds).z(-5).child(el().tw(bounds).tw(tw!("z-[2147483647] bg-blue-500")).test_id("nested")))
                .child(el().tw(bounds).rounded(6).rounded_b(8).tw(tw!("rounded-tl-[12px] rounded-r-none z-10 bg-red-500")).test_id("top"))
        })
        .unwrap();
    let top = target(&driver, window, "top");
    let runtime = driver.app().runtime().window(window).unwrap();
    let tree = runtime.tree();
    assert_eq!(tree.visual(top.node().unwrap()).radius, [12.0, 0.0, 0.0, 8.0]);
    assert_eq!(runtime.hit_test(Point::new(20.0, 20.0)).map(|target| target.hit), Some(top));
}

#[test]
fn text_reveal_replays_to_identical_reference_pixels() {
    fn open(driver: &mut HeadlessDriver) -> WindowId {
        driver
            .app_mut()
            .open_window(options(), |_| {
                col().child(text("Aé • Octara").text_reveal(TextReveal::new(
                    TextRevealKind::Gradient { trail_bytes: 6, tint: Some(Color::WHITE.into()), glide_bytes_per_sec: 48.0 },
                    "Aé • Octara".len(),
                    false,
                )))
            })
            .unwrap()
    }

    let mut original = driver();
    let id = open(&mut original);
    let mut recording = Replay::new([11, 29]);
    let mut expected = Vec::new();
    for settle_ms in [0, 125, 375] {
        let request = AgentRequest::new(Some(id), AgentCommand::Capture { source: CaptureSource::Painted, settle_ms });
        recording.record_request(original.app(), &request);
        let receipt = original.execute(request);
        assert!(receipt.ok && receipt.progress.settled);
        expected.push(Step::Agent(Box::new(receipt)));
    }
    let picture = |step: &Step| {
        let Step::Agent(receipt) = step else { unreachable!() };
        receipt.capture.as_ref().unwrap().picture.clone()
    };
    assert!(picture(&expected[0]) != picture(&expected[1]));
    assert!(picture(&expected[1]) != picture(&expected[2]));

    let mut fresh = driver();
    assert_eq!(open(&mut fresh), id);
    let wire = serde_json::to_vec(&recording).unwrap();
    let recording = serde_json::from_slice::<Replay>(&wire).unwrap();
    assert_eq!(fresh.replay(&recording).unwrap(), expected);
}

#[test]
fn rich_link_dispatches_hover_press_and_secondary_with_context_copy_formats() {
    let mut driver = driver();
    let (links, context) = driver.app_mut().build(|b| (b.signal(Vec::<LinkEvent>::new()), b.signal(None::<ContextMenuEvent>))).unwrap();
    let id = driver
        .app_mut()
        .open_window(options(), move |_| {
            col()
                .selectable(true)
                .test_id("document")
                .on_link(move |w, event| {
                    w.update(links, |links| links.push(event.clone()));
                    event.kind == LinkEventKind::Press
                })
                .on_context_menu(move |w, event| {
                    w.set(context, Some(event.clone()));
                    true
                })
                .copy_format(CopyFormat::Markdown, |_, _| Some("[日本](app:focus)".into()))
                .copy_format(CopyFormat::Custom("reference".into()), |_, _| Some("ref:日本".into()))
                .child(text(rich().link("日本", "app:focus")).test_id("link-text"))
        })
        .unwrap();
    let doc = target(&driver, id, "document");
    let text = target(&driver, id, "link-text").node().unwrap();
    let runtime = driver.app().runtime().window(id).unwrap();
    let link = runtime.tree().children(text)[0];
    let bounds = runtime.geometry(link).unwrap().rect;
    let pos = Point::new(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0);
    let hit = runtime.hit_test(pos).unwrap();
    assert_eq!(hit.hit.node(), Some(link));

    accessible(
        &mut driver,
        id,
        doc,
        A11yAction::SetTextSelection {
            anchor: TextPosition { node: text, offset: 0 },
            focus: TextPosition { node: text, offset: "日本".len() },
        },
    );
    let pointer = PointerId(20);
    for event in [
        InteractionEvent::PointerMove { id: pointer, pos, mods: Modifiers::empty() },
        InteractionEvent::PointerDown {
            id: pointer,
            kind: PointerKind::Mouse,
            target: hit,
            pos,
            button: Button::Secondary,
            mods: Modifiers::empty(),
        },
        InteractionEvent::PointerUp { id: pointer, target: Some(hit), pos, mods: Modifiers::empty() },
        InteractionEvent::PointerLeave,
    ] {
        driver.dispatch(id, Input::Interaction(event)).unwrap();
    }
    accessible(&mut driver, id, hit.hit, A11yAction::Press);
    let (links, menu) = driver.app_mut().update(|w| (w.get(links), w.get(context).unwrap())).unwrap();
    assert_eq!(
        links.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [LinkEventKind::Hover(true), LinkEventKind::SecondaryPress, LinkEventKind::Hover(false), LinkEventKind::Press,]
    );
    assert!(links.iter().all(|event| &*event.text == "日本" && &*event.href == "app:focus" && event.rect == bounds));
    assert_eq!(menu.target, link);
    assert_eq!(menu.position, Some(pos));
    assert_eq!(
        menu.items,
        [
            ContextMenuItem::Copy { format: CopyFormat::Plain, text: "日本".into() },
            ContextMenuItem::Copy { format: CopyFormat::Markdown, text: "[日本](app:focus)".into() },
            ContextMenuItem::Copy { format: CopyFormat::Custom("reference".into()), text: "ref:日本".into() },
            ContextMenuItem::CopyLink("app:focus".into()),
        ]
    );
}

#[test]
fn rich_links_receive_modifiers_except_when_shift_extends_document_selection() {
    let mut driver = driver();
    let modifiers = driver.app_mut().build(|b| b.signal(Vec::<Modifiers>::new())).unwrap();
    let id = driver
        .app_mut()
        .open_window(options(), move |_| {
            col()
                .selectable(true)
                .test_id("document")
                .on_link(move |write, event| {
                    if event.kind == LinkEventKind::Press {
                        write.update(modifiers, |mods| mods.push(event.modifiers));
                        return true;
                    }
                    false
                })
                .child(text(rich().link("Follow link", "app:focus")).test_id("link-text"))
        })
        .unwrap();
    let doc = target(&driver, id, "document");
    let text = target(&driver, id, "link-text").node().unwrap();
    let runtime = driver.app().runtime().window(id).unwrap();
    let link = runtime.tree().children(text)[0];
    let bounds = runtime.geometry(link).unwrap().rect;
    let pos = Point::new(bounds.x + bounds.w * 0.75, bounds.y + bounds.h * 0.5);
    let hit = runtime.hit_test(pos).unwrap();
    assert_eq!(hit.hit.node(), Some(link));
    let click = |driver: &mut HeadlessDriver, mods| {
        let id_pointer = PointerId(1);
        for event in [
            InteractionEvent::PointerDown { id: id_pointer, kind: PointerKind::Mouse, target: hit, pos, button: Button::Primary, mods },
            InteractionEvent::PointerUp { id: id_pointer, target: Some(hit), pos, mods },
        ] {
            driver.dispatch(id, Input::Interaction(event)).unwrap();
        }
    };
    let expected = [Modifiers::SHIFT, Modifiers::META, Modifiers::CTRL, Modifiers::ALT | Modifiers::META];
    for mods in expected {
        click(&mut driver, mods);
    }

    accessible(
        &mut driver,
        id,
        doc,
        A11yAction::SetTextSelection { anchor: TextPosition { node: text, offset: 0 }, focus: TextPosition { node: text, offset: 2 } },
    );
    click(&mut driver, Modifiers::SHIFT);
    let runtime = driver.app().runtime().window(id).unwrap();
    let selection = runtime.tree().doc_selection(doc.node().unwrap()).unwrap();
    assert_eq!(selection[0], (text, 0));
    assert!(selection[1].1 > 2);
    assert_eq!(driver.app_mut().update(|write| write.get(modifiers)).unwrap(), expected);
}
