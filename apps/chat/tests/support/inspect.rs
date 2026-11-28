use octara::prelude::{A11yAction, Key, KeyEvent, Modifiers, Size, WindowViewport};
use octara::{EditorEvent, EditorSessionId, HostReply, Input, TextEdit};
use octara_inspect::protocol::{NodeSnapshot, TreeSnapshot};
use octara_inspect::{AgentAction, AgentCommand, AgentReceipt, AgentRequest, TargetRef};

use super::session::Session;

#[allow(dead_code)]
impl Session {
    pub fn execute(&self, command: AgentCommand) -> AgentReceipt {
        let receipt = self.driver.borrow_mut().execute(AgentRequest::new(Some(self.window), command));
        assert!(receipt.ok, "{receipt:?}");
        assert!(receipt.diagnostics.is_empty(), "{:?}", receipt.diagnostics);
        receipt
    }

    pub fn snapshot(&self) -> TreeSnapshot {
        self.execute(AgentCommand::Observe).snapshot.unwrap()
    }

    pub fn node(&self, id: &str) -> NodeSnapshot {
        self.snapshot().nodes.into_iter().find(|n| n.test_id.as_deref() == Some(id)).unwrap_or_else(|| panic!("missing node {id}"))
    }

    pub fn exists(&self, id: &str) -> bool {
        self.snapshot().nodes.iter().any(|n| n.test_id.as_deref() == Some(id))
    }

    pub fn act(&self, target: TargetRef, action: AgentAction) -> AgentReceipt {
        self.execute(AgentCommand::Act { target, action })
    }

    pub fn press(&self, id: &str) -> bool {
        let node = self.node(id);
        let action = AgentAction::Accessibility { action: A11yAction::Press };
        let receipt =
            self.driver.borrow_mut().execute(AgentRequest::new(Some(self.window), AgentCommand::Act { target: node.target, action }));
        if !node.interactive {
            assert_eq!(receipt.code.as_deref(), Some("INPUT_REJECTED"), "{receipt:?}");
            return false;
        }
        assert!(receipt.ok && receipt.diagnostics.is_empty(), "{receipt:?}");
        receipt.handled == Some(true)
    }

    pub fn command(&self, name: &str) {
        let target = self.snapshot().target;
        self.act(target, AgentAction::MenuCommand { name: name.into() });
    }

    pub fn host(&self, host: HostReply) {
        self.driver.borrow_mut().dispatch(self.window, Input::Host(host)).unwrap();
    }

    pub fn resize(&self, width: f32, height: f32) {
        self.host(HostReply::ViewportChanged(WindowViewport {
            size: Size::new(width, height),
            scale: 1.0,
            safe_area: None,
            keyboard_inset: 0.0,
        }));
    }

    pub fn text(&self, value: &str) {
        self.edit(|session, expected_rev| EditorEvent::Edit {
            session,
            expected_rev,
            edit: TextEdit::Insert { text: value.into(), replacement: None },
        });
    }

    pub fn edit(&self, event: impl FnOnce(EditorSessionId, u64) -> EditorEvent) {
        let snapshot = self.snapshot();
        let editor = snapshot.editor.expect("focus must own an editor");
        self.act(snapshot.focus.unwrap(), AgentAction::Editor { event: event(editor.session, editor.document_rev).into() });
    }

    pub fn key(&self, key: Key, modifiers: Modifiers) {
        let snapshot = self.snapshot();
        let event = KeyEvent::new(key, modifiers);
        if let Some(editor) = snapshot.editor {
            self.act(
                snapshot.focus.unwrap(),
                AgentAction::Editor {
                    event: EditorEvent::KeyIntercept { session: editor.session, expected_rev: editor.document_rev, key: event }.into(),
                },
            );
        } else {
            self.act(snapshot.focus.unwrap_or(snapshot.target), AgentAction::Key { event });
        }
    }
}
