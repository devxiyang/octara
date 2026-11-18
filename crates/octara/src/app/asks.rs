//! A window's requests ([`crate::ask`]): what it asks a person, presented to its platform window
//! or drawn in it, waiting for the answer that comes as input.

use super::Window;
use crate::ask::{Answer, Ask, AskId};
use crate::platform::Reply;
use crate::reactive::graph::{Ctx, ScopeId, rt};
use crate::reactive::untrack;
use std::rc::{Rc, Weak};

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
    /// The scope that opened the request, retired with it even if its future is retained.
    scope: Option<ScopeId>,
}

/// A request belongs to its awaiting future as well as its construction scope. Weak handles
/// let cancelling a task retire the request without keeping its app or window alive.
struct Request {
    app: Weak<super::AppState>,
    window: Weak<super::WindowState>,
    id: AskId,
}

impl Request {
    fn withdraw(&self) {
        let (Some(app), Some(w)) = (self.app.upgrade(), self.window.upgrade()) else { return };
        let window = Window { app, w };
        window.enter(|| {
            window.app.rt.with_frame(Ctx::Handler, None, Some(None), || {
                let ask = window.w.asks.borrow().iter().find(|pending| pending.id == self.id).map(|pending| pending.ask.clone());
                if let Some(ask) = ask {
                    window.resolve_ask(self.id, Answer::dismissed(&ask));
                }
            });
        });
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        self.withdraw();
    }
}

impl Window {
    /// Opens `ask`: the platform shows it if it can (presented with the next frame); the answer
    /// comes as input. The future gives it.
    #[track_caller]
    pub(crate) fn ask(&self, ask: Ask) -> impl Future<Output = Answer> + use<> {
        let (reply, answer) = crate::task::later::<Answer>();
        let at = std::panic::Location::caller();
        let request = if rt(|rt| rt.allows(crate::reactive::graph::Operation::Register, at)) {
            Some(self.open_ask(ask, reply))
        } else {
            reply(Answer::dismissed(&ask));
            None
        };
        async move {
            let answer = answer.await;
            drop(request);
            answer
        }
    }

    fn open_ask(&self, ask: Ask, reply: Reply<Answer>) -> Rc<Request> {
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
        let scope = rt(|rt| rt.current_scope());
        self.w.asks.borrow_mut().push(Pending { id, ask, native, reply: Some(reply), scope });
        let request = Rc::new(Request { app: Rc::downgrade(&self.app), window: Rc::downgrade(&self.w), id });
        let weak = Rc::downgrade(&request);
        rt(|rt| {
            rt.on_cleanup(Box::new(move || {
                if let Some(request) = weak.upgrade() {
                    request.withdraw();
                }
            }))
        });
        request
    }

    /// The requests waiting for an answer, oldest first.
    pub fn asks(&self) -> Vec<(AskId, Ask)> {
        self.w.asks.borrow().iter().map(|p| (p.id, p.ask.clone())).collect()
    }

