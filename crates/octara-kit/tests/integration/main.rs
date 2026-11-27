//! Component behavior through native layout and the v2 event cycle.

use octara::{
    EditorEvent, EditorSession, HostCommand, HostReply, Input, InteractionEvent, NodeId, PointerId, PointerKind, Tree, WindowRuntime,
    prelude::*,
};
use octara_inspect::{AgentAction, AgentCommand, AgentRequest, HeadlessDriver};
use octara_kit::*;
use std::time::Duration;

struct Mounted {
    driver: HeadlessDriver,
    id: WindowId,
}

impl Mounted {
    fn new<V: IntoView>(mut driver: HeadlessDriver, view: impl FnOnce(&mut BuildCx<'_>) -> V + 'static) -> Self {
        let id = driver.app_mut().open_window(WindowOptions::default(), view).unwrap();
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

    fn update<R>(&mut self, update: impl FnOnce(&mut WriteCx<'_>) -> R) -> R {
        self.driver.app_mut().update_window(self.id, update).unwrap()
    }

    fn a11y(&mut self, name: &str, action: A11yAction) -> bool {
        let target = self.tree().guard(self.node(name));
        self.driver.dispatch(self.id, Input::Host(HostReply::AccessibilityAction { target, action })).unwrap().handled
    }

    fn key(&mut self, key: Key) -> bool {
        self.driver.dispatch(self.id, Input::Interaction(InteractionEvent::Key(KeyEvent::new(key, Modifiers::empty())))).unwrap().handled
    }

    fn pointer(&mut self, target: &str, action: AgentAction) -> bool {
        let target = self.tree().guard(self.node(target));
        let receipt = self.driver.execute(AgentRequest::new(Some(self.id), AgentCommand::Act { target, action }));
        assert!(receipt.ok, "{receipt:?}");
        receipt.handled.unwrap_or_default()
    }

    fn commit(&mut self, value: &str) {
        let node = self.tree().focus().unwrap();
        let editor = self.tree().editor(node).unwrap();
        let session = EditorSession::active(self.tree()).unwrap().id();
        let event = EditorEvent::Commit { session, expected_rev: editor.document_rev(), text: value.into() };
        assert!(self.driver.dispatch(self.id, Input::Editor(event)).unwrap().handled);
    }

    fn mark(&mut self, value: &str) {
        let node = self.tree().focus().unwrap();
        let editor = self.tree().editor(node).unwrap();
        let session = EditorSession::active(self.tree()).unwrap().id();
        let event = EditorEvent::Composition {
            session,
            expected_rev: editor.document_rev(),
            text: value.into(),
            selection: Some((value.len(), value.len())),
            replacement: None,
        };
        assert!(self.driver.dispatch(self.id, Input::Editor(event)).unwrap().handled);
    }

    fn focus_is(&self, name: &str) {
        assert_eq!(self.tree().focus(), Some(self.node(name)));
    }
}

mod choices;
mod components;
mod controls;
