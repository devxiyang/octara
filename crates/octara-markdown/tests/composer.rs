#![cfg(feature = "composer")]

use octara::{EditorEvent, EditorSession, Input, InteractionEvent, NodeId, PointerId, PointerKind, TextEdit, WindowRuntime, prelude::*};
use octara_inspect::HeadlessDriver;
use octara_markdown::{Completion, MarkdownComposer, TriggerConfig, TriggerQuery};
use std::{ops::Range, sync::Arc};

type RowBuilder = fn(&mut BuildCx<'_>, RowSignal<Completion>, ReadSignal<bool>) -> El;

struct Mounted {
    driver: HeadlessDriver,
    window: WindowId,
    model: Signal<String>,
    composer: MarkdownComposer,
    action: Signal<Option<String>>,
    scheme: Option<Signal<ColorScheme>>,
}

impl Mounted {
    fn new(initial: &str, triggers: Option<Vec<TriggerConfig>>) -> Self {
        Self::with_input(initial, triggers, |input, _| input)
    }

    fn with_input(
        initial: &str,
        triggers: Option<Vec<TriggerConfig>>,
        configure: impl FnOnce(TextInput, Signal<String>) -> TextInput + 'static,
    ) -> Self {
        Self::with_theme(initial, triggers, configure, None)
    }

    fn with_theme(
        initial: &str,
        triggers: Option<Vec<TriggerConfig>>,
        configure: impl FnOnce(TextInput, Signal<String>) -> TextInput + 'static,
        theme: Option<ColorScheme>,
    ) -> Self {
        Self::with_row(initial, triggers, configure, theme, None)
    }

    fn with_row(
        initial: &str,
        triggers: Option<Vec<TriggerConfig>>,
        configure: impl FnOnce(TextInput, Signal<String>) -> TextInput + 'static,
        theme: Option<ColorScheme>,
        render_row: Option<RowBuilder>,
    ) -> Self {
        let mut driver = HeadlessDriver::default();
        let (model, composer, action, scheme) = driver
            .app_mut()
            .build(|build| {
                let model = build.signal(initial.to_owned());
                let composer = match triggers {
                    Some(triggers) => MarkdownComposer::bind_with_triggers(build, model, triggers),
                    None => MarkdownComposer::bind(build, model),
                };
                (model, composer, build.signal(None), theme.map(|scheme| build.signal(scheme)))
            })
            .unwrap();
        let window = driver
            .app_mut()
            .open_window(WindowOptions::default(), move |build| {
                let editor = configure(textarea().test_id("draft"), model);
                let on_complete = move |write: &mut WriteCx<'_>, item: &Completion| {
                    if let Some(key) = item.action_key() {
                        write.set(action, Some(key.into()));
                    }
                };
                let composer = match render_row {
                    Some(render_row) => composer.editor_with_completion_row(build, editor, suggestions, on_complete, render_row),
                    None => composer.editor_with_completion(build, editor, suggestions, on_complete),
                };
                let container = col().p(30.0).child(composer);
                if let Some(scheme) = scheme {
                    container.theme_patch(move |read: &mut ReadCx<'_>| {
                        ThemePatch::scope(match read.get(scheme) {
                            ColorScheme::Light => Theme::light(),
                            ColorScheme::Dark => Theme::dark(),
                        })
                    })
                } else {
                    container
                }
            })
            .unwrap();
        driver.app_mut().update_window(window, |write| write.focus("draft")).unwrap();
        Self { driver, window, model, composer, action, scheme }
    }

    fn window(&self) -> &WindowRuntime {
        self.driver.app().runtime().window(self.window).unwrap()
    }

    fn node(&self) -> NodeId {
        self.window().tree().test_ids("draft")[0]
    }

    fn editor(&self) -> &octara::tree::editor::EditorSlot {
        self.window().tree().editor(self.node()).unwrap()
    }

    fn proposal(&mut self, event: impl FnOnce(octara::EditorSessionId, u64) -> EditorEvent) {
        let tree = self.window().tree();
        let session = EditorSession::active(tree).unwrap();
        let event = event(session.id(), session.editor().document_rev());
        assert!(self.driver.dispatch(self.window, Input::Editor(event)).unwrap().handled);
    }

    fn select(&mut self, selection: (usize, usize)) {
        self.proposal(|session, expected_rev| EditorEvent::ProposeSelection { session, expected_rev, selection });
    }

    fn type_text(&mut self, text: &str) {
        self.proposal(|session, expected_rev| EditorEvent::ProposeEdit {
            session,
            expected_rev,
            edit: TextEdit::Insert { text: text.into(), replacement: None },
        });
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        assert!(
            self.driver.dispatch(self.window, Input::Interaction(InteractionEvent::Key(KeyEvent::new(key, modifiers)))).unwrap().handled
        );
    }

    fn popup(&self) -> Option<NodeId> {
        let tree = self.window().tree();
        tree.children(tree.overlay_root()).iter().copied().find(|node| {
            tree.overlay(*node).is_some_and(|overlay| overlay.target == Some((AnchorTarget::EditorCaret(self.node()), Placement::BELOW)))
        })
    }

    fn active(&mut self) -> Option<TriggerQuery> {
        self.driver.app_mut().update_window(self.window, |write| write.get(self.composer.active_trigger())).unwrap()
    }

    fn source(&mut self) -> String {
        self.driver.app_mut().update_window(self.window, |write| write.get(self.composer.markdown())).unwrap()
    }

    fn set_source(&mut self, source: &str) {
        self.driver.app_mut().update_window(self.window, |write| write.set(self.model, source.to_owned())).unwrap();
    }

    fn action(&mut self) -> Option<String> {
        self.driver.app_mut().update_window(self.window, |write| write.get(self.action)).unwrap()
    }
}

