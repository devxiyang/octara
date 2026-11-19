//! The chat's views, composed from the kit: conversations (`list`) beside the
//! open one; its transcript a selectable scroll container following its tail over a virtual
//! list of messages. Message parts use paced Markdown, `collapsible` reasoning and tool calls,
//! and a `callout` for failures. The queue sits above the composer (`text_area`, `removable_chip`s,
//! `icon_button`s).

use crate::chat::{Chat, Options};
use crate::model::{Author, Message, Part, PartId, Status, Thread, ThreadId, ToolState};
use octara::prelude::*;
use octara::reactive::list::FlatKey;
use octara::reactive::untrack;
use octara::view::paced;
use octara_icons as icons;
use octara_kit::markdown::markdown;
use octara_kit::*;
use std::cmp::Reverse;
use std::time::Duration;

fn compact() -> bool {
    breakpoint().get() == Breakpoint::Compact
}

/// The app: conversations beside the open one.
pub fn root(options: impl Fn() -> Options + 'static) -> View {
    scope("Chat", move || {
        let chat = Chat::new(options());
        view(&chat)
    })
}

/// Composes the product UI from an explicitly supplied model in the current owner.
/// Build once within a window root or an explicit scope; UI state and cleanup stay in that
/// owner. Handlers and deferred content retain cloned model handles, not this borrow.
#[ui]
pub fn view(chat: &Chat) -> View {
    let width = state("sidebar", || 260.0f32);
    // A phone shows the conversations or the open one; wider windows show both.
    let compact = memo(|| breakpoint().get() == Breakpoint::Compact);
    let listing = state("listing", || false);
    let (desktop, phone) = (chat.clone(), chat.clone());
    commands(chat, listing);
    row()
        .id("panes")
        .relative()
        .size_full()
        .child(show(move || !compact.get(), move || desktop_navigation(&desktop, listing, width)))
        .child(el().h_full().flex_1().min_w(0.0).child(main_pane(chat, listing)))
        .child(show(
            move || compact.get() && listing.get(),
            move || {
                el().id("chat-listing")
                    .role(Role::Dialog)
                    .label("Conversations")
                    .absolute()
                    .inset_0()
                    .size_full()
                    .bg(Token::Surface)
                    .modal(false, move || listing.set(false))
                    .child(sidebar(&phone, listing))
            },
        ))
        .into()
}

#[ui]
fn desktop_navigation(chat: &Chat, listing: Signal<bool>, width: Signal<f32>) -> View {
    let divider = splitter("panes-divider", (move || width.get(), move |next: f32| width.set(next.clamp(180.0, 420.0))));
    View::fragment(vec![
        el().h_full().flex_shrink_0().style_with(move |s| s.w(width.get())).child(sidebar(chat, listing)).into(),
        divider.into(),
    ])
}

/// What the app's commands do, and its menu bar.
fn commands(chat: &Chat, listing: Signal<bool>) {
    let c = chat.clone();
    on_command("chat.new", move || {
        c.new_chat();
        listing.set(false);
    });
    let c = chat.clone();
    on_command("chat.stop", move || {
        if let Some(t) = untrack(|| c.current.get()) {
            c.stop(t);
        }
    });
    on_command("chat.focus_composer", move || {
        listing.set(false);
        focus("composer");
    });
    bind_keys([("mod+n", "chat.new", None), ("mod+l", "chat.focus_composer", None), ("mod+.", "chat.stop", None)]).expect("chords");
    set_menus([
        Menu::new("File").item(MenuItem::action("New Chat", "chat.new")),
        Menu::edit(),
        Menu::new("Chat")
            .item(MenuItem::action("Focus Composer", "chat.focus_composer"))
            .item(MenuItem::action("Stop Answering", "chat.stop")),
    ]);
}

// ---- the sidebar ----

/// How long ago a conversation changed, as the sidebar groups it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
enum Age {
    Today,
    Yesterday,
    Week,
    Older,
}

impl std::fmt::Display for Age {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Age::Today => "Today",
            Age::Yesterday => "Yesterday",
            Age::Week => "Previous 7 Days",
            Age::Older => "Older",
        })
    }
}

fn age(updated: u64, now: u64) -> Age {
    const DAY: u64 = 86_400;
    let (today, day) = (now / DAY, updated / DAY);
    match today.saturating_sub(day) {
        0 => Age::Today,
        1 => Age::Yesterday,
        2..=7 => Age::Week,
        _ => Age::Older,
    }
}

fn sidebar(chat: &Chat, listing: Signal<bool>) -> El {
    let new = chat.clone();
    col()
        .id("sidebar")
        .size_full()
        .bg(Token::Surface)
        .child(row().items_center().gap_3().h(72.0).px_5().child(brand_mark(32.0)).child(text("octara").text_xl().font_semibold()))
        .child(
            el().px_3().pb_4().child(
                button("New conversation")
                    .id("new-chat")
                    .label("New chat")
                    .w_full()
                    .justify_start()
                    .gap_2()
                    .h(40.0)
                    .rounded_lg()
                    .bg(Token::Background)
                    .on_press(move || {
                        new.new_chat();
                        listing.set(false);
                    }),
            ),
        )
        .child(text("YOUR CONVERSATIONS").text_xs().text_color(Token::TextMuted).px_5().pb_2())
        .child(thread_list(chat, listing))
        .child(el().h(1.0).w_full().bg(Token::Border).a11y_hidden())
        .child(
            row().items_center().gap_3().p_4().child(icon(icons::HARD_DRIVE).size_4().text_color(Token::TextMuted)).child(
                col()
                    .gap_1()
                    .child(text("Personal workspace").text_sm().font_medium())
                    .child(text("History saved on this device").text_xs().text_color(Token::TextMuted)),
            ),
        )
}

