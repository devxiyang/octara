//! Input by position and by key, as a window delivers it: what a platform shell calls, and what
//! tests and agents can call too ("click at 120, 40").

use super::{App, Handler, Modality};
use crate::kernel::{HandlerKind, Kind, NodeId, State};
use crate::reactive::graph::Ctx;

impl App {
    /// Runs `f` as one frame: a new cause id for what it changes, then settle.
    pub(super) fn input_frame(&self, f: impl FnOnce()) {
        self.enter(|| {
            let cause = self.next_cause.get() + 1;
            self.next_cause.set(cause);
            self.win().ui.kernel.borrow_mut().set_cause(Some(cause));
            f();
            self.settle_frame();
            if self.refresh_hover() {
                self.settle_frame();
            }
            self.win().ui.kernel.borrow_mut().set_cause(None);
        })
    }

    pub(super) fn run(&self, n: NodeId, kinds: &[HandlerKind], input: Option<String>) -> bool {
        let handler = {
            let k = self.win().ui.kernel.borrow();
            if k.state(n).contains(State::DISABLED) {
                return false;
            }
            kinds.iter().find_map(|kind| k.handler(n, *kind)).and_then(|h| self.win().ui.handlers.borrow().get(&h.0).cloned())
        };
        let Some(h) = handler else { return false };
        self.rt.with_frame(Ctx::Handler, None, Some(None), || match (h, input) {
            (Handler::Press(f) | Handler::Toggle(f), _) => f(),
            (Handler::Input(f), Some(v)) => f(v),
            _ => {}
        });
        true
    }

    pub(super) fn focused_input(&self) -> Option<(NodeId, String)> {
        let n = self.win().interaction.borrow().focused?;
        let k = self.win().ui.kernel.borrow();
        (k.is_alive(n) && k.kind(n) == Kind::TextInput).then(|| (n, k.value(n).to_owned()))
    }

    /// Text typed (or committed by an input method) into the focused text field. Ends any
    /// composition.
    pub fn type_text(&mut self, text: &str) {
        if self.focused_input().is_some() {
            self.input_frame(|| self.insert_text(text));
        }
    }

    /// The caret of the focused text field `(x, y, w, h)` in logical pixels, for placing an input
    /// method's candidate window.
    pub fn caret(&self) -> Option<(f32, f32, f32, f32)> {
        let (n, _) = self.focused_input()?;
        let r = self.win().ui.kernel.borrow().caret(n)?;
        Some((r.x, r.y, r.w, r.h))
    }

    /// The accessibility tree, for an AccessKit adapter (and for agents): roles, names, values,
    /// states, actions and bounds (logical pixels; the window node carries `scale`).
    pub fn accessibility_tree(&self, scale: f64) -> accesskit::TreeUpdate {
        let focus = self.win().interaction.borrow().focused;
        self.win().ui.kernel.borrow().accessibility_tree(focus, scale)
    }

    /// What changed for assistive technology since the last update (the whole tree the first
    /// time, and after [`restart_accessibility`](Self::restart_accessibility)).
    pub fn accessibility_update(&self, scale: f64) -> accesskit::TreeUpdate {
        let focus = self.win().interaction.borrow().focused;
        self.win().ui.kernel.borrow().accessibility_update(focus, scale)
    }

    /// The next accessibility update is the whole tree (the platform asked for it again).
    pub fn restart_accessibility(&self) {
        self.win().ui.kernel.borrow().restart_accessibility();
    }

    /// Performs an action assistive technology asked for. Returns whether it did anything.
    pub fn accessibility_action(
        &mut self,
        target: accesskit::NodeId,
        action: accesskit::Action,
        data: Option<accesskit::ActionData>,
    ) -> bool {
        let Some(n) = self.win().ui.kernel.borrow().node_of_a11y(target) else { return false };
        let mut done = false;
        match action {
            accesskit::Action::Click => self.input_frame(|| done = self.run(n, &[HandlerKind::Press, HandlerKind::Toggle], None)),
            accesskit::Action::Focus => self.input_frame(|| {
                self.set_modality(Modality::Keyboard);
                done = self.set_focus(Some(n)).is_ok()
            }),
            accesskit::Action::SetValue => {
                if let Some(accesskit::ActionData::Value(v)) = data {
                    let v = v.to_string();
                    self.input_frame(|| done = self.set_value_of(n, v));
                }
            }
            accesskit::Action::ShowContextMenu => self.input_frame(|| done = self.run(n, &[HandlerKind::SecondaryPress], None)),
            _ => {}
        }
        done
    }
}
