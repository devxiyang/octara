//! The Chat model picker preserves the composer and uses model IDs across real app input.

use octara::prelude::*;
use octara_chat::engine::{Cancel, Model, Purpose, Request};
use octara_chat::model::Event as ModelEvent;
use octara_chat::{ModelEntry, Options, root};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

#[derive(Clone)]
struct Probe {
    name: String,
    title: Option<String>,
    requests: Rc<RefCell<Vec<Request>>>,
}

impl Probe {
    fn streaming(name: &str) -> Self {
        Self { name: name.into(), title: None, requests: Rc::default() }
    }

    fn entry(&self, id: &str) -> ModelEntry {
        ModelEntry::new(id, Rc::new(self.clone()))
    }

    fn replies(&self) -> usize {
        self.requests.borrow().iter().filter(|request| request.purpose == Purpose::Reply).count()
    }
}

impl Model for Probe {
    fn name(&self) -> &str {
        &self.name
    }

    fn respond(&self, request: Request, send: Sender<ModelEvent>) -> Cancel {
        if let Some(title) = &self.title {
            let answer = if request.purpose == Purpose::Title { title.clone() } else { "A short answer.".into() };
            send.send(ModelEvent::Text(answer)).unwrap();
            send.send(ModelEvent::Done).unwrap();
        }
        self.requests.borrow_mut().push(request);
        Box::new(|| {})
    }
}

fn open(models: Vec<ModelEntry>) -> Window {
    let mut window = Window::headless(move || {
        root({
            let models = models.clone();
            move || Options { models: models.clone(), ..Options::default() }
        })
    });
    window.resize(1000.0, 720.0);
    window
}

fn focused(window: &Window, id: &str) -> bool {
    window.kernel().state_of(id).is_some_and(|state| state.contains(State::FOCUSED))
}

fn clean(window: &Window) {
    clean_with_truncation(window, &[]);
}

