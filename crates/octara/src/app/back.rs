//! A platform back gesture owns one mounted command through its terminal phase.

use super::{GuardTarget, Handler, TargetGuard, Window};
use crate::inspect::BackGestureState;
use crate::kernel::HandlerId;
use serde::{Deserialize, Serialize};

const BACK_COMMAND: &str = "navigation.back";

/// Phases of a platform-owned interactive return.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackPhase {
    Started,
    /// The reported fraction of the transition, from zero to one.
    Progress(f32),
    Cancelled,
    Completed,
}

struct BackCommand {
    target: TargetGuard,
    handler: HandlerId,
}

#[derive(Default)]
pub(super) struct BackInteraction {
    active: Option<BackCommand>,
    pub(super) last: Option<BackGestureState>,
}

impl Window {
    fn back_command(&self) -> Option<BackCommand> {
        self.check_running().ok()?;
        let modal = self.top_modal();
        let path = self.focus_path();
        let k = self.w.kernel.borrow();
        path.into_iter().find_map(|node| {
            k.check_activation(node, modal).ok()?;
            let handler = k.command_handler(node, BACK_COMMAND)?;
            self.live_handler(handler)?;
            Some(BackCommand { target: TargetGuard::new(self, GuardTarget::Node(node)), handler })
        })
    }

    /// Identifies the command a native back transition can start from this frame.
    #[doc(hidden)]
    pub fn back_gesture_target(&self) -> Option<HandlerId> {
        self.back_command().map(|command| command.handler)
    }

    pub(super) fn on_back_gesture(&self, id: u64, phase: BackPhase) -> bool {
        if !self.is_active() {
            return false;
        }
        let mut handled = false;
        self.input_frame(|| handled = self.apply_back_phase(id, phase));
        handled
    }

    fn apply_back_phase(&self, id: u64, phase: BackPhase) -> bool {
        let state = BackGestureState { id, phase, delivered: false };
        if phase == BackPhase::Started {
            {
                let interaction = self.w.interaction.borrow();
                let back = &interaction.back;
                if id == 0 || back.active.is_some() || back.last.is_some_and(|previous| previous.id >= id) {
                    return false;
                }
            }
            let Some(command) = self.back_command() else { return false };
            let mut interaction = self.w.interaction.borrow_mut();
            interaction.back.active = Some(command);
            interaction.back.last = Some(state);
            return true;
        }
        let command = {
            let mut interaction = self.w.interaction.borrow_mut();
            let back = &mut interaction.back;
            if back.last.is_none_or(|previous| previous.id != id) || back.active.is_none() {
                return false;
            }
            if let BackPhase::Progress(progress) = phase {
                if !(0.0..=1.0).contains(&progress) {
                    return false;
                }
                back.last = Some(state);
                return true;
            }
            back.last = Some(state);
            // Consume the session before the application callback can reenter the window.
            back.active.take().expect("the matching back gesture is active")
        };
        if phase == BackPhase::Cancelled {
            return true;
        }
        let delivered = self.complete_back(command);
        if let Some(last) = self.w.interaction.borrow_mut().back.last.as_mut()
            && last.id == id
        {
            last.delivered = delivered;
        }
        delivered
    }

    fn complete_back(&self, command: BackCommand) -> bool {
        if !command.target.is_current(self) {
            return false;
        }
        let node = command.target.node().expect("a back command belongs to a view");
        let eligible = {
            let k = self.w.kernel.borrow();
            k.check_activation(node, self.top_modal()).is_ok() && k.command_handler(node, BACK_COMMAND) == Some(command.handler)
        };
        eligible
            && self
                .live_handler(command.handler)
                .and_then(|handler| {
                    handler.run(&self.app.rt, |handler| match handler {
                        Handler::Press(callback) => {
                            callback();
                            true
                        }
                        _ => false,
                    })
                })
                .unwrap_or(false)
    }

    pub(super) fn cancel_back_gesture(&self) {
        let mut interaction = self.w.interaction.borrow_mut();
        if interaction.back.active.take().is_some()
            && let Some(last) = interaction.back.last.as_mut()
        {
            last.phase = BackPhase::Cancelled;
        }
    }
}