fn suggestions(_: &mut ReadCx<'_>, query: &TriggerQuery) -> Vec<Completion> {
    match query.trigger {
        '@' if query.query == "many" => (0..20).map(|n| Completion::new(format!("@Member {n}"), format!("user://member_{n}"))).collect(),
        '@' => vec![
            Completion::new("@Alice", "user://u_1").icon(octara_icons::HASH).description("Design team").category("People"),
            Completion::new("@Émile", "user://u_2"),
        ],
        '#' => vec![Completion::new("#design", "channel://c_4")],
        '/' => vec![Completion::action("/summarize", "summary")],
        _ => vec![],
    }
}

#[test]
fn default_completion_row_displays_icon_description_and_category() {
    let mut mounted = Mounted::new("", None);
    mounted.type_text("@");
    let popup = mounted.popup().unwrap();
    let tree = mounted.window().tree();
    let option = tree
        .children(popup)
        .iter()
        .copied()
        .find(|&node| tree.semantics(node).is_some_and(|semantics| semantics.role == Some(Role::ListBoxOption)))
        .unwrap();
    let semantics = tree.semantics(option).unwrap();
    assert_eq!(semantics.role, Some(Role::ListBoxOption));
    assert_eq!(semantics.label.as_deref(), Some("@Alice"));
    assert_eq!(semantics.description.as_deref(), Some("People. Design team"));
    assert!(tree.subtree(option).any(|node| tree.icon(node).is_some_and(|icon| icon.name == "hash")));
    let titles: Vec<_> = tree.subtree(option).filter_map(|node| tree.text(node).map(|text| text.text.as_ref())).collect();
    assert_eq!(titles, ["@Alice", "Design team", "People"]);
}

