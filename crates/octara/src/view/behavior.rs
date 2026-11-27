//! Composable callbacks for retained node handlers.

use octara_model::{
    geometry::Vec2,
    input::{DroppedFile, KeyEvent, PasteEvent},
};
use octara_runtime::{HandlerEvent, HandlerOutcome, MountCx, WriteCx};
use octara_tree::{NodeRef, tables::HandlerKind};
use smallvec::{SmallVec, smallvec};

type RawHandler = Box<dyn Fn(&mut WriteCx<'_>, &HandlerEvent<'_>) -> HandlerOutcome>;
type EventHandler = Box<dyn Fn(&mut WriteCx<'_>, &HandlerEvent<'_>)>;
type WriteHandler<T> = Box<dyn Fn(&mut WriteCx<'_>, T)>;
type BorrowedHandler<T> = Box<dyn Fn(&mut WriteCx<'_>, &T)>;
type ConsumingHandler<T> = Box<dyn Fn(&mut WriteCx<'_>, &T) -> bool>;

// Keep event matching with the erased callback, so each authored closure needs only one box.
pub(super) enum Handler {
    Raw(HandlerKind, RawHandler),
    Event(HandlerKind, EventHandler),
    Action(HandlerKind, Box<dyn Fn(&mut WriteCx<'_>)>),
    Hover(WriteHandler<bool>),
    FileDrop(BorrowedHandler<[DroppedFile]>),
    Input(BorrowedHandler<str>),
    Key(ConsumingHandler<KeyEvent>),
    Paste(ConsumingHandler<PasteEvent>),
}

impl Handler {
    fn kind(&self) -> HandlerKind {
        match self {
            Self::Raw(kind, _) | Self::Event(kind, _) | Self::Action(kind, _) => *kind,
            Self::Hover(_) => HandlerKind::Hover,
            Self::FileDrop(_) => HandlerKind::FileDrop,
            Self::Input(_) => HandlerKind::Input,
            Self::Key(_) => HandlerKind::Key,
            Self::Paste(_) => HandlerKind::Paste,
        }
    }

    fn call(&self, w: &mut WriteCx<'_>, event: &HandlerEvent<'_>) -> HandlerOutcome {
        match (self, event) {
            (Self::Raw(_, callback), _) => return callback(w, event),
            (Self::Event(_, callback), _) => callback(w, event),
            (Self::Action(_, callback), _) => callback(w),
            (Self::Hover(callback), HandlerEvent::Hover(hovered)) => callback(w, *hovered),
            (Self::FileDrop(callback), HandlerEvent::FileDrop(files)) => callback(w, files),
            (Self::Input(callback), HandlerEvent::EditorInput { value, .. }) => callback(w, value),
            (Self::Key(callback), HandlerEvent::Key(key)) if callback(w, key) => {
                return HandlerOutcome { consumed: true, ..Default::default() };
            }
            (Self::Paste(callback), HandlerEvent::Paste(paste)) if callback(w, paste) => {
                return HandlerOutcome { consumed: true, ..Default::default() };
            }
            _ => {}
        }
        HandlerOutcome::default()
    }
}

/// A set of callbacks that can be attached to an element and combined with other behaviors.
#[derive(Default)]
pub struct Behavior(SmallVec<[Handler; 2]>);

impl Behavior {
    /// Registers a raw retained-tree event callback.
    pub fn handle(self, kind: HandlerKind, f: impl Fn(&mut WriteCx<'_>, &HandlerEvent<'_>) -> HandlerOutcome + 'static) -> Self {
        self.with_handler(Handler::Raw(kind, Box::new(f)))
    }

    pub fn on(self, kind: HandlerKind, f: impl Fn(&mut WriteCx<'_>, &HandlerEvent<'_>) + 'static) -> Self {
        self.with_handler(Handler::Event(kind, Box::new(f)))
    }

    pub fn on_press(self, f: impl Fn(&mut WriteCx<'_>) + 'static) -> Self {
        self.with_handler(Handler::Action(HandlerKind::Press, Box::new(f)))
    }

    pub fn on_toggle(self, f: impl Fn(&mut WriteCx<'_>) + 'static) -> Self {
        self.with_handler(Handler::Action(HandlerKind::Toggle, Box::new(f)))
    }

    /// Observes entry and departure of the pointer in the window's live scope.
    pub fn on_hover_change(self, f: impl Fn(&mut WriteCx<'_>, bool) + 'static) -> Self {
        self.with_handler(Handler::Hover(Box::new(f)))
    }

    /// Registers a handler for a batch of file references dropped on this element.
    pub fn on_file_drop(self, f: impl Fn(&mut WriteCx<'_>, &[DroppedFile]) + 'static) -> Self {
        self.with_handler(Handler::FileDrop(Box::new(f)))
    }

    pub fn on_input(self, f: impl Fn(&mut WriteCx<'_>, &str) + 'static) -> Self {
        self.with_handler(Handler::Input(Box::new(f)))
    }

    pub fn on_submit(self, f: impl Fn(&mut WriteCx<'_>) + 'static) -> Self {
        self.with_handler(Handler::Action(HandlerKind::Submit, Box::new(f)))
    }

    pub fn on_key(self, f: impl Fn(&mut WriteCx<'_>, &KeyEvent) -> bool + 'static) -> Self {
        self.with_handler(Handler::Key(Box::new(f)))
    }

    /// Returns `true` to consume the paste before ancestors or the focused editor receive it.
    pub fn on_paste(self, f: impl Fn(&mut WriteCx<'_>, &PasteEvent) -> bool + 'static) -> Self {
        self.with_handler(Handler::Paste(Box::new(f)))
    }

    #[inline(never)]
    fn with_handler(mut self, handler: Handler) -> Self {
        self.push(handler);
        self
    }

    #[inline(never)]
    pub(super) fn push(&mut self, handler: Handler) {
        self.0.push(handler);
    }

    pub(crate) fn extend(&mut self, behavior: Self) {
        self.0.extend(behavior.0);
    }

    pub(crate) fn mount(self, cx: &mut MountCx<'_>, node: NodeRef) {
        let mut groups = SmallVec::<[(HandlerKind, SmallVec<[Handler; 1]>); 2]>::new();
        for handler in self.0 {
            let kind = handler.kind();
            match groups.iter_mut().find(|(old, _)| *old == kind) {
                Some((_, callbacks)) => callbacks.push(handler),
                None => groups.push((kind, smallvec![handler])),
            }
        }
        for (kind, callbacks) in groups {
            cx.handle(node, kind, move |w, event| {
                let mut scroll = Vec2::default();
                for handler in &callbacks {
                    let result = handler.call(w, event);
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
