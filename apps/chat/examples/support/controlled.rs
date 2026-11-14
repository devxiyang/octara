//! A transport that keeps accepting delivery after cancellation, for lifecycle verification.
//! Each response keeps its own sender, so a test can release an old delta or terminal event
//! after a replacement request has already started.

use octara::prelude::Sender;
use octara_chat::engine::{Cancel, Model, Purpose, Request};
use octara_chat::model::Event;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct Response {
    request: Request,
    send: Sender<Event>,
    cancelled: Rc<Cell<bool>>,
}

#[derive(Clone, Default)]
pub struct Controlled {
    responses: Rc<RefCell<Vec<Response>>>,
}

impl Controlled {
    pub fn requests(&self) -> Vec<Request> {
        self.responses.borrow().iter().map(|response| response.request.clone()).collect()
    }

    /// The response's absolute index; `ordinal` counts only this purpose, from zero.
    pub fn index(&self, purpose: Purpose, ordinal: usize) -> Option<usize> {
        self.responses.borrow().iter().enumerate().filter(|(_, response)| response.request.purpose == purpose).nth(ordinal).map(|(i, _)| i)
    }

    pub fn count(&self, purpose: Purpose) -> usize {
        self.responses.borrow().iter().filter(|response| response.request.purpose == purpose).count()
    }

    /// Cancellation is a transport hint, not proof that already-in-flight events disappeared.
    pub fn emit(&self, index: usize, event: Event) -> bool {
        let send = self.responses.borrow().get(index).map(|response| response.send.clone());
        send.is_some_and(|send| send.send(event).is_ok())
    }

    pub fn cancelled(&self, index: usize) -> bool {
        self.responses.borrow().get(index).is_some_and(|response| response.cancelled.get())
    }
}

impl Model for Controlled {
    fn name(&self) -> &str {
        "Controlled lifecycle"
    }

    fn respond(&self, request: Request, send: Sender<Event>) -> Cancel {
        let cancelled = Rc::new(Cell::new(false));
        self.responses.borrow_mut().push(Response { request, send, cancelled: cancelled.clone() });
        Box::new(move || cancelled.set(true))
    }
}
