//! A window's requests ([`crate::ask`]): what it asks a person, presented to its platform window
//! or drawn in it, waiting for the answer that comes as input.

use super::Window;
use crate::ask::{Answer, Ask, AskId};
use crate::platform::Reply;

/// A request waiting for its answer.
pub(crate) struct Pending {
    pub(super) id: AskId,
    pub(super) ask: Ask,
    /// The platform shows it.
    pub(super) native: bool,
    /// What gets the answer (the awaiting task's).
    pub(super) reply: Option<Reply<Answer>>,
}

impl Window {
    /// Opens `ask`: the platform shows it if it can (presented with the next frame); the answer
    /// comes as input. The future gives it.
    pub(crate) fn ask(&self, ask: Ask) -> impl Future<Output = Answer> + use<> {
        let (reply, answer) = crate::task::later::<Answer>();
        let id = self.w.ask_next.get();
        self.w.ask_next.set(id + 1);
        let native = self.w.platform.borrow().shows_ask(&ask);
        if native {
            self.w.asks_new.borrow_mut().push((id, ask.clone()));
        }
        self.w.asks.borrow_mut().push(Pending { id, ask, native, reply: Some(reply) });
        answer
    }

    /// The requests waiting for an answer, oldest first.
    pub fn asks(&self) -> Vec<(AskId, Ask)> {
        self.w.asks.borrow().iter().map(|p| (p.id, p.ask.clone())).collect()
    }

    /// Answers request `id` as a person would (tests, agents): the same input a platform sends.
    pub fn answer(&mut self, id: AskId, answer: Answer) -> Result<(), String> {
        let ask = self.w.asks.borrow().iter().find(|p| p.id == id).map(|p| p.ask.clone()).ok_or(format!("no request {id} waits"))?;
        if !answer.fits(&ask) {
            return Err(format!("{answer:?} does not answer {ask:?}"));
        }
        self.event(super::InputEvent::Answer { id, answer });
        Ok(())
    }

    /// The answer to request `id` arrived: what awaits it gets it; a platform still showing it
    /// closes it. An answer that does not fit the request dismisses it.
    pub(super) fn on_answer(&self, id: AskId, answer: Answer) -> bool {
        let pending = {
            let mut asks = self.w.asks.borrow_mut();
            let Some(i) = asks.iter().position(|p| p.id == id) else { return false };
            asks.remove(i)
        };
        let answer = if answer.fits(&pending.ask) { answer } else { Answer::dismissed(&pending.ask) };
        self.input_frame(|| {
            if pending.native {
                self.w.asks_withdrawn.borrow_mut().push(id);
            }
            if let Some(reply) = pending.reply {
                reply(answer);
            }
        });
        true
    }

    /// Where a menu opens when no point is given: at the pointer, else under the focused
    /// element (a context menu from the keyboard), else the window's corner.
    pub(crate) fn menu_point(&self) -> (f32, f32) {
        if let Some(p) = self.w.interaction.borrow().pointer.hover_at {
            return p;
        }
        let focused = self.w.interaction.borrow().focused;
        let k = self.w.kernel.borrow();
        focused.filter(|n| k.is_alive(*n)).map_or((0.0, 0.0), |n| {
            let (x, y, _, h) = k.shown_rect(n);
            (x, y + h)
        })
    }
}
