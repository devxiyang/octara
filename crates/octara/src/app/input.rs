//! Input by position and by key, as a window delivers it: what a platform shell calls, and what
//! tests and agents can call too ("click at 120, 40").

use super::{Handler, Modality, Window};
use crate::kernel::{HandlerKind, Kind, NodeId, State};
use crate::reactive::graph::Ctx;

impl Window {
    /// Runs `f` as one frame: a new cause id for what it changes, then settle.
    pub(super) fn input_frame(&self, f: impl FnOnce()) {
        self.enter(|| {
            let cause = self.app.next_cause.get() + 1;
            self.app.next_cause.set(cause);
            self.w.kernel.borrow_mut().set_cause(Some(cause));
            f();
            self.settle_frame();
            if self.refresh_hover() {
                self.settle_frame();
            }
            self.w.kernel.borrow_mut().set_cause(None);
        })
    }

    pub(super) fn run(&self, n: NodeId, kinds: &[HandlerKind], input: Option<String>) -> bool {
        let handler = {
            let k = self.w.kernel.borrow();
            if k.state(n).contains(State::DISABLED) {
                return false;
            }
            kinds.iter().find_map(|kind| k.handler(n, *kind)).and_then(|h| self.w.handlers.borrow().get(&h.0).cloned())
        };
        // A link pressed: the nearest `on_link` above it gets its target.
        if handler.is_none() && kinds.contains(&HandlerKind::Press) {
            let link = {
                let k = self.w.kernel.borrow();
                k.link_target(n).and_then(|target| {
                    let at = k.handler_above(k.parent(n), HandlerKind::Link)?;
                    let h = k.handler(at, HandlerKind::Link)?;
                    Some((target, self.w.handlers.borrow().get(&h.0).cloned()?))
                })
            };
            if let Some((target, Handler::Link(f))) = link {
                self.app.rt.with_frame(Ctx::Handler, None, Some(None), || f(&target));
                return true;
            }
        }
        let Some(h) = handler else { return false };
        self.app.rt.with_frame(Ctx::Handler, None, Some(None), || match (h, input) {
            (Handler::Press(f) | Handler::Toggle(f), _) => f(),
            (Handler::Input(f), Some(v)) => f(v),
            _ => {}
        });
        true
    }

    pub(super) fn focused_input(&self) -> Option<(NodeId, String)> {
        let n = self.w.interaction.borrow().focused?;
        let k = self.w.kernel.borrow();
        (k.is_alive(n) && k.kind(n) == Kind::TextInput).then(|| (n, k.value(n).to_owned()))
    }

    /// Text typed (or committed by an input method) into the focused text field. Ends any
    /// composition.
    pub fn type_text(&mut self, text: &str) {
        self.event(super::event::InputEvent::Text(text.to_owned()));
    }
    pub(super) fn on_text(&mut self, text: &str) -> bool {
        if self.focused_input().is_none() {
            return false;
        }
        self.input_frame(|| self.insert_text(text));
        true
    }

    /// The caret of the focused text field `(x, y, w, h)` in logical pixels, for placing an input
    /// method's candidate window.
    pub fn caret(&self) -> Option<(f32, f32, f32, f32)> {
        let (n, _) = self.focused_input()?;
        let r = self.w.kernel.borrow().caret(n)?;
        Some((r.x, r.y, r.w, r.h))
    }

    /// The accessibility tree, for an AccessKit adapter (and for agents): roles, names, values,
    /// states, actions and bounds (logical pixels; the window node carries `scale`).
    pub fn accessibility_tree(&self) -> accesskit::TreeUpdate {
        let scale = f64::from(self.w.scale.get());
        let focus = self.w.interaction.borrow().focused;
        self.w.kernel.borrow().accessibility_tree(focus, scale)
    }

    /// What changed for assistive technology since the last update (the whole tree the first
    /// time, and after [`restart_accessibility`](Self::restart_accessibility)).
    pub fn accessibility_update(&self) -> accesskit::TreeUpdate {
        let scale = f64::from(self.w.scale.get());
        let focus = self.w.interaction.borrow().focused;
        self.w.kernel.borrow().accessibility_update(focus, scale)
    }

    /// The next accessibility update is the whole tree (the platform asked for it again).
    pub fn restart_accessibility(&self) {
        self.w.kernel.borrow().restart_accessibility();
    }

    /// Performs an action assistive technology asked for. Returns whether it did anything.
    pub(super) fn on_accessibility(
        &mut self,
        target: accesskit::NodeId,
        action: accesskit::Action,
        data: Option<accesskit::ActionData>,
    ) -> bool {
        let Some(n) = self.w.kernel.borrow().node_of_a11y(target) else { return false };
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