fn thread_list(chat: &Chat, listing: Signal<bool>) -> El {
    const VIEWPORT: &str = "threads";
    let now = chat.now();
    let flat = chat.threads.read_only().sort_by(|t| Reverse(t.updated)).group_by(move |t| age(t.updated, now)).flatten();
    let endpoints = flat.clone();
    let renaming = chat.renaming;
    let rows = chat.clone();
    list()
        .id(VIEWPORT)
        .label("Conversations")
        .flex_1()
        .min_h(0.0)
        .px_2()
        .pb_2()
        .overflow_y_scroll()
        .on_key(move |event| {
            let first = match event.key {
                Key::Home => true,
                Key::End => false,
                _ => return false,
            };
            // The inline rename field uses these keys to move its caret.
            if !event.modifiers.is_empty() || untrack(|| renaming.get()).is_some() {
                return false;
            }
            let mut keys = untrack(|| endpoints.keys()).into_iter();
            let is_thread = |key: &FlatKey<Age, ThreadId>| matches!(key, FlatKey::Item(_));
            let target = if first { keys.find(is_thread) } else { keys.rfind(is_thread) };
            let Some(key @ FlatKey::Item(id)) = target else { return false };
            if !reveal_row(VIEWPORT, key) {
                return false;
            }
            focus(&format!("thread-{id}"));
            true
        })
        .child(virtual_each(
            move || flat.clone(),
            |item| if item.header().is_some() { 36.0 } else { 34.0 },
            move |key, item| match key.clone() {
                FlatKey::Header(age) => list_header(age.to_string()).id(format!("age-{age:?}")),
                FlatKey::Item(id) => thread_row(&rows, listing, id, move || item.with(|f| f.item().cloned())),
            },
        ))
}

fn brand_mark(size: f32) -> El {
    el().size(size)
        .flex_shrink_0()
        .rounded_lg()
        .bg(Token::Accent)
        .items_center()
        .justify_center()
        .child(icon(icons::SPARKLES).size_4().text_color(Token::AccentText))
        .a11y_hidden()
}

fn thread_row(chat: &Chat, listing: Signal<bool>, id: ThreadId, thread: impl Fn() -> Option<Thread> + Clone + 'static) -> El {
    let title = move || thread().map(|t| t.title).unwrap_or_default();
    let renaming = chat.renaming;
    let (open, menu, busy, rename, selected) = (chat.clone(), chat.clone(), chat.clone(), chat.clone(), chat.current);
    let (shown, named, menu_title) = (title.clone(), title.clone(), title.clone());
    // One element per row: while renaming, its title is a field.
    list_item(keyed(
        move || renaming.get() == Some(id),
        move |editing| {
            if *editing {
                rename_field(&rename, id, &title()).into()
            } else {
                text(shown.clone()).id(format!("thread-title-{id}")).truncate().flex_1()
            }
        },
    ))
    .id(format!("thread-{id}"))
    .label(named)
    .selected(move || selected.get() == Some(id))
    .focus_preferred(move || selected.get() == Some(id))
    .on_press(move || {
        open.open(id);
        listing.set(false);
    })
    .on_secondary_press(move || thread_menu(&menu, id, menu_title()))
    .child(show(move || busy.is_answering(id), spinner))
}

fn rename_field(chat: &Chat, id: ThreadId, title: &str) -> TextInput {
    let draft = state("rename", || title.to_owned());
    let (submit, blur, cancel) = (chat.clone(), chat.clone(), chat.renaming);
    text_field(draft)
        .id(format!("rename-{id}"))
        .label("Conversation name")
        .w_full()
        .on_submit(move || submit.rename(id, &untrack(|| draft.get())))
        .on_blur(move || blur.rename(id, &untrack(|| draft.get())))
        .on_key(move |k| {
            if k.key == Key::Escape {
                cancel.set(None);
                return true;
            }
            false
        })
}

/// A conversation's context menu: rename it in place, or delete it once confirmed.
fn thread_menu(chat: &Chat, id: ThreadId, title: String) {
    let chat = chat.clone();
    spawn(async move {
        let menu = Menu::new("Conversation").items([
            MenuItem::action("Rename", "rename"),
            MenuItem::separator(),
            MenuItem::action("Delete…", "delete"),
        ]);
        match popup_menu(menu, None).await.as_deref() {
            Some("rename") => {
                chat.renaming.set(Some(id));
                focus(&format!("rename-{id}"));
            }
            Some("delete") => {
                let message = format!("Delete “{title}”?");
                let asked = prompt(
                    PromptLevel::Warning,
                    &message,
                    Some("The conversation and its messages are deleted. This can’t be undone."),
                    &["Cancel", "Delete"],
                );
                if asked.await == Some(1) {
                    chat.delete(id);
                }
            }
            _ => {}
        }
    });
}

// ---- the open conversation ----

fn main_pane(chat: &Chat, listing: Signal<bool>) -> El {
    let settings_open = state("settings-open", || false);
    let settings = settings_dialog(chat.clone(), settings_open);
    let title = chat.clone();
    let current = chat.current;
    let history = chat.clone();
    col()
        .id("conversation")
        .size_full()
        .bg(Token::Background)
        .child(
            row()
                .id("conversation-header")
                .items_center()
                .gap_2()
                .h(64.0)
                .px_4()
                .child(show(compact, move || {
                    icon_button(icons::MENU, "Conversations").id("conversations").on_press(move || listing.set(true))
                }))
                .child(
                    text(move || {
                        let current = title.current.get();
                        current.and_then(|t| title.threads.item(&t)).map_or_else(|| "New chat".to_owned(), |t| t.get().title)
                    })
                    .id("title")
                    .role(Role::Heading)
                    .level(1)
                    .font_medium()
                    .text_sm()
                    .truncate()
                    .min_w(0.0)
                    .flex_1(),
                )
                .child(model_picker(chat))
                .child(icon_button(icons::SETTINGS, "Settings").id("settings-open").on_press(move || settings_open.set(true))),
        )
        .child(el().relative().flex_1().min_h(0.0).child(keyed(move || current.get().unwrap_or(0), move |t| transcript(&history, *t))))
        .child(composer(chat))
        .child(settings)
}

