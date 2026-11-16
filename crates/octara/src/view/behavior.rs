//! Component event layers, composed once into the existing mounted handlers.

use super::Handler;
use crate::app::{Drag, KeyEvent};
use crate::kernel::HandlerKind;
use crate::reactive::graph::{ScopeId, rt};
use std::rc::Rc;

/// A typed set of component callbacks, configured through [`super::El::behavior`].
/// Setting an event twice replaces it within this layer. Separate layers run in registration
/// order, followed by the element's application callback. All share the element's mounted
/// scope and window. No separate dispatch phase or runtime is created.
#[derive(Default)]
pub struct Behavior {
    handlers: Vec<(HandlerKind, Handler)>,
}

impl Behavior {
    /// A press notification.
    pub fn on_press(mut self, f: impl Fn() + 'static) -> Self {
        self.set(HandlerKind::Press, Handler::Press(Rc::new(f)));
        self
    }
    /// Proposed text; read the model to observe the accepted value.
    pub fn on_input(mut self, f: impl Fn(String) + 'static) -> Self {
        self.set(HandlerKind::Input, Handler::Input(Rc::new(f)));
        self
    }
    /// A finite proposed numeric value, before any component normalization.
    pub fn on_numeric_input(mut self, f: impl Fn(f64) + 'static) -> Self {
        self.set(HandlerKind::NumericInput, Handler::NumericInput(Rc::new(f)));
        self
    }
    /// A toggle request.
    pub fn on_toggle(mut self, f: impl Fn() + 'static) -> Self {
        self.set(HandlerKind::Toggle, Handler::Toggle(Rc::new(f)));
        self
    }
    /// The second press of a double click or tap.
    pub fn on_double_press(mut self, f: impl Fn() + 'static) -> Self {
        self.set(HandlerKind::DoublePress, Handler::Press(Rc::new(f)));
        self
    }
    /// A context menu request.
    pub fn on_secondary_press(mut self, f: impl Fn() + 'static) -> Self {
        self.set(HandlerKind::SecondaryPress, Handler::Press(Rc::new(f)));
        self
    }
    /// A long press.
    pub fn on_long_press(mut self, f: impl Fn() + 'static) -> Self {
        self.set(HandlerKind::LongPress, Handler::Press(Rc::new(f)));
        self
    }
    /// A drag phase, including cancellation.
    pub fn on_drag(mut self, f: impl Fn(&Drag) + 'static) -> Self {
        self.set(HandlerKind::Drag, Handler::Drag(Rc::new(f)));
        self
    }
    /// Submission after IME routing.
    pub fn on_submit(mut self, f: impl Fn() + 'static) -> Self {
        self.set(HandlerKind::Submit, Handler::Press(Rc::new(f)));
        self
    }
    /// Keys before the application callback. Return true to consume.
    pub fn on_key(mut self, f: impl Fn(&KeyEvent) -> bool + 'static) -> Self {
        self.set(HandlerKind::Key, Handler::Key(Rc::new(f)));
        self
    }
    /// A link activation.
    pub fn on_link(mut self, f: impl Fn(&str) + 'static) -> Self {
        self.set(HandlerKind::Link, Handler::Link(Rc::new(f)));
        self
    }
    /// Files dropped on the element.
    pub fn on_drop(mut self, f: impl Fn(&[std::path::PathBuf]) + 'static) -> Self {
        self.set(HandlerKind::Drop, Handler::Drop(Rc::new(f)));
        self
    }
    /// The element gained keyboard focus.
    pub fn on_focus(mut self, f: impl Fn() + 'static) -> Self {
        self.set(HandlerKind::Focus, Handler::Press(Rc::new(f)));
        self
    }
    /// The element lost keyboard focus.
    pub fn on_blur(mut self, f: impl Fn() + 'static) -> Self {
        self.set(HandlerKind::Blur, Handler::Press(Rc::new(f)));
        self
    }

    pub(super) fn set(&mut self, kind: HandlerKind, handler: Handler) {
        if let Some((_, slot)) = self.handlers.iter_mut().find(|(k, _)| *k == kind) {
            *slot = handler;
        } else {
            self.handlers.push((kind, handler));
        }
    }

    pub(super) fn compose(self, layers: Vec<Self>) -> Vec<(HandlerKind, Handler)> {
        if layers.iter().all(|layer| layer.handlers.is_empty()) {
            return self.handlers;
        }
        let mut groups: Vec<(HandlerKind, Vec<Handler>)> = Vec::new();
        for layer in layers.into_iter().chain([self]) {
            for (kind, handler) in layer.handlers {
                if let Some((_, handlers)) = groups.iter_mut().find(|(k, _)| *k == kind) {
                    handlers.push(handler);
                } else {
                    groups.push((kind, vec![handler]));
                }
            }
        }
        let scope = rt(|rt| rt.current_scope());
        groups.into_iter().map(|(kind, handlers)| (kind, compose(handlers, scope))).collect()
    }
}

struct Chain<F: ?Sized> {
    handlers: Vec<Rc<F>>,
    scope: Option<ScopeId>,
}

impl<F: ?Sized> Chain<F> {
    fn alive(&self) -> bool {
        rt(|rt| self.scope.is_none_or(|scope| rt.scope_alive(scope)))
    }

    // A single retirement/consumption rule for every callback signature. No runtime borrow
    // is held across application code. The final check also blocks a key's default action.
    fn run(&self, mut call: impl FnMut(&F, bool) -> bool) -> bool {
        for (index, handler) in self.handlers.iter().enumerate() {
            if !self.alive() || call(handler, index + 1 == self.handlers.len()) {
                return true;
            }
        }
        !self.alive()
    }
}

fn compose(handlers: Vec<Handler>, scope: Option<ScopeId>) -> Handler {
    // HandlerKind fixes the callback signature. Only these typed adapters differ; all event
    // layers use Chain::run for ordering, consumption and lifetime checks.
    macro_rules! chain {
        ($variant:ident, |$chain:ident| $callback:expr) => {{
            let $chain = Chain {
                handlers: handlers
                    .into_iter()
                    .map(|handler| {
                        let Handler::$variant(f) = handler else { unreachable!("event handler signature") };
                        f
                    })
                    .collect(),
                scope,
            };
            Handler::$variant(Rc::new($callback))
        }};
    }
    match &handlers[0] {
        Handler::Press(_) => chain!(Press, |chain| move || {
            chain.run(|f, _| {
                f();
                false
            });
        }),
        Handler::Toggle(_) => chain!(Toggle, |chain| move || {
            chain.run(|f, _| {
                f();
                false
            });
        }),
        Handler::Input(_) => chain!(Input, |chain| move |mut value: String| {
            chain.run(|f, last| {
                f(if last { std::mem::take(&mut value) } else { value.clone() });
                false
            });
        }),
        Handler::NumericInput(_) => chain!(NumericInput, |chain| move |value| {
            chain.run(|f, _| {
                f(value);
                false
            });
        }),
        Handler::Drag(_) => chain!(Drag, |chain| move |event| {
            chain.run(|f, _| {
                f(event);
                false
            });
        }),
        Handler::Key(_) => chain!(Key, |chain| move |event| chain.run(|f, _| f(event))),
        Handler::Link(_) => chain!(Link, |chain| move |url| {
            chain.run(|f, _| {
                f(url);
                false
            });
        }),
        Handler::Drop(_) => chain!(Drop, |chain| move |files| {
            chain.run(|f, _| {
                f(files);
                false
            });
        }),
        Handler::Native(_) => unreachable!("native protocols do not use component event layers"),
    }
}