#[test]
fn custom_completion_row_follows_keyboard_selection_and_acceptance() {
    let mut mounted = Mounted::with_row(
        "",
        None,
        |input, _| input,
        None,
        Some(|_, item, selected| {
            text(move |read: &mut ReadCx<'_>| {
                let marker = if read.get(selected) { "Selected" } else { "Available" };
                read.try_with(item, |item| format!("{marker} {}", item.label)).unwrap_or_default()
            })
            .test_id("custom-completion")
        }),
    );
    mounted.type_text("@");
    let rows = |mounted: &Mounted| {
        let tree = mounted.window().tree();
        tree.test_ids("custom-completion").iter().map(|&node| tree.text(node).unwrap().text.to_string()).collect::<Vec<_>>()
    };
    assert_eq!(rows(&mounted), ["Selected @Alice", "Available @Émile"]);
    mounted.key(Key::Down, Modifiers::empty());
    assert_eq!(rows(&mounted), ["Available @Alice", "Selected @Émile"]);
    mounted.key(Key::Enter, Modifiers::empty());
    assert_eq!(mounted.source(), "[@Émile](user://u_2)");
    assert!(mounted.popup().is_none());
    assert_eq!(mounted.window().tree().focus(), Some(mounted.node()));
}

#[test]
fn completion_popup_follows_the_composers_live_theme_scope() {
    let mut mounted = Mounted::with_theme("", None, |input, _| input, Some(ColorScheme::Dark));
    mounted.type_text("@");
    let popup = mounted.popup().unwrap();
    let tree = mounted.window().tree();
    assert_eq!(tree.physical_parent(popup), Some(tree.overlay_root()));
    assert_eq!(tree.logical_parent(popup), Some(mounted.node()));
    assert_eq!(tree.theme_at(popup).scheme, ColorScheme::Dark);
    assert_eq!(tree.visual(popup).background, Some(Theme::dark().get(ColorToken::SurfaceElevated)));

    let scheme = mounted.scheme.unwrap();
    mounted.driver.app_mut().update_window(mounted.window, |write| write.set(scheme, ColorScheme::Light)).unwrap();
    let tree = mounted.window().tree();
    assert_eq!(mounted.popup(), Some(popup));
    assert_eq!(tree.theme_at(popup).scheme, ColorScheme::Light);
    assert_eq!(tree.visual(popup).background, Some(Theme::light().get(ColorToken::SurfaceElevated)));
}

#[test]
fn keyboard_completion_anchors_to_the_live_caret_and_undoes_as_an_atom() {
    let mut mounted = Mounted::new("Hello ", None);
    mounted.select((6, 6));
    mounted.type_text("@");
    let popup = mounted.popup().unwrap();
    let left = mounted.window().geometry(popup).unwrap().rect.x;

    mounted.type_text("al");
    let popup = mounted.popup().unwrap();
    let caret = mounted.window().caret_rect(mounted.node()).unwrap();
    let popup_geometry = mounted.window().geometry(popup).unwrap().rect;
    assert_eq!(mounted.active(), Some(TriggerQuery { trigger: '@', query: "al".into(), range: 6..9 }));
    assert!(popup_geometry.x > left);
    assert!((popup_geometry.x - caret.x).abs() < 1.0);
    assert!((popup_geometry.y - (caret.bottom() + Placement::BELOW.gap)).abs() < 1.0);

    mounted.key(Key::Enter, Modifiers::empty());
    assert_eq!(mounted.source(), "Hello [@Alice](user://u_1)");
    assert!(mounted.popup().is_none());
    let atom = &mounted.editor().atoms()[0];
    assert_eq!(&mounted.editor().value()[atom.range.clone()], "[@Alice](user://u_1)");
    assert_eq!(atom.kind.as_ref(), "mention");
    assert_eq!(mounted.editor().attachments().len(), 2);
    assert!(
        mounted
            .editor()
            .decorations()
            .iter()
            .any(|(range, span)| { &mounted.editor().value()[range.clone()] == "@Alice" && span.background.is_some() })
    );
    let end = mounted.editor().value().len();
    assert_eq!(mounted.editor().selection(), (end, end));

    mounted.key(Key::Backspace, Modifiers::empty());
    assert_eq!(mounted.source(), "Hello ");
    assert!(mounted.editor().atoms().is_empty());
    mounted.key(Key::Char('z'), Modifiers::META);
    assert_eq!(mounted.source(), "Hello [@Alice](user://u_1)");
    assert_eq!(mounted.editor().atoms().len(), 1);
    mounted.key(Key::Char('z'), Modifiers::META);
    assert_eq!(mounted.source(), "Hello @al");
    assert_eq!(mounted.active().unwrap().query, "al");
    mounted.key(Key::Char('z'), Modifiers::META | Modifiers::SHIFT);
    assert_eq!(mounted.source(), "Hello [@Alice](user://u_1)");
    assert_eq!(mounted.editor().atoms().len(), 1);
}