#[ui]
fn settings_dialog(chat: Chat, open: Signal<bool>) -> View {
    dialog(
        "settings",
        "Conversation settings",
        open,
        DialogOptions {
            description: "Choose how your next answer is generated. Messages already sent or queued keep their settings.".into(),
            click_outside: false,
        },
        move || {
            let initial = untrack(|| chat.generation.get());
            let model = state("settings-model", || untrack(|| chat.model.get()));
            let temperature = state("temperature", || initial.temperature.map_or_else(String::new, |n| n.to_string()));
            let tokens = state("tokens", || initial.max_completion_tokens.map_or_else(String::new, |n| n.to_string()));
            let error = state("save-error", String::new);
            let options: Vec<_> = chat.models().iter().map(|entry| Choice::new(&entry.id, entry.model.name())).collect();
            let available: Vec<_> = chat.models().iter().map(|entry| entry.id.clone()).collect();
            let valid = memo(move || {
                model.get().is_some_and(|id| available.contains(&id))
                    && crate::settings::Generation::parse(&temperature.get(), &tokens.get()).is_ok()
            });
            let save = chat.clone();
            let default_model = chat.models().first().map(|entry| entry.id.clone());
            col()
                .gap_3()
                .child(text("Model").font_medium())
                .child(combobox(
                    "settings-model",
                    "Model",
                    model,
                    move || options.clone(),
                    |caption| button(caption).w_full().min_w(0.0).truncate(),
                ))
                .child(text("Temperature (0–2)").font_medium())
                .child(
                    number_field(temperature, 0.0, 2.0, 0.1)
                        .id("settings-temperature")
                        .label("Temperature")
                        .invalid(move || crate::settings::Generation::parse(&temperature.get(), "").is_err())
                        .w_full(),
                )
                .child(text("Maximum output tokens (1–131072)").font_medium())
                .child(
                    number_field(tokens, 1.0, 131072.0, 1.0)
                        .id("settings-tokens")
                        .label("Maximum output tokens")
                        .invalid(move || crate::settings::Generation::parse("", &tokens.get()).is_err())
                        .w_full(),
                )
                .child(
                    text("Leave blank to use model defaults. Available parameters and output limits vary by model.")
                        .text_sm()
                        .text_color(Token::TextMuted),
                )
                .child(
                    text(move || {
                        crate::settings::Generation::parse(&temperature.get(), &tokens.get())
                            .err()
                            .unwrap_or_else(|| if !valid.get() { "Choose an available model.".into() } else { error.get() })
                    })
                    .id("settings-error")
                    .role(Role::Status)
                    .text_sm(),
                )
                .child(
                    row()
                        .gap_2()
                        .child(button("Restore defaults").id("settings-reset").on_press(move || {
                            model.set(default_model.clone());
                            temperature.set(String::new());
                            tokens.set(String::new());
                            error.set(String::new());
                        }))
                        .child(spacer())
                        .child(button("Cancel").id("settings-cancel").on_press(move || open.set(false)))
                        .child(primary_button("Save").id("settings-save").disabled(move || !valid.get()).on_press(move || {
                            let result = crate::settings::Generation::parse(&temperature.get(), &tokens.get())
                                .and_then(|generation| save.configure(model.get(), generation));
                            match result {
                                Ok(()) => open.set(false),
                                Err(message) => error.set(message),
                            }
                        })),
                )
        },
    )
}

/// Labels belong to people; the selection and retained option rows use the model's stable ID.
#[ui]
fn model_picker(chat: &Chat) -> View {
    let options: Vec<_> = chat.models().iter().map(|entry| Choice::new(&entry.id, entry.model.name())).collect();
    let empty = options.is_empty();
    combobox(
        "model",
        "Model",
        chat.model,
        move || options.clone(),
        move |caption| {
            let caption = move || if empty { "No models".into() } else { caption.get() };
            button(caption)
                .min_w(0.0)
                .max_w(relative(0.55))
                .flex_shrink_0()
                .truncate()
                .style_with(|style| style.w(if compact() { 128.0 } else { 220.0 }))
                .tooltip(move || format!("{}\nApplies to the next message or regenerated answer.", caption()))
                .disabled(empty)
        },
    )
}

