//! Composable callbacks for retained node handlers.

use octara_model::{
    geometry::Vec2,
    input::{DroppedFile, KeyEvent},
};
use octara_runtime::{HandlerEvent, HandlerOutcome, MountCx, WriteCx};
use octara_tree::{NodeRef, tables::HandlerKind};
use smallvec::{SmallVec, smallvec};

pub(crate) type Handler = Box<dyn Fn(&mut WriteCx<'_>, &HandlerEvent<'_>) -> HandlerOutcome>;

/// A set of callbacks that can be attached to an element and combined with other behaviors.
#[derive(Default)]
pub struct Behavior(SmallVec<[(HandlerKind, Handler); 2]>);

impl Behavior {
    /// Registers a raw retained-tree event callback.
    pub fn handle(mut self, kind: HandlerKind, f: impl Fn(&mut WriteCx<'_>, &HandlerEvent<'_>) -> HandlerOutcome + 'static) -> Self {
        self.0.push((kind, Box::new(f)));
        self
    }

    pub fn on(self, kind: HandlerKind, f: impl Fn(&mut WriteCx<'_>, &HandlerEvent<'_>) + 'static) -> Self {
        self.handle(kind, move |w, event| {
            f(w, event);
            HandlerOutcome::default()
        })
    }

    pub fn on_press(self, f: impl Fn(&mut WriteCx<'_>) + 'static) -> Self {
        self.on(HandlerKind::Press, move |w, _| f(w))
    }

    pub fn on_toggle(self, f: impl Fn(&mut WriteCx<'_>) + 'static) -> Self {
        self.on(HandlerKind::Toggle, move |w, _| f(w))
    }

    /// Observes entry and departure of the pointer in the window's live scope.
    pub fn on_hover_change(self, f: impl Fn(&mut WriteCx<'_>, bool) + 'static) -> Self {
        self.on(HandlerKind::Hover, move |w, event| {
            if let HandlerEvent::Hover(hovered) = event {
                f(w, *hovered);
            }
        })
    }

    /// Registers a handler for a batch of file references dropped on this element.
    pub fn on_file_drop(self, f: impl Fn(&mut WriteCx<'_>, &[DroppedFile]) + 'static) -> Self {
        self.on(HandlerKind::FileDrop, move |w, event| {
            if let HandlerEvent::FileDrop(files) = event {
                f(w, files);
            }
        })
    }

    pub fn on_input(self, f: impl Fn(&mut WriteCx<'_>, &str) + 'static) -> Self {
        self.on(HandlerKind::Input, move |w, event| {
            if let HandlerEvent::EditorInput { value, .. } = event {
                f(w, value);
            }
        })
    }

    pub fn on_submit(self, f: impl Fn(&mut WriteCx<'_>) + 'static) -> Self {
        self.on(HandlerKind::Submit, move |w, _| f(w))
    }

    pub fn on_key(self, f: impl Fn(&mut WriteCx<'_>, &KeyEvent) -> bool + 'static) -> Self {
        self.handle(HandlerKind::Key, move |w, event| {
            let consumed = matches!(event, HandlerEvent::Key(key) if f(w, key));
            HandlerOutcome { consumed, ..Default::default() }
        })
    }

    pub(crate) fn extend(&mut self, behavior: Self) {
        self.0.extend(behavior.0);
    }

    pub(crate) fn mount(self, cx: &mut MountCx<'_>, node: NodeRef) {
        let mut groups = SmallVec::<[(HandlerKind, SmallVec<[Handler; 1]>); 2]>::new();
        for (kind, handler) in self.0 {
            match groups.iter_mut().find(|(old, _)| *old == kind) {
                Some((_, callbacks)) => callbacks.push(handler),
                None => groups.push((kind, smallvec![handler])),
            }
        }
        for (kind, callbacks) in groups {
            cx.handle(node, kind, move |w, event| {
                let mut scroll = Vec2::default();
                for f in &callbacks {
                    let result = f(w, event);
                    scroll.x += result.scroll.x;
                    scroll.y += result.scroll.y;
                    if result.consumed {
                        return HandlerOutcome { consumed: true, scroll };
                    }
                }
                HandlerOutcome { consumed: false, scroll }
            });
        }
    }
}