#[test]
fn channel_click_and_slash_action_use_the_retained_input_path() {
    let mut mounted = Mounted::new("", None);
    mounted.type_text("#de");
    let popup = mounted.popup().unwrap();
    let tree = mounted.window().tree();
    let option = tree
        .children(popup)
        .iter()
        .copied()
        .find(|&node| tree.semantics(node).is_some_and(|semantics| semantics.role == Some(Role::ListBoxOption)))
        .unwrap();
    let panel = mounted.window().geometry(popup).unwrap().rect;
    let rect = mounted.window().geometry(option).unwrap().rect;
    let pos = Point::new(panel.x + rect.x + rect.w / 2.0, panel.y + rect.y + rect.h / 2.0);
    let target = mounted.window().hit_test(pos).unwrap();
    assert_eq!(target.action.and_then(|guard| guard.node()), Some(option), "hit {target:?} at {pos:?}, option {rect:?}");
    let id = PointerId(1);
    let mods = Modifiers::empty();
    let down = InteractionEvent::PointerDown { id, kind: PointerKind::Mouse, target, pos, button: Button::Primary, mods };
    assert!(mounted.driver.dispatch(mounted.window, Input::Interaction(down)).unwrap().handled);
    let up = InteractionEvent::PointerUp { id, target: mounted.window().hit_test(pos), pos, mods };
    assert!(mounted.driver.dispatch(mounted.window, Input::Interaction(up)).unwrap().handled);
    assert_eq!(mounted.source(), "[#design](channel://c_4)");
    assert_eq!(mounted.window().tree().focus(), Some(mounted.node()));

    mounted.type_text(" /sum");
    mounted.key(Key::Enter, Modifiers::empty());
    assert_eq!(mounted.source(), "[#design](channel://c_4) ");
    assert_eq!(mounted.action().as_deref(), Some("summary"));
}

#[test]
fn normalized_completion_stays_silent() {
    let mut mounted = Mounted::with_input("", None, |input, model| {
        input.on_input(move |write, text| {
            if text.is_empty() {
                write.set(model, "Summary canceled".into());
            }
        })
    });
    mounted.type_text("/sum");
    mounted.key(Key::Enter, Modifiers::empty());
    assert_eq!(mounted.source(), "Summary canceled");
    assert_eq!(mounted.editor().value(), "Summary canceled");
    assert_eq!(mounted.action(), None);
}

#[test]
fn keyboard_navigation_reveals_the_selected_suggestion() {
    let mut mounted = Mounted::new("", None);
    mounted.type_text("@many");
    let popup = mounted.popup().unwrap();
    mounted.key(Key::Up, Modifiers::empty());
    let tree = mounted.window().tree();
    assert!(tree.scroll(popup).unwrap().offset.y > 0.0);
    let panel = tree.semantics(popup).unwrap();
    assert_eq!(panel.label.as_deref(), Some("Suggestions"));
    assert!(panel.relations.labelled_by.is_none());
    let active = tree.semantics(mounted.node()).unwrap().relations.active_descendant.as_deref().unwrap();
    let [option] = tree.named_nodes(active) else { panic!("the active option must be mounted") };
    assert_eq!(tree.semantics(*option).unwrap().label.as_deref(), Some("@Member 19"));
    mounted.key(Key::Enter, Modifiers::empty());
    assert_eq!(mounted.source(), "[@Member 19](user://member_19)");
}