/// The open conversation's messages: one document to select across, following its end as the
/// answer streams, with a way back to the end when scrolled away.
fn transcript(chat: &Chat, thread: ThreadId) -> View {
    let messages = chat.messages.read_only().filter(move |m| m.thread == thread);
    let empty = messages.clone();
    let rows = chat.clone();
    let draft = chat.draft;
    View::fragment(vec![
        col()
            .id("transcript")
            .size_full()
            .overflow_y_scroll()
            .follow_tail()
            .selectable()
            .copy_markdown(|_| None)
            .child(virtual_each(
                move || messages.clone(),
                |m: &Message| if m.author == Author::User { 60.0 } else { 160.0 },
                move |id, m| message(&rows, *id, m),
            ))
            .child(show(
                move || empty.is_empty(),
                move || {
                    col()
                        .items_center()
                        .justify_center()
                        .flex_grow()
                        .flex_shrink_0()
                        .gap_4()
                        .px_6()
                        .py_8()
                        .w_full()
                        .max_w(650.0)
                        .mx_auto()
                        .child(brand_mark(48.0))
                        .child(
                            col()
                                .items_center()
                                .gap_1()
                                .child(
                                    text("A little curiosity.")
                                        .font_medium()
                                        .style_with(|p| if compact() { p.text_2xl() } else { p.text_3xl() }),
                                )
                                .child(
                                    text("A world of possibilities.")
                                        .font_medium()
                                        .style_with(|p| if compact() { p.text_2xl() } else { p.text_3xl() }),
                                ),
                        )
                        .child(
                            text("Make room for your next idea. Think, write, and build together.")
                                .text_sm()
                                .text_color(Token::TextMuted)
                                .self_center(),
                        )
                        .child(show(
                            || !compact(),
                            move || {
                                row()
                                    .w_full()
                                    .gap_2()
                                    .mt_4()
                                    .flex_wrap()
                                    .child(starter(
                                        draft,
                                        "starter-write",
                                        icons::PENCIL,
                                        "Find the right words",
                                        "Help me write",
                                        "Help me write a thoughtful, concise introduction. Ask me what it is for first.",
                                    ))
                                    .child(starter(
                                        draft,
                                        "starter-learn",
                                        icons::BOOK_OPEN,
                                        "Make it click",
                                        "Explain a concept",
                                        "Explain a complex idea in simple terms. Ask me which topic I want to understand.",
                                    ))
                                    .child(starter(
                                        draft,
                                        "starter-build",
                                        icons::CODE,
                                        "Build something",
                                        "Explore an idea",
                                        "Help me turn an idea into a small, practical project. Ask me about my idea first.",
                                    ))
                            },
                        ))
                        .select_none()
                },
            ))
            .into(),
        show(
            || !following_tail("transcript").get(),
            || {
                el().absolute().bottom(12.0).left(0.0).right(0.0).items_center().justify_center().child(
                    icon_button(icons::ARROW_DOWN, "Jump to latest")
                        .id("jump")
                        .bg(Token::Surface)
                        .border_color(Token::Border)
                        .rounded_full()
                        .shadow_md()
                        .on_press(|| {
                            scroll_to_end("transcript");
                        }),
                )
            },
        ),
    ])
}

#[ui]
fn starter(draft: Signal<String>, id: &str, symbol: icons::Icon, title: &str, caption: &str, prompt: &'static str) -> El {
    button_root(el())
        .id(id)
        .label(title)
        .description(caption)
        .flex_1()
        .min_w(150.0)
        .p_4()
        .rounded_xl()
        .bg(Token::Background)
        .hover(|p| p.bg(Token::Surface))
        .cursor_pointer()
        .child(
            col()
                .gap_2()
                .a11y_hidden()
                .child(icon(symbol).size_4().text_color(Token::Accent))
                .child(text(title.to_owned()).text_sm().font_medium())
                .child(text(caption.to_owned()).text_xs().text_color(Token::TextMuted)),
        )
        .on_press(move || {
            draft.set(prompt.into());
            focus("composer");
        })
}

/// One message: the person's on the right; an answer's parts, its state and actions.
fn message(chat: &Chat, id: u64, m: ReadSignal<Message>) -> El {
    let author = untrack(|| m.with(|m| m.author));
    let body = match author {
        Author::User => user_message(chat, id, m),
        Author::Assistant => answer(chat, id, m),
    };
    col().id(format!("msg-{id}")).px_6().py_4().w_full().max_w(800.0).mx_auto().child(body)
}

fn user_message(chat: &Chat, id: u64, m: ReadSignal<Message>) -> El {
    let (edit, copy) = (chat.clone(), m);
    let editing = chat.editing;
    let parts = untrack(|| m.with(|m| m.parts.clone()));
    let files: Vec<View> =
        parts.iter().filter_map(|(_, p)| if let Part::File(f) = p { Some(chip(f.name.clone()).into()) } else { None }).collect();
    let body = parts.into_iter().find_map(|(_, p)| if let Part::Text(t) = p { Some(t) } else { None });
    col()
        .group("msg")
        .items_end()
        .gap_1()
        .child(row().gap_1().children(files))
        .children(body.map(|t| {
            text(t)
                .id(format!("msg-body-{id}"))
                .px_4()
                .py_3()
                .rounded_2xl()
                .bg(Token::Surface)
                .text_base()
                .max_w(560.0)
                .style_when(State::SELECTED, |p| p.border_color(Token::Accent))
        }))
        .child(
            row()
                .gap_0p5()
                .select_none()
                .child(
                    icon_button(icons::COPY, "Copy")
                        .id(format!("copy-{id}"))
                        .on_press(move || write_to_clipboard(untrack(|| copy.with(|m| m.text())))),
                )
                .child(icon_button(icons::PENCIL, "Edit").id(format!("edit-{id}")).on_press(move || edit.edit(id))),
        )
        .child(show(
            move || editing.get() == Some(id),
            || text("Editing — send to replace this and what followed").text_xs().text_color(Token::Accent).select_none(),
        ))
}