    /// Answers request `id` as a person would (tests, agents): the same input a platform sends.
    pub fn answer(&mut self, id: AskId, answer: Answer) -> Result<(), String> {
        self.check_running()?;
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
        if self.w.root_scope.get().is_some_and(|scope| self.app.rt.scope_alive(scope))
            && let Some((menus, prompts)) = *self.w.drawn.borrow()
        {
            menus.remove(&id);
            prompts.remove(&id);
        }
        let live = pending.scope.is_none_or(|scope| self.app.rt.scope_alive(scope));
        let answer = if live && answer.fits(&pending.ask) { answer } else { Answer::dismissed(&pending.ask) };
        if pending.native {
            let mut new = self.w.asks_new.borrow_mut();
            let unpresented = new.iter().any(|(pending, _)| *pending == id);
            new.retain(|(pending, _)| *pending != id);
            if !unpresented {
                self.w.asks_withdrawn.borrow_mut().push(id);
            }
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
    View::fragment(vec![
        super::tooltip::view(tips),
        // Where menus anchor (their content is in the overlay layer).
        el().absolute()
            .top(0.0)
            .left(0.0)
            .size(0.0)
            .a11y_hidden()
            .child(each(
                move || menus.read_only(),
                |id, ask| {
                    let id = *id;
                    match untrack(|| ask.get()).1 {
                        Ask::Menu { menu, at } => el().absolute().left(at.0).top(at.1).size(0.0).overlay(
                            || true,
                            Placement::BELOW.gap(0.0),
                            move || menu_view(id, menu.clone()),
                        ),
                        _ => el(),
                    }
                },
            ))
            .into(),
        show(
            move || !prompts.is_empty(),
            move || {
                col().absolute().top(0.0).left(0.0).size_full().child(each(
                    move || prompts.read_only(),
                    |id, ask| {
                        let id = *id;
                        match untrack(|| ask.get()).1 {
                            Ask::Prompt { level, message, detail, answers } => prompt_view(id, level, message, detail, answers),
                            _ => el(),
                        }
                    },
                ))
            },
        ),
    ])
}

/// A finite menu surface using the window's modal stack. `id` prefixes its semantic IDs:
/// `{id}-menu`, `{id}-action-{command}` and `{id}-submenu-{index}`. Submenu indices belong to
/// this snapshot; action IDs must be unique throughout the snapshot. Reopening may build a new
/// snapshot, but an open surface does not observe changes to `menu`.
///
/// A pick calls `pick` with the action ID. The caller closes its controlled surface there;
/// outside presses and Escape call `dismiss`. Up/Down and Home/End move among enabled rows,
/// Right opens a submenu, and Left or Escape closes only the innermost submenu. Typing finds
/// enabled labels by prefix; repeated letters cycle matches, resetting after 700 ms.
/// This supplies the same drawn rows and keyboard behavior as [`crate::ask::popup_menu`].
#[track_caller]
pub fn menu_surface(id: &str, menu: crate::menu::Menu, pick: impl Fn(String) + 'static, dismiss: impl Fn() + 'static) -> crate::view::El {
    menu_surface_inner(id.to_owned(), menu, std::rc::Rc::new(pick), std::rc::Rc::new(dismiss), false, false)
}

/// A menu request uses the same surface while keeping its established `menu:{action}` IDs.
fn menu_view(id: AskId, menu: crate::ask::Menu) -> crate::view::El {
    menu_surface_inner(
        format!("menu:{id}"),
        menu,
        std::rc::Rc::new(move |pick| crate::app::current().resolve_ask(id, Answer::Menu(Some(pick)))),
        std::rc::Rc::new(move || crate::app::current().resolve_ask(id, Answer::Menu(None))),
        false,
        true,
    )
}

fn menu_surface_inner(
    prefix: String,
    menu: crate::menu::Menu,
    pick: std::rc::Rc<dyn Fn(String)>,
    dismiss: std::rc::Rc<dyn Fn()>,
    submenu: bool,
    legacy: bool,
) -> crate::view::El {
    use crate::ask::MenuItem;
    use crate::prelude::*;
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::rc::Rc;
    use std::time::Duration;
    let apple = crate::app::current().app().target_platform().is_apple();
    let mut enabled_rows = Vec::new();
    let mut names = HashSet::new();
    let items: Vec<View> = menu
        .items
        .into_iter()
        .enumerate()
        .map(|(index, item)| match item {
            MenuItem::Separator => el().h(1.0).my_1().bg(Token::Border).a11y_hidden().into(),
            MenuItem::Action { id: item, label, enabled, checked, shortcut } => {
                let row_id = if legacy { format!("menu:{item}") } else { format!("{prefix}-action-{item}") };
                if enabled {
                    enabled_rows.push((row_id.clone(), label.to_lowercase()));
                }
                let (pick, selected, reveal) = (pick.clone(), item.clone(), row_id.clone());
                let row = menu_row(&label, checked, shortcut.map(|s| chord_label(&s, apple)), enabled)
                    .id(row_id)
                    .role(if checked.is_some() { Role::MenuItemCheckBox } else { Role::MenuItem })
                    .on_focus(move || scroll_into_view(&reveal))
                    .on_press(move || pick(selected.clone()));
                match checked {
                    Some(c) => row.checked(c),
                    None => row,
                }
                .into()
            }
            MenuItem::Submenu { menu, enabled } => {
                let label = menu.name.clone();
                let row_id = if legacy && !submenu && names.insert(label.clone()) {
                    format!("menu:{label}")
                } else {
                    format!("{prefix}-submenu-{index}")
                };
                if enabled {
                    enabled_rows.push((row_id.clone(), label.to_lowercase()));
                }
                let pick = pick.clone();
                scope(format!("MenuSubmenu[{row_id}]"), move || {
                    let open = state("open", || false);
                    let (child_id, reveal) = (row_id.clone(), row_id.clone());
                    menu_row(&label, None, Some("›".to_owned()), enabled)
                        .id(row_id.clone())
                        .role(Role::MenuItem)
                        .expanded(move || open.get())
                        .on_focus(move || scroll_into_view(&reveal))
                        .on_press(move || open.set(true))
                        .on_key(move |key| {
                            if key.key == Key::Right && key.modifiers.is_empty() {
                                open.set(true);
                                true
                            } else {
                                false
                            }
                        })
                        .overlay(
                            move || open.get(),
                            Placement::RIGHT,
                            move || {
                                menu_surface_inner(
                                    child_id.clone(),
                                    menu.clone(),
                                    pick.clone(),
                                    Rc::new(move || open.set(false)),
                                    true,
                                    legacy,
                                )
                            },
                        )
                })
            }
        })
        .collect();
    let typed = RefCell::new((Duration::ZERO, String::new()));
    let outside = dismiss.clone();
    let empty = enabled_rows.is_empty();
    col()
        .id(format!("{prefix}-menu"))
        .role(Role::Menu)
        .label(menu.name)
        .min_w(180.0)
        .style_with(|p| {
            let (width, height) = viewport().get();
            p.max_w((width - 16.0).max(0.0)).max_h((height - 16.0).max(0.0))
        })
        .overflow_y_scroll()
        .p_1()
        .rounded_md()
        .border()
        .border_color(Token::Border)
        .bg(Token::Surface)
        .shadow_lg()
        .text_sm()
        .text_color(Token::Text)
        .focus_group()
        .when(empty, |menu| menu.focusable().tab_index(-1).autofocus().min_h(28.0).when_touch(|p| p.min_h(44.0)))
        .modal(true, move || outside())
        .on_key(move |key| {
            if key.key == Key::Left && key.modifiers.is_empty() && submenu {
                dismiss();
                return true;
            }
            let Key::Char(character) = key.key else {
                typed.borrow_mut().1.clear();
                return false;
            };
            if !(key.modifiers - Modifiers::SHIFT).is_empty() || character.is_control() {
                return false;
            }
            let now = crate::task::now();
            let mut typed = typed.borrow_mut();
            if now.saturating_sub(typed.0) > Duration::from_millis(700) {
                typed.1.clear();
            }
            typed.0 = now;
            let letter = character.to_lowercase().collect::<String>();
            let repeated = typed.1 == letter;
            if !repeated {
                typed.1.push_str(&letter);
            }
            let window = crate::app::window();
            let current = window.interaction.borrow().focused.and_then(|node| window.kernel.borrow().test_id(node).map(str::to_owned));
            let at = enabled_rows.iter().position(|(id, _)| Some(id) == current.as_ref());
            let find = |query: &str, include_current: bool| {
                let len = enabled_rows.len();
                let start = match at {
                    Some(at) if include_current => at,
                    Some(at) => at + 1,
                    None => 0,
                };
                (0..len).map(|offset| &enabled_rows[(start + offset) % len]).find(|(_, label)| label.starts_with(query)).map(|(id, _)| id)
            };
            let mut target = find(&typed.1, !repeated && typed.1 != letter);
            if target.is_none() {
                typed.1 = letter;
                target = find(&typed.1, false);
            }
            if let Some(id) = target {
                focus(id);
                scroll_into_view(id);
            }
            true
        })
        .children(items)
}

fn menu_row(label: &str, checked: Option<bool>, trail: Option<String>, enabled: bool) -> crate::view::El {
    use crate::prelude::*;
    row()
        .label(label.to_owned())
        .focusable()
        .min_h(28.0)
        .when_touch(|p| p.min_h(44.0))
        .min_w(0.0)
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_sm()
        .hover(|p| p.bg(Token::SurfaceHover))
        .focus_visible(|p| p.bg(Token::SurfaceHover))
        .disabled(!enabled)
        .style_when(State::DISABLED, |p| p.opacity(0.45))
        .child(text(if checked == Some(true) { "✓" } else { "" }).w(14.0).select_none().a11y_hidden())
        .child(text(label).flex_1().min_w(0.0).select_none().a11y_hidden())
        .children(trail.map(|t| text(t).text_color(Token::TextMuted).select_none().a11y_hidden()))
}

/// A prompt drawn in the window: its message and detail over a scrim, its answers as buttons (the
/// last the default, focused); Escape dismisses it.
fn prompt_view(
    id: AskId,
    level: crate::ask::PromptLevel,
    message: String,
    detail: Option<String>,
    answers: Vec<String>,
) -> crate::view::El {
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
    col().absolute().top(0.0).left(0.0).size_full().items_center().justify_center().bg(Color::BLACK.multiply_alpha(0.3)).child(
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
}