fn clean_with_truncation(window: &Window, allowed_ids: &[&str]) {
    // Long captions intentionally use ellipses. Their full text remains in the model's
    // value/option name and its tooltip, and in the conversation heading/sidebar names.
    let issues = window.layout_issues();
    assert!(
        issues
            .iter()
            .all(|issue| { issue.code == "TEXT_TRUNCATED" && issue.test_id.as_deref().is_some_and(|id| allowed_ids.contains(&id)) }),
        "{issues:?}"
    );
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

fn choose(window: &mut Window, id: &str) {
    window.tap("model").unwrap();
    window.tap(&format!("model-option-{id}")).unwrap();
}

#[test]
fn chinese_search_selects_same_named_models_by_id_without_consuming_the_draft() {
    let first = Probe::streaming("聊天模型");
    let second = Probe::streaming("聊天模型");
    let other = Probe::streaming("Writer");
    let mut window = open(vec![first.entry("first"), other.entry("writer"), second.entry("second")]);
    window.invoke("chat.focus_composer").unwrap();
    window.type_text("Keep this draft 😀");
    window.press_key("shift+left").unwrap();
    let composer = window.inspect().find("composer").unwrap().target.clone();
    let selection = window.text_field().unwrap().selection;

    window.tap("model").unwrap();
    assert_eq!(window.kernel().role_of("model"), Some(Role::ComboBox));
    assert!(focused(&window, "model-search"));
    window.type_text("聊天");
    assert!(!window.kernel().exists("model-option-writer"));
    for id in ["first", "second"] {
        assert_eq!(window.kernel().text_of(&format!("model-option-{id}")).as_deref(), Some("聊天模型"));
    }
    window.tap("model-option-second").unwrap();
    assert!(!window.kernel().exists("model-popover"));
    assert!(focused(&window, "model"));
    assert_eq!(window.kernel().value_of("model").as_deref(), Some("聊天模型"));
    assert_eq!(window.kernel().value_of("composer").as_deref(), Some("Keep this draft 😀"));
    assert_eq!(window.inspect().find("composer").unwrap().target, composer);
    window.invoke("chat.focus_composer").unwrap();
    assert_eq!(window.text_field().unwrap().selection, selection);
    window.press_key("enter").unwrap();
    assert_eq!(first.replies(), 0);
    assert_eq!(second.replies(), 1, "matching labels still select distinct model identities");

    choose(&mut window, "first");
    assert!(window.kernel().exists("stop"), "changing the next model leaves the active answer's Stop available");
    window.tap("stop").unwrap();
    assert!(!window.kernel().exists("stop"));
    clean(&window);
}

#[test]
fn queue_shows_the_model_captured_when_the_message_was_enqueued() {
    let first = Probe::streaming("Alpha");
    let second = Probe::streaming("Beta");
    let mut window = open(vec![first.entry("alpha"), second.entry("beta")]);
    window.invoke("chat.focus_composer").unwrap();
    window.type_text("First message");
    window.press_key("enter").unwrap();
    choose(&mut window, "beta");
    window.invoke("chat.focus_composer").unwrap();
    window.type_text("Queued message");
    window.press_key("enter").unwrap();
    let queued = window.kernel().child_ids("composer-area").into_iter().find(|id| id.starts_with("queued-")).unwrap();
    let id = queued.strip_prefix("queued-").unwrap();
    let label = format!("queued-model-{id}");
    assert_eq!(window.kernel().text_of(&label).as_deref(), Some("Queued · Beta"));
    choose(&mut window, "alpha");
    assert_eq!(window.kernel().text_of(&label).as_deref(), Some("Queued · Beta"));
    window.tap(&format!("send-now-{id}")).unwrap();
    assert_eq!(first.replies(), 1);
    assert_eq!(second.replies(), 1);
    assert!(!window.kernel().exists(&queued));
    window.tap("stop").unwrap();
    clean(&window);
}

#[test]
fn long_model_and_conversation_names_fit_phone_and_desktop_in_both_themes() {
    let name = "A model with a very long display name 模型名称用于检查窄屏布局 and additional details";
    let title = "A conversation title that stays readable while the model picker shares a narrow header";
    for (width, height) in [(320.0, 640.0), (390.0, 844.0), (1000.0, 720.0)] {
        for theme in [Theme::light(), Theme::dark()] {
            let model = Probe { title: Some(title.into()), ..Probe::streaming(name) };
            let mut window = open(vec![model.entry("long-name"), Probe::streaming("Another model").entry("other")]);
            window.resize(width, height);
            window.app().set_theme(theme);
            window.invoke("chat.focus_composer").unwrap();
            window.type_text("Hello");
            window.press_key("enter").unwrap();
            window.app().run_until_idle(Duration::from_secs(5)).unwrap();
            assert_eq!(window.kernel().text_of("title").as_deref(), Some(title));
            assert_eq!(window.kernel().value_of("model").as_deref(), Some(name));
            {
                let kernel = window.kernel();
                assert!(kernel.tooltip(kernel.find("model-picker").unwrap()).unwrap().contains(name));
            }
            let snapshot = window.inspect();
            let mut closed_truncation = vec!["model", "title"];
            if let Some(threads) = snapshot.find("threads") {
                let titles: Vec<_> = threads
                    .walk()
                    .into_iter()
                    .filter(|node| node.id.as_deref().is_some_and(|id| id.starts_with("thread-title-")))
                    .collect();
                assert_eq!(titles.len(), 1, "this conversation supplies the sidebar's only title");
                let sidebar_title = titles[0];
                assert_eq!(sidebar_title.name.as_deref(), Some(title), "the sidebar keeps the full accessible title");
                closed_truncation.push(sidebar_title.id.as_deref().unwrap());
            }
            let mut popup_truncation = closed_truncation.clone();
            popup_truncation.push("model-option-long-name");
            let [title_x, _, title_width, _] = snapshot.find("title").unwrap().bounds;
            let [picker_x, _, picker_width, _] = snapshot.find("model").unwrap().bounds;
            assert!(title_width > 0.0 && picker_width > 0.0);
            assert!(title_x + title_width <= picker_x, "title and picker remain separate at {width}px");
            assert!(picker_x + picker_width <= width, "picker stays within the window at {width}px");
            assert!(
                window.layout_issues().iter().any(|issue| issue.code == "TEXT_TRUNCATED" && issue.test_id.as_deref() == Some("model")),
                "the long model caption uses the bounded trigger's ellipsis"
            );
            clean_with_truncation(&window, &closed_truncation);
            window.tap("model").unwrap();
            clean_with_truncation(&window, &popup_truncation);
            window.type_text("模型");
            assert!(window.kernel().exists("model-option-long-name"));
            assert!(!window.kernel().exists("model-option-other"));
            assert_eq!(window.inspect().find("model-option-long-name").unwrap().name.as_deref(), Some(name));
            clean_with_truncation(&window, &popup_truncation);
            window.press_key("escape").unwrap();
            clean_with_truncation(&window, &closed_truncation);
        }
    }
}

#[test]
fn no_models_keeps_a_usable_composer_and_preserves_unsent_text() {
    for (width, height) in [(320.0, 640.0), (1000.0, 720.0)] {
        for theme in [Theme::light(), Theme::dark()] {
            let mut window = open(vec![]);
            window.resize(width, height);
            window.app().set_theme(theme);
            assert!(window.kernel().state_of("model").unwrap().contains(State::DISABLED));
            assert!(window.kernel().exists("model-status"));
            window.invoke("chat.focus_composer").unwrap();
            window.type_text("A draft without a configured model");
            assert!(window.kernel().state_of("send").unwrap().contains(State::DISABLED));
            window.press_key("enter").unwrap();
            assert_eq!(window.kernel().value_of("composer").as_deref(), Some("A draft without a configured model"));
            assert_eq!(window.kernel().text_of("title").as_deref(), Some("New chat"));
            assert!(window.kernel().child_ids("transcript").into_iter().all(|id| !id.starts_with("msg-")));
            assert!(!window.kernel().exists("stop"));
            clean(&window);
        }
    }
}