fn answer(chat: &Chat, id: u64, m: ReadSignal<Message>) -> El {
    let status = move || m.with(|m| m.status.clone());
    let (regenerate, copy) = (chat.clone(), m);
    col()
        .group("msg")
        .gap_3()
        .child(row().items_center().gap_2().select_none().child(brand_mark(24.0)).child(text("Octara").text_sm().font_semibold()))
        .child(each(move || m.with(|m| m.parts.clone()), move |part_id, p| part(id, *part_id, p)))
        .child(show(
            move || status() == Status::Streaming && m.with(|m| m.parts.is_empty()),
            move || {
                row()
                    .id(format!("waiting-{id}"))
                    .gap_2()
                    .items_center()
                    .child(spinner())
                    .child(text("Waiting for response…").text_sm().text_color(Token::TextMuted))
                    .select_none()
            },
        ))
        .child(show(move || matches!(status(), Status::Failed(_)), {
            let retry = chat.clone();
            move || {
                let why = match untrack(|| m.get()).status {
                    Status::Failed(w) => w,
                    _ => String::new(),
                };
                let (retry, available) = (retry.clone(), retry.clone());
                callout(PromptLevel::Critical, "The answer failed", Some(&why)).id(format!("failed-{id}")).select_none().child(
                    button("Retry")
                        .id(format!("retry-{id}"))
                        .disabled(move || !available.has_model())
                        .on_press(move || retry.regenerate(id)),
                )
            }
        }))
        .child(show(move || status() == Status::Stopped, || text("Stopped").text_xs().text_color(Token::TextMuted).select_none()))
        .child(show(
            move || status() != Status::Streaming,
            move || {
                let (regenerate, available, copy) = (regenerate.clone(), regenerate.clone(), copy);
                row()
                    .gap_0p5()
                    .select_none()
                    .child(
                        icon_button(icons::COPY, "Copy")
                            .id(format!("copy-{id}"))
                            .on_press(move || write_to_clipboard(untrack(|| copy.with(|m| m.text())))),
                    )
                    .child(
                        icon_button(icons::REFRESH_CW, "Regenerate")
                            .id(format!("regenerate-{id}"))
                            .disabled(move || !available.has_model())
                            .on_press(move || regenerate.regenerate(id)),
                    )
            },
        ))
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum PartKind {
    Text,
    Reasoning,
    Tool,
    File,
}

impl PartKind {
    fn of(part: &Part) -> Self {
        match part {
            Part::Text(_) => Self::Text,
            Part::Reasoning { .. } => Self::Reasoning,
            Part::Tool(_) => Self::Tool,
            Part::File(_) => Self::File,
        }
    }
}

impl std::fmt::Display for PartKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Text => "text",
            Self::Reasoning => "reasoning",
            Self::Tool => "tool",
            Self::File => "file",
        })
    }
}

/// Equality is the buffer's identity; its diagnostic label never includes its changing text.
#[derive(PartialEq, Eq, Hash)]
struct TextResource(TextBuffer);

impl std::fmt::Display for TextResource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("buffer")
    }
}

fn text_resource(p: ReadSignal<Part>) -> TextResource {
    p.with(|p| {
        TextResource(match p {
            Part::Text(t) | Part::Reasoning { text: t, .. } => t.clone(),
            Part::Tool(t) => t.input.clone(),
            Part::File(_) => unreachable!("file parts have no text subtree"),
        })
    })
}

/// The row keeps its identity through replacement; its variant and captured buffers have
/// separate child lifetimes. Status changes only update the bindings inside the current kind.
#[ui]
fn part(id: u64, part_id: PartId, p: ReadSignal<Part>) -> El {
    col().id(format!("part-{id}-{part_id}")).w_full().min_w(0.0).child(keyed(
        move || p.with(PartKind::of),
        move |kind| match kind {
            PartKind::Text => {
                keyed(move || text_resource(p), move |resource| markdown(paced(&resource.0)).id(format!("text-{id}-{part_id}")))
            }
            PartKind::Reasoning => reasoning(id, part_id, p),
            PartKind::Tool => tool(id, part_id, p),
            PartKind::File => chip(move || {
                p.with(|p| match p {
                    Part::File(f) => f.name.clone(),
                    _ => String::new(),
                })
            })
            .into(),
        },
    ))
}

/// What the model thought: open while it thinks, closed once it has (the person may open it).
#[ui]
fn reasoning(id: u64, part_id: PartId, p: ReadSignal<Part>) -> View {
    let ended = move || match p.get() {
        Part::Reasoning { started, ended, .. } => ended.map(|e| e.saturating_sub(started)),
        _ => None,
    };
    let open = state("open", move || untrack(ended).is_none());
    watch(
        move || ended().is_some(),
        move |done, before| {
            if *done && before == Some(&false) {
                open.set(false);
            }
        },
    );
    let header =
        row().gap_1p5().items_center().child(show(move || ended().is_none(), || spinner().size_3p5())).child(text(move || match ended() {
            None => "Thinking…".to_owned(),
            Some(d) => format!("Thought for {}", seconds(d)),
        }));
    collapsible(&format!("reasoning-{id}-{part_id}"), open, header, move || {
        keyed(
            move || text_resource(p),
            move |resource| {
                text(paced(&resource.0)).id(format!("reasoning-text-{id}-{part_id}")).text_sm().text_color(Token::TextMuted).pl_5()
            },
        )
    })
    .into()
}

fn seconds(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 1.0 { "less than a second".to_owned() } else { format!("{} s", s.round() as u64) }
}

/// A tool call: its name and state; open, its input and output.
#[ui]
fn tool(id: u64, part_id: PartId, p: ReadSignal<Part>) -> View {
    let tool = move || match p.get() {
        Part::Tool(t) => Some(t),
        _ => None,
    };
    let open = state("open", || false);
    let state_label = move || match tool().map(|t| t.state) {
        Some(ToolState::Streaming) => "Preparing".to_owned(),
        Some(ToolState::Running) => "Running".to_owned(),
        Some(ToolState::Done) => "Done".to_owned(),
        Some(ToolState::Failed(why)) => format!("Failed: {why}"),
        None => String::new(),
    };
    let working = move || tool().is_some_and(|t| matches!(t.state, ToolState::Streaming | ToolState::Running));
    let name = move || tool().map(|t| t.name).unwrap_or_default();
    let header = row()
        .gap_1p5()
        .items_center()
        .child(icon(icons::WRENCH).size_3p5().a11y_hidden())
        .child(text(name).font_medium())
        .child(show(working, || spinner().size_3p5()))
        .child(text(state_label).text_xs());
    collapsible(&format!("tool-{id}-{part_id}"), open, header, move || {
        col()
            .gap_1()
            .pl_5()
            .text_xs()
            .child(text("Input").text_color(Token::TextMuted))
            .child(keyed(
                move || text_resource(p),
                move |resource| {
                    text(resource.0.clone()).id(format!("tool-input-{id}-{part_id}")).font_mono().p_2().rounded_md().bg(Token::Surface)
                },
            ))
            .child(show(
                move || tool().and_then(|t| t.output).is_some(),
                move || {
                    col().gap_1().child(text("Output").text_color(Token::TextMuted)).child(
                        text(move || tool().and_then(|t| t.output).unwrap_or_default()).font_mono().p_2().rounded_md().bg(Token::Surface),
                    )
                },
            ))
    })
    .into()
}