#[test]
fn configured_triggers_follow_selection_focus_dismissal_and_ime() {
    let mut mounted =
        Mounted::new("", Some(vec![TriggerConfig { trigger: '@', min_chars: 2, allow_spaces: true }, TriggerConfig::new('#')]));
    mounted.type_text("@É");
    assert!(mounted.active().is_none());
    mounted.type_text("m de");
    assert_eq!(mounted.active().unwrap().query, "Ém de");
    mounted.key(Key::Escape, Modifiers::empty());
    assert!(mounted.active().is_none());
    assert!(mounted.popup().is_none());
    mounted.select((3, 3));
    assert!(mounted.active().is_none());
    mounted.select((4, 4));
    assert_eq!(mounted.active().unwrap().query, "Ém");
    mounted.select((1, 4));
    assert!(mounted.active().is_none());
    mounted.select((4, 4));
    mounted.proposal(|session, expected_rev| EditorEvent::Composition {
        session,
        expected_rev,
        text: "字".into(),
        selection: Some((3, 3)),
        replacement: None,
    });
    assert!(mounted.active().is_none());
    mounted.proposal(|session, expected_rev| EditorEvent::CancelComposition { session, expected_rev });
    assert_eq!(mounted.active().unwrap().query, "Ém");
    mounted.driver.app_mut().update_window(mounted.window, |write| write.blur()).unwrap();
    assert!(mounted.active().is_none());
}

#[test]
fn markdown_roundtrips_verbatim_and_decorations_update_with_the_binding() {
    let markdown = "**bold** *italic* `code` <https://octara.dev> https://example.org\r\n\r\n[@Alice](user://u_1) [#design](channel://c_4)";
    let mut mounted = Mounted::new(markdown, None);
    assert_eq!(mounted.source(), markdown);
    assert_eq!(mounted.editor().value(), markdown);
    assert_eq!(mounted.editor().atoms().iter().map(|atom| atom.kind.as_ref()).collect::<Vec<_>>(), ["mention", "channel"]);
    let find = |predicate: fn(RichSpan) -> bool| -> Vec<&str> {
        mounted
            .editor()
            .decorations()
            .iter()
            .filter_map(|(range, span)| predicate(*span).then_some(&mounted.editor().value()[range.clone()]))
            .collect()
    };
    assert_eq!(find(|span| span.weight == Some(700)), ["**bold**"]);
    assert_eq!(find(|span| span.italic), ["*italic*"]);
    assert_eq!(find(|span| span.mono), ["`code`"]);
    assert_eq!(find(|span| span.underline), ["<https://octara.dev>", "https://example.org"]);

    mounted.set_source("`@al`");
    mounted.select((4, 4));
    assert!(mounted.active().is_none());
    assert!(mounted.editor().atoms().is_empty());
    mounted.select((4, 5));
    mounted.type_text("");
    assert_eq!(mounted.source(), "`@al");
    assert!(mounted.editor().decorations().is_empty());
}

struct CustomBackend;

impl EditorBackend for CustomBackend {
    fn rich_text(&self, value: &str) -> Option<EditorRichText> {
        (value == "**custom**").then(|| EditorRichText {
            decorations: Arc::from([(Range { start: 0, end: value.len() }, RichSpan { strike: true, ..Default::default() })]),
            ..Default::default()
        })
    }
}

#[test]
fn scoped_backend_overrides_and_can_fall_back_to_the_composer() {
    let mut driver = HeadlessDriver::default();
    let draft = driver.app_mut().build(|build| build.signal("**custom**".to_owned())).unwrap();
    let id = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |build| {
            build.provide::<Arc<dyn EditorBackend>>(Arc::new(CustomBackend));
            let composer = MarkdownComposer::bind(build, draft);
            col().child(composer.editor(build, textarea().test_id("draft"), |_, _| Vec::new()))
        })
        .unwrap();
    let decoration = |driver: &HeadlessDriver| {
        let tree = driver.app().runtime().window(id).unwrap().tree();
        tree.editor(tree.test_ids("draft")[0]).unwrap().decorations()[0].1
    };
    assert!(decoration(&driver).strike);
    driver.app_mut().update_window(id, |write| write.set(draft, "**ordinary**".into())).unwrap();
    assert_eq!(decoration(&driver).weight, Some(700));
}
