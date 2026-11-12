//! Tooltips (`El::tooltip`): the window's one tooltip, for the innermost hovered element that
//! has one. It shows once the mouse has rested there a moment (at once while moving on from one
//! just shown, along a toolbar), and goes when hover leaves, or on a press, a scroll, a key or
//! the window going inactive (then not again until hover leaves the element). Drawn in the
//! window's layer of requests, below the element (above where there is no room); the text is
//! also the element's description for assistive technology.

use super::Window;
use crate::app::InputEvent;
use crate::kernel::NodeId;
use crate::reactive::list::StateList;

/// How long the mouse rests on an element before its tooltip shows (GPUI's 500 ms).
const DELAY_MS: f64 = 500.0;
/// After a tooltip goes, the next one shows at once within this long.
const WARM_MS: f64 = 300.0;

/// The window's tooltip as it draws it: its number, its text and its element.
pub(crate) type Shown = (u64, String, NodeId);

/// Where the window's tooltip is.
pub(super) struct Tip {
    /// The hovered element whose tooltip is due or shown.
    anchor: Option<NodeId>,
    /// When it shows.
    due: Option<f64>,
    shown: bool,
    /// Dismissed: it does not show again until hover leaves the element.
    spent: bool,
    /// When the last one went.
    hidden_at: f64,
    /// Numbers the tooltips shown.
    next: u64,
    /// The text showing.
    text: Option<String>,
}

impl Default for Tip {
    fn default() -> Self {
        Tip { anchor: None, due: None, shown: false, spent: false, hidden_at: f64::NEG_INFINITY, next: 0, text: None }
    }
}

impl Window {
    /// Hover moved: the innermost hovered element with a tooltip is the one whose tooltip is due.
    pub(super) fn tip_hovered(&self) {
        let want = {
            let i = self.w.interaction.borrow();
            let k = self.w.kernel.borrow();
            i.hovered.and_then(|n| k.tooltip_above(n))
        };
        if self.w.interaction.borrow().tip.anchor == want {
            return;
        }
        let was_shown = self.w.interaction.borrow().tip.shown;
        self.hide_tip();
        let now = self.now_ms();
        let warm = {
            let mut i = self.w.interaction.borrow_mut();
            let t = &mut i.tip;
            t.anchor = want;
            t.spent = false;
            let warm = was_shown || now - t.hidden_at < WARM_MS;
            t.due = want.map(|_| if warm { now } else { crate::task::whole_ns(now + DELAY_MS) });
            warm
        };
        if want.is_some() && warm {
            self.show_tip();
        }
    }

    /// When the tooltip is due to show, if one waits.
    pub(super) fn tip_due(&self) -> Option<f64> {
        let i = self.w.interaction.borrow();
        let t = &i.tip;
        (!t.shown && !t.spent).then_some(t.due).flatten()
    }

    /// The clock moved: a tooltip now due shows.
    pub(super) fn tip_time(&self) {
        if self.tip_due().is_some_and(|due| due <= self.now_ms()) {
            self.show_tip();
        }
    }

    /// `event` puts the tooltip away: a press, a scroll, a key, the window going inactive.
    pub(super) fn tip_dismissed_by(&self, event: &InputEvent) {
        let dismisses = match event {
            InputEvent::Pointer(p) => p.phase == super::Phase::Down,
            InputEvent::Wheel { .. } | InputEvent::Key(_) => true,
            InputEvent::Active(on) => !on,
            _ => false,
        };
        let pending = {
            let i = self.w.interaction.borrow();
            i.tip.anchor.is_some() && !i.tip.spent
        };
        if dismisses && pending {
            self.input_frame(|| {
                self.hide_tip();
                let mut i = self.w.interaction.borrow_mut();
                i.tip.spent = true;
                i.tip.due = None;
            });
        }
    }

    /// The text of the tooltip showing, if one is.
    pub fn tooltip(&self) -> Option<String> {
        self.w.interaction.borrow().tip.text.clone()
    }

    fn show_tip(&self) {
        let Some(list) = *self.w.tip.borrow() else { return };
        let shown = {
            let k = self.w.kernel.borrow();
            let i = self.w.interaction.borrow();
            i.tip.anchor.filter(|n| k.is_alive(*n)).and_then(|n| Some((k.tooltip(n)?.to_owned(), n)))
        };
        let Some((text, anchor)) = shown else { return };
        let n = {
            let mut i = self.w.interaction.borrow_mut();
            i.tip.shown = true;
            i.tip.text = Some(text.clone());
            i.tip.next += 1;
            i.tip.next
        };
        list.retain(|_| false);
        list.push((n, text, anchor));
    }

    fn hide_tip(&self) {
        let shown = std::mem::replace(&mut self.w.interaction.borrow_mut().tip.shown, false);
        if shown {
            let now = self.now_ms();
            let mut i = self.w.interaction.borrow_mut();
            i.tip.hidden_at = now;
            i.tip.text = None;
            drop(i);
            if let Some(list) = *self.w.tip.borrow() {
                list.retain(|_| false);
            }
        }
    }
}

/// The tooltip as the window draws it.
pub(crate) fn view(tips: StateList<u64, Shown>) -> crate::view::View {
    use crate::prelude::*;
    // Where the tooltip is mounted (its content is in the overlay layer).
    el().absolute()
        .top(0.0)
        .left(0.0)
        .size(0.0)
        .a11y_hidden()
        .child(each(
            move || tips.source(),
            |_, tip| {
                let (_, text_, anchor) = tip.get_untracked();
                // Placed by the element itself: nothing here takes the pointer from it.
                el().size(0.0)
                    .a11y_hidden()
                    .overlay_by(anchor, Placement::BELOW.gap(4.0), move || {
                        text(text_.clone())
                            .id("tooltip")
                            .role(Role::Tooltip)
                            .max_w(320.0)
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border()
                            .border_color(Token::Border)
                            .bg(Token::Surface)
                            .text_color(Token::Text)
                            .text_xs()
                            .shadow_md()
                            .select_none()
                            .into()
                    })
                    .into()
            },
        ))
        .into()
}