// ---- the composer ----

fn composer(chat: &Chat) -> El {
    let current = chat.current;
    let (send, stop, attach, drop_in, busy, busy2, editing, cancel_edit) =
        (chat.clone(), chat.clone(), chat.clone(), chat.clone(), chat.clone(), chat.clone(), chat.editing, chat.clone());
    let answering = move || current.get().is_some_and(|t| busy.is_answering(t));
    let files = chat.files;
    let available = chat.clone();
    let file_actions = chat.clone();
    col()
        .id("composer-area")
        .w_full()
        .max_w(800.0)
        .mx_auto()
        .px_4()
        .pb_4()
        .gap_2()
        .on_drop(move |paths| drop_in.attach(paths))
        .drag_over(|p| p.bg(Token::Surface))
        .child(composer_queue(chat))
        .child(show(move || !available.has_model(), {
            let no_models = chat.models().is_empty();
            move || {
                text(if no_models { "No models available. Your draft is kept." } else { "Choose a model to send. Your draft is kept." })
                    .id("model-status")
                    .role(Role::Status)
                    .live(Live::Polite)
                    .text_xs()
                    .text_color(Token::TextMuted)
            }
        }))
        .child(
            col()
                .gap_2()
                .p_3()
                .rounded_2xl()
                .border()
                .border_color(Token::Border)
                .bg(Token::Background)
                .shadow_sm()
                .child(text("Message").text_xs().text_color(Token::TextMuted).px_3())
                .child(show(
                    move || !files.is_empty(),
                    move || {
                        let file_actions = file_actions.clone();
                        row().gap_1().flex_wrap().child(each(
                            move || files.read_only(),
                            move |fid, f| {
                                let (fid, chat) = (*fid, file_actions.clone());
                                let name = untrack(|| f.with(|(_, file)| file.name.clone()));
                                removable_chip(&name, |remove| remove.id(format!("remove-file-{fid}")).on_press(move || chat.detach(fid)))
                                    .id(format!("file-{fid}"))
                            },
                        ))
                    },
                ))
                .child(
                    text_area(chat.draft)
                        .id("composer")
                        .label("Message")
                        .placeholder("Ask anything…")
                        .text_base()
                        .rows(1, 8)
                        .submit_on_enter()
                        .border_color(Paint::Color(Color::TRANSPARENT))
                        .bg(Paint::Color(Color::TRANSPARENT))
                        .on_submit(move || send.send())
                        .on_key(move |k| {
                            if k.key == Key::Escape && untrack(|| editing.get()).is_some() {
                                cancel_edit.cancel_edit();
                                return true;
                            }
                            false
                        }),
                )
                .child(
                    row()
                        .items_center()
                        .gap_1()
                        .child(
                            icon_button(icons::PAPERCLIP, "Attach files")
                                .id("attach")
                                .tooltip("Keep a local file reference. File contents are not sent to the model.")
                                .on_press(move || {
                                    let attach = attach.clone();
                                    spawn(async move {
                                        if let Some(paths) = prompt_for_paths(PathPromptOptions {
                                            files: true,
                                            directories: false,
                                            multiple: true,
                                            prompt: Some("Attach".into()),
                                        })
                                        .await
                                        {
                                            attach.attach(&paths);
                                        }
                                    });
                                }),
                        )
                        .child(text("Enter to send").text_xs().text_color(Token::TextMuted))
                        .child(spacer())
                        .child(keyed(answering, move |answering| {
                            if *answering {
                                let stop = stop.clone();
                                icon_button(icons::SQUARE, "Stop").id("stop").on_press(move || {
                                    if let Some(t) = untrack(|| stop.current.get()) {
                                        stop.stop(t);
                                    }
                                })
                            } else {
                                let (send, available) = (busy2.clone(), busy2.clone());
                                primary_icon(
                                    icon_button(icons::ARROW_UP, "Send")
                                        .id("send")
                                        .disabled(move || !available.can_send())
                                        .on_press(move || send.send()),
                                )
                            }
                        })),
                ),
        )
        .child(text("A place to think. Double-check the details that matter.").text_xs().text_color(Token::TextMuted).self_center())
}

/// A send button's look: the accent.
fn primary_icon(b: El) -> El {
    b.bg(Token::Accent).text_color(Token::AccentText).rounded_full().hover(|p| p.bg(Token::AccentHover).text_color(Token::AccentText))
}

#[derive(PartialEq, Eq, Hash)]
struct QueueThread(Option<ThreadId>);

impl std::fmt::Display for QueueThread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(thread) => write!(f, "{thread}"),
            None => f.write_str("new chat"),
        }
    }
}

#[ui]
fn composer_queue(chat: &Chat) -> View {
    let current = chat.current;
    let chat = chat.clone();
    keyed(
        move || QueueThread(current.get()),
        move |key| {
            let Some(thread) = key.0 else { return View::empty() };
            let queued = chat.queue.read_only().filter(move |q| q.thread == thread);
            let chat = chat.clone();
            each(move || queued.clone(), move |id, q| queued_row(&chat, *id, q))
        },
    )
}

