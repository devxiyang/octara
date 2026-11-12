//! A window's requests ([`crate::ask`]): what it asks a person, presented to its platform window
//! or drawn in it, waiting for the answer that comes as input.

use super::Window;
use crate::ask::{Answer, Ask, AskId};
use crate::platform::Reply;

/// The lists of the requests a window draws: menus, prompts.
pub(crate) type Drawn = (DrawnList, DrawnList);
type DrawnList = crate::reactive::list::StateList<AskId, (AskId, Ask)>;

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
        } else if let Some((menus, prompts)) = *self.w.drawn.borrow() {
            // The platform does not show it: the window draws a menu or a prompt.
            match ask {
                Ask::Menu { .. } => menus.push((id, ask.clone())),
                Ask::Prompt { .. } => prompts.push((id, ask.clone())),
                _ => {}
            }
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
        if !self.w.asks.borrow().iter().any(|p| p.id == id) {
            return false;
        }
        self.input_frame(|| self.resolve_ask(id, answer));
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

// ---- drawn in the window ----

/// The keys of `chord` (`"mod+shift+c"`) as a menu shows them: AppKit's symbols on Apple
/// platforms (`⇧⌘C`), `Ctrl+Shift+C` elsewhere.
pub(crate) fn chord_label(chord: &str, apple: bool) -> String {
    let mut mods: Vec<&str> = Vec::new();
    let mut key = String::new();
    for part in chord.split('+') {
        match (part, apple) {
            ("mod", true) | ("cmd", _) => mods.push("⌘"),
            ("mod", false) | ("ctrl", false) => mods.push("Ctrl"),
            ("ctrl", true) => mods.push("⌃"),
            ("alt", true) => mods.push("⌥"),
            ("alt", false) => mods.push("Alt"),
            ("shift", true) => mods.push("⇧"),
            ("shift", false) => mods.push("Shift"),
            (k, _) => {
                key = match k {
                    "enter" => if apple { "↩" } else { "Enter" }.to_owned(),
                    "escape" => if apple { "⎋" } else { "Esc" }.to_owned(),
                    "backspace" => if apple { "⌫" } else { "Backspace" }.to_owned(),
                    "delete" => if apple { "⌦" } else { "Del" }.to_owned(),
                    "tab" => if apple { "⇥" } else { "Tab" }.to_owned(),
                    "space" => "Space".to_owned(),
                    "up" => "↑".to_owned(),
                    "down" => "↓".to_owned(),
                    "left" => "←".to_owned(),
                    "right" => "→".to_owned(),
                    k => k.to_uppercase(),
                }
            }
        }
    }
    if apple {
        // AppKit's order: ⌃ ⌥ ⇧ ⌘.
        let order = ["⌃", "⌥", "⇧", "⌘"];
        mods.sort_by_key(|m| order.iter().position(|o| o == m));
        format!("{}{key}", mods.concat())
    } else {
        mods.push(&key);
        mods.join("+")
    }
}

impl Window {
    /// Closes request `id` with `answer` from inside a frame (a drawn menu's item pressed).
    pub(crate) fn resolve_ask(&self, id: AskId, answer: Answer) {
        let pending = {
            let mut asks = self.w.asks.borrow_mut();
            let Some(i) = asks.iter().position(|p| p.id == id) else { return };
            asks.remove(i)
        };
        if let Some((menus, prompts)) = *self.w.drawn.borrow() {
            menus.remove(&id);
            prompts.remove(&id);
        }
        let answer = if answer.fits(&pending.ask) { answer } else { Answer::dismissed(&pending.ask) };
        if pending.native {
            self.w.asks_withdrawn.borrow_mut().push(id);
        }
        if let Some(reply) = pending.reply {
            reply(answer);
        }
    }
}

/// The requests a window draws itself (menus and prompts its platform does not show), above
/// everything: each menu at its point (an overlay, placed as the window's room allows); prompts
/// in a layer over the window, there only while one is open (it takes the pointer then).
pub(crate) fn layer() -> crate::view::View {
    use crate::prelude::*;
    let menus = state_list("menus", Vec::new, |e: &(AskId, Ask)| e.0);
    let prompts = state_list("prompts", Vec::new, |e: &(AskId, Ask)| e.0);
    *super::window().drawn.borrow_mut() = Some((menus, prompts));
    let tips = state_list("tooltips", Vec::new, |e: &super::tooltip::Shown| e.0);
    *super::window().tip.borrow_mut() = Some(tips);
    View::Fragment(vec![
        super::tooltip::view(tips),
        // Where menus anchor (their content is in the overlay layer).
        el().absolute()
            .top(0.0)
            .left(0.0)
            .size(0.0)
            .a11y_hidden()
            .child(each(
                move || menus.source(),
                |id, ask| {
                    let id = *id;
                    match ask.get_untracked().1 {
                        Ask::Menu { menu, at } => el()
                            .absolute()
                            .left(at.0)
                            .top(at.1)
                            .size(0.0)
                            .overlay(|| true, Placement::BELOW.gap(0.0), move || menu_view(id, menu.clone(), true))
                            .into(),
                        _ => el().into(),
                    }
                },
            ))
            .into(),
        show(
            move || !prompts.is_empty(),
            move || {
                col()
                    .absolute()
                    .top(0.0)
                    .left(0.0)
                    .size_full()
                    .child(each(
                        move || prompts.source(),
                        |id, ask| {
                            let id = *id;
                            match ask.get_untracked().1 {
                                Ask::Prompt { level, message, detail, answers } => prompt_view(id, level, message, detail, answers),
                                _ => el().into(),
                            }
                        },
                    ))
                    .into()
            },
        ),
    ])
}

/// A menu drawn in the window: its items, a press on one answers it, a press outside or Escape
/// dismisses it (a submenu: only it). Arrow keys move between items.
fn menu_view(id: AskId, menu: crate::ask::Menu, top: bool) -> crate::view::View {
    use crate::ask::MenuItem;
    use crate::prelude::*;
    let apple = crate::app::current().app().target_platform().is_apple();
    let items: Vec<View> = menu
        .items
        .into_iter()
        .map(|item| match item {
            MenuItem::Separator => el().h(1.0).my_1().bg(Token::Border).a11y_hidden().into(),
            MenuItem::Action { id: item, label, enabled, checked, shortcut } => {
                let pick = item.clone();
                let row = menu_row(&label, checked, shortcut.map(|s| chord_label(&s, apple)), enabled)
                    .id(format!("menu:{item}"))
                    .role(if checked.is_some() { Role::MenuItemCheckBox } else { Role::MenuItem })
                    .on_press(move || crate::app::current().resolve_ask(id, Answer::Menu(Some(pick.clone()))));
                match checked {
                    Some(c) => row.checked(c),
                    None => row,
                }
                .into()
            }
            MenuItem::Submenu { menu, enabled } => {
                let open = state("open", || false);
                let label = menu.name.clone();
                menu_row(&label, None, Some("›".to_owned()), enabled)
                    .id(format!("menu:{label}"))
                    .role(Role::MenuItem)
                    .expanded(move || open.get())
                    .on_press(move || open.set(true))
                    .overlay(
                        move || open.get(),
                        Placement::RIGHT,
                        move || {
                            let sub = menu.clone();
                            component_keyed("Submenu", sub.name.clone(), move || menu_view_closing(id, sub.clone(), open))
                        },
                    )
                    .into()
            }
        })
        .collect();
    let list = col()
        .role(Role::Menu)
        .label(menu.name)
        .min_w(180.0)
        .p_1()
        .rounded_md()
        .border()
        .border_color(Token::Border)
        .bg(Token::Surface)
        .shadow_lg()
        .text_sm()
        .focus_group()
        .children(items);
    if top { list.modal(true, move || crate::app::current().resolve_ask(id, Answer::Menu(None))).into() } else { list.into() }
}

/// A submenu: dismissing it closes it alone.
fn menu_view_closing(id: AskId, menu: crate::ask::Menu, open: crate::reactive::graph::Signal<bool>) -> crate::view::View {
    use crate::prelude::*;
    match menu_view(id, menu, false) {
        View::El(e) => e.modal(true, move || open.set(false)).into(),
        v => v,
    }
}

fn menu_row(label: &str, checked: Option<bool>, trail: Option<String>, enabled: bool) -> crate::view::El {
    use crate::prelude::*;
    row()
        .focusable()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_sm()
        .hover(|p| p.bg(Token::SurfaceHover))
        .focus_visible(|p| p.bg(Token::SurfaceHover))
        .disabled(!enabled)
        .style_when(State::DISABLED, |p| p.opacity(0.45))
        .child(text(if checked == Some(true) { "✓" } else { "" }).w(14.0).select_none())
        .child(text(label).flex_1().select_none())
        .children(trail.map(|t| text(t).text_color(Token::TextMuted).select_none()))
}

/// A prompt drawn in the window: its message and detail over a scrim, its answers as buttons (the
/// last the default, focused); Escape dismisses it.
fn prompt_view(
    id: AskId,
    level: crate::ask::PromptLevel,
    message: String,
    detail: Option<String>,
    answers: Vec<String>,
) -> crate::view::View {
    use crate::prelude::*;
    let last = answers.len().saturating_sub(1);
    let buttons: Vec<View> = answers
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let b = el()
                .id(format!("prompt:{i}"))
                .role(Role::Button)
                .label(a.as_str())
                .focusable()
                .px_4()
                .py_1p5()
                .rounded_md()
                .border()
                .border_color(Token::Border)
                .focus_visible(|p| p.border_color(Token::Focus))
                .on_press(move || crate::app::current().resolve_ask(id, Answer::Prompt(Some(i))))
                .child(text(a.as_str()).select_none());
            if i == last {
                b.bg(Token::Accent).text_color(Token::AccentText).border_color(Token::Accent).autofocus()
            } else {
                b.bg(Token::Background)
            }
            .into()
        })
        .collect();
    let mark = match level {
        crate::ask::PromptLevel::Info => None,
        crate::ask::PromptLevel::Warning => Some(("!", Token::Accent)),
        crate::ask::PromptLevel::Critical => Some(("!", Token::Danger)),
    };
    col()
        .absolute()
        .top(0.0)
        .left(0.0)
        .size_full()
        .items_center()
        .justify_center()
        .bg(Color::BLACK.multiply_alpha(0.3))
        .child(
            col()
                .role(Role::AlertDialog)
                .label(message.as_str())
                .w(360.0)
                .p_5()
                .gap_3()
                .rounded_lg()
                .bg(Token::Background)
                .border()
                .border_color(Token::Border)
                .shadow_xl()
                .focus_group()
                .modal(false, move || crate::app::current().resolve_ask(id, Answer::Prompt(None)))
                .children(mark.map(|(m, t)| text(m).font_bold().text_color(t).text_xl()))
                .child(text(message.as_str()).font_semibold())
                .children(detail.map(|d| text(d).text_sm().text_color(Token::TextMuted)))
                .child(row().justify_end().gap_2().children(buttons)),
        )
        .into()
}