fn queued_row(chat: &Chat, id: u64, q: ReadSignal<crate::chat::Queued>) -> El {
    let (now, edit, remove, available, label, hint) = (chat.clone(), chat.clone(), chat.clone(), chat.clone(), chat.clone(), chat.clone());
    row()
        .id(format!("queued-{id}"))
        .items_center()
        .gap_2()
        .px_3()
        .py_1p5()
        .rounded_md()
        .bg(Token::Surface)
        .text_sm()
        .child(
            col()
                .flex_1()
                .min_w(0.0)
                .gap_0p5()
                .child(
                    text(move || q.with(|q| format!("Queued · {}", label.model_name(&q.model_id).unwrap_or("Model unavailable"))))
                        .id(format!("queued-model-{id}"))
                        .truncate()
                        .text_xs()
                        .text_color(Token::TextMuted),
                )
                .child(text(move || q.with(|q| q.text.clone())).truncate()),
        )
        .child(
            icon_button(icons::ARROW_UP, "Send now")
                .id(format!("send-now-{id}"))
                .disabled(move || q.with(|q| available.model_name(&q.model_id).is_none()))
                .tooltip(move || {
                    q.with(|q| {
                        hint.model_name(&q.model_id)
                            .map_or_else(|| "Queued model is unavailable".to_owned(), |name| format!("Send now with {name}"))
                    })
                })
                .on_press(move || now.send_now(id)),
        )
        .child(icon_button(icons::PENCIL, "Edit").id(format!("edit-queued-{id}")).on_press(move || edit.edit_queued(id)))
        .child(icon_button(icons::X, "Remove").id(format!("unqueue-{id}")).on_press(move || remove.unqueue(id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Attachment, Event, Tool};
    use octara::platform::HeadlessPlatform;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[test]
    fn an_explicit_model_outlives_and_drives_reopened_product_views() {
        let app = App::new(HeadlessPlatform::default());
        let chat = app.run(|| Chat::new(Options::default()));
        let shown = chat.clone();
        let mut first = app.open_window(WindowOptions::new("First chat").size(1000.0, 720.0), move || view(&shown)).unwrap();

        first.dispatch(octara::app::Event::Input("composer".into(), "From the first window".into())).unwrap();

        assert_eq!(app.run(|| chat.draft.get()), "From the first window");
        app.close_window(first.id());
        app.run(|| chat.draft.set("Kept by the caller".into()));
        let shown = chat.clone();
        let mut second = app.open_window(WindowOptions::new("Second chat").size(1000.0, 720.0), move || view(&shown)).unwrap();

        assert_eq!(second.kernel().value_of("composer").as_deref(), Some("Kept by the caller"));
        second.dispatch(octara::app::Event::Input("composer".into(), "From the second window".into())).unwrap();
        assert_eq!(app.run(|| chat.draft.get()), "From the second window", "view callbacks act on the supplied model");
        assert!(second.layout_issues().is_empty(), "{:?}", second.layout_issues());
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
        app.shutdown();
    }

    type Action = Box<dyn FnOnce(Signal<Message>)>;

    struct Parts {
        window: Window,
        action: Rc<RefCell<Option<Action>>>,
        reads: Rc<Cell<usize>>,
    }

    impl Parts {
        fn new(init: impl FnOnce() -> Message + 'static) -> Self {
            let action: Rc<RefCell<Option<Action>>> = Rc::default();
            let invoke = action.clone();
            let reads = Rc::new(Cell::new(0));
            let counted = reads.clone();
            let init = RefCell::new(Some(init));
            let window = Window::headless(move || {
                let chat = Chat::new(Options { models: Vec::new(), ..Options::default() });
                let message = state("message", || init.borrow_mut().take().unwrap()());
                let counted = counted.clone();
                // Every structural source reads this memo. Buffer edits must leave it clean.
                let current = memo(move || {
                    counted.set(counted.get() + 1);
                    message.get()
                });
                let invoke = invoke.clone();
                on_command("test.parts", move || {
                    if let Some(action) = invoke.borrow_mut().take() {
                        action(message);
                    }
                });
                col().size_full().child(answer(&chat, 1, current).id("answer"))
            });
            Self { window, action, reads }
        }

        fn run(&mut self, action: impl FnOnce(Signal<Message>) + 'static) {
            *self.action.borrow_mut() = Some(Box::new(action));
            self.window.invoke("test.parts").unwrap();
            self.window.app().advance(Duration::from_secs(1));
        }

        fn target(&self, id: &str) -> String {
            self.window.inspect().find(id).unwrap().target.clone()
        }

        fn texts(&self, id: &str) -> String {
            let kernel = self.window.kernel();
            let mut stack = vec![kernel.find(id).unwrap()];
            let mut texts = Vec::new();
            while let Some(node) = stack.pop() {
                if !kernel.text(node).is_empty() {
                    texts.push(kernel.text(node).to_owned());
                }
                stack.extend(kernel.children(node).iter().rev());
            }
            texts.join(" ")
        }

        fn clean(&self) {
            assert!(self.window.layout_issues().is_empty(), "{:?}", self.window.layout_issues());
            assert!(self.window.app().diagnostics().is_empty(), "{:?}", self.window.app().diagnostics());
        }
    }

    #[test]
    fn parts_reorder_delete_and_replace_kind_by_business_key() {
        let mut parts = Parts::new(|| {
            let mut message = Message::answer(1, 1);
            for text in ["first", "second", "third"] {
                message.push_part(Part::Text(TextBuffer::new(text)));
            }
            message
        });
        let retained = parts.target("part-1-1");
        let removed = parts.target("part-1-0");
        parts.run(|message| {
            message.update(|message| {
                message.parts.reverse();
                message.parts.retain(|(id, _)| *id != 0);
            })
        });
        assert_eq!(&parts.window.kernel().child_ids("answer")[..2], ["part-1-2", "part-1-1"]);
        assert_eq!(parts.target("part-1-1"), retained);
        assert!(!parts.window.kernel().exists("part-1-0"));

        let text = parts.target("text-1-1");
        parts.run(|message| message.update(|message| message.parts[1].1 = Part::Text(TextBuffer::new("replacement text"))));
        assert_eq!(parts.target("part-1-1"), retained);
        assert_ne!(parts.target("text-1-1"), text);
        assert_eq!(parts.texts("text-1-1"), "replacement text");

        parts.run(|message| {
            message.update(|message| {
                message.parts[1].1 = Part::File(Attachment { name: "replacement.txt".into(), path: "replacement.txt".into() });
            })
        });
        assert_eq!(parts.target("part-1-1"), retained, "a new kind replaces only the row's content");
        assert!(!parts.window.kernel().exists("text-1-1"));
        assert_eq!(parts.texts("part-1-1"), "replacement.txt");
        parts.run(|message| {
            message.update(|message| {
                message.parts.push((0, Part::Text(TextBuffer::new("returned"))));
            })
        });
        assert_ne!(parts.target("part-1-0"), removed, "published deletion retires the old row's target");
        parts.clean();
    }

    #[test]
    fn reasoning_buffer_replacement_preserves_the_open_disclosure() {
        let mut parts = Parts::new(|| {
            let mut message = Message::answer(1, 1);
            message.push_part(Part::Reasoning {
                text: TextBuffer::new("old thought"),
                started: Duration::ZERO,
                ended: Some(Duration::from_secs(1)),
            });
            message
        });
        parts.window.tap("reasoning-1-0-trigger").unwrap();
        let trigger = parts.target("reasoning-1-0-trigger");
        let old_text = parts.target("reasoning-text-1-0");
        parts.run(|message| {
            message.update(|message| {
                let Part::Reasoning { text, .. } = &mut message.parts[0].1 else { panic!() };
                *text = TextBuffer::new("new thought");
            })
        });
        assert_eq!(parts.window.kernel().expanded_of("reasoning-1-0-trigger"), Some(true));
        assert_eq!(parts.target("reasoning-1-0-trigger"), trigger);
        assert_ne!(parts.target("reasoning-text-1-0"), old_text);
        assert_eq!(parts.texts("reasoning-text-1-0"), "new thought");
        parts.run(|message| message.update(|message| message.status = Status::Stopped));
        assert_eq!(parts.target("reasoning-1-0-trigger"), trigger);
        assert_eq!(parts.window.kernel().expanded_of("reasoning-1-0-trigger"), Some(true));
        let row = parts.target("part-1-0");
        parts.run(|message| message.update(|message| message.parts[0].1 = Part::Text(TextBuffer::new("visible answer"))));
        assert_eq!(parts.target("part-1-0"), row);
        assert!(!parts.window.kernel().exists("reasoning-1-0-trigger"));
        assert_eq!(parts.texts("text-1-0"), "visible answer");
        parts.clean();
    }

    #[test]
    fn tool_status_binds_in_place_and_only_a_new_input_replaces_its_text() {
        let mut parts = Parts::new(|| {
            let mut message = Message::answer(1, 1);
            message.push_part(Part::Tool(Tool {
                id: "lookup".into(),
                name: "lookup".into(),
                input: TextBuffer::new("old input"),
                state: ToolState::Streaming,
                output: None,
            }));
            message
        });
        parts.window.tap("tool-1-0-trigger").unwrap();
        let trigger = parts.target("tool-1-0-trigger");
        let input = parts.target("tool-input-1-0");
        parts.run(|message| {
            message.update(|message| {
                let Part::Tool(tool) = &mut message.parts[0].1 else { panic!() };
                tool.name = "renamed lookup".into();
                tool.state = ToolState::Done;
                tool.output = Some("found".into());
            })
        });
        assert_eq!(parts.target("tool-1-0-trigger"), trigger);
        assert_eq!(parts.target("tool-input-1-0"), input);
        assert!(parts.texts("tool-1-0").contains("renamed lookup Done"));
        assert!(parts.texts("tool-1-0").contains("found"));
        parts.run(|message| {
            message.update(|message| {
                let Part::Tool(tool) = &mut message.parts[0].1 else { panic!() };
                let old = std::mem::replace(&mut tool.input, TextBuffer::new("new input"));
                old.append(" stale delta");
            })
        });
        assert_eq!(parts.target("tool-1-0-trigger"), trigger);
        assert_eq!(parts.window.kernel().expanded_of("tool-1-0-trigger"), Some(true));
        assert_ne!(parts.target("tool-input-1-0"), input);
        assert_eq!(parts.texts("tool-input-1-0"), "new input");
        parts.clean();
    }

    #[test]
    fn streamed_tokens_leave_message_structure_and_targets_untouched() {
        let mut parts = Parts::new(|| {
            let mut message = Message::answer(1, 1);
            message.push_part(Part::Text(TextBuffer::new("first")));
            message
        });
        let row = parts.target("part-1-0");
        let text = parts.target("text-1-0");
        let reads = parts.reads.get();
        for _ in 0..3 {
            parts.run(|message| assert!(message.with(|message| message.stream(&Event::Text(" token".into())))));
        }
        assert_eq!(parts.reads.get(), reads, "streaming never invalidates the message/parts source");
        assert_eq!(parts.target("part-1-0"), row);
        assert_eq!(parts.target("text-1-0"), text);
        assert_eq!(parts.texts("text-1-0"), "first token token token");
        parts.clean();
    }
}
