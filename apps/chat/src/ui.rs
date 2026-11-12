//! The chat's views, composed from the kit: a `split` with the conversations (`list`) beside the
//! open one; its transcript a selectable scroll container following its tail over a virtual
//! list of messages; parts as Markdown (paced), `collapsible` reasoning and tool calls, a
//! `callout` for a failure; a composer (`text_field`, `removable_chip`s, `icon_button`s) with
//! the queue above it.

use crate::chat::{Chat, Options};
use crate::model::{Author, Message, Part, Status, Thread, ThreadId, Tool, ToolState};
use octara::prelude::*;
use octara_icons as icons;
use octara_kit::markdown::markdown;
use octara_kit::*;
use std::cmp::Reverse;
use std::time::Duration;

/// Whether a phone shows the conversations (instead of the open one).
#[derive(Clone, Copy)]
struct Listing(Signal<bool>);

fn listing() -> Signal<bool> {
    use_context::<Listing>().expect("inside the chat").0
}

fn compact() -> bool {
    breakpoint().get() == Breakpoint::Compact
}

fn chat() -> Chat {
    use_context::<Chat>().expect("inside the chat")
}

/// The app: conversations beside the open one.
pub fn root(options: impl Fn() -> Options + 'static) -> View {
    component("Chat", move || {
        let chat = Chat::new(options());
        provide(chat.clone());
        commands(&chat);
        let width = state("sidebar", || 260.0f32);
        let clamp = move |w: f32| width.set(w.clamp(180.0, 420.0));
        // A phone shows the conversations or the open one; wider windows show both.
        let compact = memo(|| breakpoint().get() == Breakpoint::Compact);
        let listing = state("listing", || false);
        provide(Listing(listing));
        show_else(
            move || compact.get(),
            move || show_else(move || listing.get(), || sidebar().into(), || main_pane().into()),
            move || split("panes", (move || width.get(), clamp), sidebar(), main_pane()).into(),
        )
    })
}

/// What the app's commands do, and its menu bar.
fn commands(chat: &Chat) {
    let c = chat.clone();
    on_command("chat.new", move || c.new_chat());
    let c = chat.clone();
    on_command("chat.stop", move || {
        if let Some(t) = c.current.get_untracked() {
            c.stop(t);
        }
    });
    on_command("chat.focus_composer", || focus("composer"));
    bind_keys([("mod+n", "chat.new", None), ("mod+l", "chat.focus_composer", None), ("mod+.", "chat.stop", None)]).expect("chords");
    set_menus([
        Menu::new("File").item(MenuItem::action("New Chat", "chat.new").shortcut("mod+n")),
        Menu::edit(),
        Menu::new("Chat")
            .item(MenuItem::action("Focus Composer", "chat.focus_composer").shortcut("mod+l"))
            .item(MenuItem::action("Stop Answering", "chat.stop").shortcut("mod+.")),
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

fn sidebar() -> El {
    let chat = chat();
    let now = chat.now();
    let flat = chat.threads.sort_by(|t| Reverse(t.updated)).group_by(move |t| age(t.updated, now)).flatten();
    let new = chat.clone();
    let listing = listing();
    col()
        .id("sidebar")
        .size_full()
        .bg(Token::Surface)
        .child(row().items_center().h(44.0).px_3().child(text("Chats").font_medium().text_sm().flex_1()).child(
            icon_button(icons::SQUARE_PEN, "New chat").id("new-chat").on_press(move || {
                new.new_chat();
                listing.set(false);
            }),
        ))
        .child(list().id("threads").label("Conversations").flex_1().min_h(0.0).px_2().pb_2().overflow_y_scroll().child(virtual_each(
            move || flat.source(),
            |item| if item.header().is_some() { 36.0 } else { 34.0 },
            move |key, item| match key.clone() {
                FlatKey::Header(age) => list_header(age.to_string()).id(format!("age-{age:?}")).into(),
                FlatKey::Item(id) => thread_row(id, move || item.with(|f| f.item().cloned())),
            },
        )))
}

fn thread_row(id: ThreadId, thread: impl Fn() -> Option<Thread> + Clone + 'static) -> View {
    let chat = chat();
    let title = move || thread().map(|t| t.title).unwrap_or_default();
    let renaming = chat.renaming;
    let back = listing();
    let (open, menu, busy, selected) = (chat.clone(), chat.clone(), chat.clone(), chat.current);
    let (shown, named, menu_title) = (title.clone(), title.clone(), title.clone());
    // One element per row: while renaming, its title is a field.
    list_item(show_else(
        move || renaming.get() == Some(id),
        {
            let chat = chat.clone();
            move || rename_field(&chat, id, &title())
        },
        move || {
            let shown = shown.clone();
            text(shown.clone()).truncate().flex_1().into()
        },
    ))
    .id(format!("thread-{id}"))
    .label(named)
    .selected(move || selected.get() == Some(id))
    .on_press(move || {
        open.open(id);
        back.set(false);
    })
    .on_secondary_press(move || thread_menu(&menu, id, menu_title()))
    .child(show(move || busy.is_answering(id), || spinner().into()))
    .into()
}

fn rename_field(chat: &Chat, id: ThreadId, title: &str) -> View {
    let draft = state("rename", || title.to_owned());
    let (submit, blur, cancel) = (chat.clone(), chat.clone(), chat.renaming);
    text_field(draft)
        .id(format!("rename-{id}"))
        .label("Conversation name")
        .w_full()
        .on_submit(move || submit.rename(id, &draft.get_untracked()))
        .on_blur(move || blur.rename(id, &draft.get_untracked()))
        .on_key(move |k| {
            if k.key == Key::Escape {
                cancel.set(None);
                return true;
            }
            false
        })
        .into()
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
    })
    .detach();
}

// ---- the open conversation ----

fn main_pane() -> El {
    let chat = chat();
    let (title, models) = (chat.clone(), chat.models());
    let listing = listing();
    col()
        .id("conversation")
        .size_full()
        .bg(Token::Background)
        .child(
            row()
                .items_center()
                .gap_2()
                .h(44.0)
                .px_4()
                .child(show(compact, move || {
                    icon_button(icons::MENU, "Conversations").id("conversations").on_press(move || listing.set(true)).into()
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
                    .flex_1(),
                )
                .child(select("model", chat.model, models.into_iter().enumerate())),
        )
        .child(el().h(1.0).w_full().bg(Token::Border).a11y_hidden())
        .child(el().relative().flex_1().min_h(0.0).child(keyed(move || chat.current.get().unwrap_or(0), |t| transcript(*t))))
        .child(composer())
}

/// The open conversation's messages: one document to select across, following its end as the
/// answer streams, with a way back to the end when scrolled away.
fn transcript(thread: ThreadId) -> View {
    let chat = chat();
    let messages = chat.messages.filter(move |m| m.thread == thread);
    let empty = messages;
    View::Fragment(vec![
        col()
            .id("transcript")
            .size_full()
            .overflow_y_scroll()
            .follow_tail()
            .selectable()
            .copy_markdown(|_| None)
            .child(virtual_each(
                move || messages.source(),
                |m: &Message| if m.author == Author::User { 60.0 } else { 160.0 },
                |id, m| message(*id, m),
            ))
            .child(show(
                move || empty.is_empty(),
                || {
                    col()
                        .items_center()
                        .justify_center()
                        .flex_1()
                        .gap_2()
                        .p_8()
                        .child(icon(icons::MESSAGE_SQUARE).size_8().text_color(Token::TextMuted))
                        .child(text("Ask anything. The scripted model answers offline.").text_sm().text_color(Token::TextMuted))
                        .select_none()
                        .into()
                },
            ))
            .into(),
        show(
            || !following_tail("transcript").get(),
            || {
                el().absolute()
                    .bottom(12.0)
                    .left(0.0)
                    .right(0.0)
                    .items_center()
                    .justify_center()
                    .child(
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
                    .into()
            },
        ),
    ])
}

/// One message: the person's on the right; an answer's parts, its state and actions.
fn message(id: u64, m: ReadSignal<Message>) -> View {
    let author = m.with_untracked(|m| m.author);
    let body = match author {
        Author::User => user_message(id, m),
        Author::Assistant => answer(id, m),
    };
    col().id(format!("msg-{id}")).px_4().py_2().w_full().max_w(760.0).mx_auto().child(body).into()
}

fn user_message(id: u64, m: ReadSignal<Message>) -> El {
    let chat = chat();
    let (edit, copy) = (chat.clone(), m);
    let editing = chat.editing;
    let parts = m.with_untracked(|m| m.parts.clone());
    let files: Vec<View> =
        parts.iter().filter_map(|p| if let Part::File(f) = p { Some(chip(f.name.clone()).into()) } else { None }).collect();
    let body = parts.into_iter().find_map(|p| if let Part::Text(t) = p { Some(t) } else { None });
    col()
        .group("msg")
        .items_end()
        .gap_1()
        .child(row().gap_1().children(files))
        .children(body.map(|t| {
            text(t)
                .id(format!("msg-body-{id}"))
                .px_3()
                .py_2()
                .rounded_lg()
                .bg(Token::Surface)
                .text_sm()
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
                        .on_press(move || write_to_clipboard(copy.with_untracked(|m| m.text()))),
                )
                .child(icon_button(icons::PENCIL, "Edit").id(format!("edit-{id}")).on_press(move || edit.edit(id))),
        )
        .child(show(
            move || editing.get() == Some(id),
            || text("Editing — send to replace this and what followed").text_xs().text_color(Token::Accent).select_none().into(),
        ))
}

fn answer(id: u64, m: ReadSignal<Message>) -> El {
    let chat = chat();
    // The parts as a list of their own, by index: a new part adds a row, a tool moving on
    // updates its row, and the rest (Markdown streaming in) stays as it is.
    let parts = state_list("parts", || m.with_untracked(|m| m.parts.iter().cloned().enumerate().collect()), |p: &(usize, Part)| p.0);
    watch(
        move || m.with(|m| m.parts.clone()),
        move |now, _| {
            for (i, p) in now.iter().enumerate() {
                match parts.item(&i) {
                    Some(old) if old.with_untracked(|o| o.1 == *p) => {}
                    Some(_) => parts.update(&i, |o| o.1 = p.clone()),
                    None => parts.push((i, p.clone())),
                }
            }
        },
    );
    let status = move || m.with(|m| m.status.clone());
    let (regenerate, copy) = (chat.clone(), m);
    col()
        .group("msg")
        .gap_2()
        .child(each(move || parts.source(), move |i, p| part(id, *i, p)))
        .child(show(
            move || status() == Status::Streaming && parts.is_empty(),
            || {
                row()
                    .gap_2()
                    .items_center()
                    .child(spinner())
                    .child(text("Thinking…").text_sm().text_color(Token::TextMuted))
                    .select_none()
                    .into()
            },
        ))
        .child(show(move || matches!(status(), Status::Failed(_)), {
            let retry = chat.clone();
            move || {
                let why = match m.get_untracked().status {
                    Status::Failed(w) => w,
                    _ => String::new(),
                };
                let retry = retry.clone();
                callout(PromptLevel::Critical, "The answer failed", Some(&why))
                    .id(format!("failed-{id}"))
                    .select_none()
                    .child(button("Retry").id(format!("retry-{id}")).on_press(move || retry.regenerate(id)))
                    .into()
            }
        }))
        .child(show(move || status() == Status::Stopped, || text("Stopped").text_xs().text_color(Token::TextMuted).select_none().into()))
        .child(show(
            move || status() != Status::Streaming,
            move || {
                let (regenerate, copy) = (regenerate.clone(), copy);
                row()
                    .gap_0p5()
                    .select_none()
                    .child(
                        icon_button(icons::COPY, "Copy")
                            .id(format!("copy-{id}"))
                            .on_press(move || write_to_clipboard(copy.with_untracked(|m| m.text()))),
                    )
                    .child(
                        icon_button(icons::REFRESH_CW, "Regenerate")
                            .id(format!("regenerate-{id}"))
                            .on_press(move || regenerate.regenerate(id)),
                    )
                    .into()
            },
        ))
}

/// One part of an answer.
fn part(id: u64, i: usize, p: ReadSignal<(usize, Part)>) -> View {
    match p.get_untracked().1 {
        Part::Text(t) => markdown(paced(&t)).id(format!("text-{id}-{i}")).into(),
        Part::Reasoning { text: t, .. } => reasoning(id, i, t, p),
        Part::Tool(_) => tool(id, i, p),
        Part::File(f) => chip(f.name).into(),
    }
}

/// What the model thought: open while it thinks, closed once it has (the person may open it).
fn reasoning(id: u64, i: usize, t: TextBuffer, p: ReadSignal<(usize, Part)>) -> View {
    let ended = move || match p.with(|p| p.1.clone()) {
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
    let header = row().gap_1p5().items_center().child(show(move || ended().is_none(), || spinner().size_3p5().into())).child(text(
        move || match ended() {
            None => "Thinking…".to_owned(),
            Some(d) => format!("Thought for {}", seconds(d)),
        },
    ));
    collapsible(&format!("reasoning-{id}-{i}"), open, header, move || text(paced(&t)).text_sm().text_color(Token::TextMuted).pl_5().into())
        .into()
}

fn seconds(d: Duration) -> String {
    let s = d.as_secs_f64();
    if s < 1.0 { "less than a second".to_owned() } else { format!("{} s", s.round() as u64) }
}

/// A tool call: its name and state; open, its input and output.
fn tool(id: u64, i: usize, p: ReadSignal<(usize, Part)>) -> View {
    let tool = move || match p.with(|p| p.1.clone()) {
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
        .child(show(working, || spinner().size_3p5().into()))
        .child(text(state_label).text_xs());
    collapsible(&format!("tool-{id}-{i}"), open, header, move || {
        let t: Option<Tool> = tool();
        let input = t.as_ref().map(|t| t.input.clone()).unwrap_or_else(|| TextBuffer::new(""));
        col()
            .gap_1()
            .pl_5()
            .text_xs()
            .child(text("Input").text_color(Token::TextMuted))
            .child(text(input).font_mono().p_2().rounded_md().bg(Token::Surface))
            .child(show(
                move || tool().and_then(|t| t.output).is_some(),
                move || {
                    col()
                        .gap_1()
                        .child(text("Output").text_color(Token::TextMuted))
                        .child(
                            text(move || tool().and_then(|t| t.output).unwrap_or_default())
                                .font_mono()
                                .p_2()
                                .rounded_md()
                                .bg(Token::Surface),
                        )
                        .into()
                },
            ))
            .into()
    })
    .into()
}

// ---- the composer ----

fn composer() -> El {
    let chat = chat();
    let current = chat.current;
    let queued = chat.queue.filter(move |q| Some(q.thread) == current.get_untracked());
    let (send, stop, attach, drop_in, busy, busy2, editing, cancel_edit) =
        (chat.clone(), chat.clone(), chat.clone(), chat.clone(), chat.clone(), chat.clone(), chat.editing, chat.clone());
    let answering = move || current.get().is_some_and(|t| busy.is_answering(t));
    let files = chat.files;
    col()
        .id("composer-area")
        .w_full()
        .max_w(760.0)
        .mx_auto()
        .px_4()
        .pb_4()
        .gap_2()
        .on_drop(move |paths| drop_in.attach(paths))
        .drag_over(|p| p.bg(Token::Surface))
        .child(each(move || queued.source(), |qid, q| queued_row(*qid, q)))
        .child(
            col()
                .gap_2()
                .p_2()
                .rounded_lg()
                .border()
                .border_color(Token::Border)
                .bg(Token::Background)
                .child(show(
                    move || !files.is_empty(),
                    move || {
                        row()
                            .gap_1()
                            .flex_wrap()
                            .child(each(
                                move || files.source(),
                                move |fid, f| {
                                    let (fid, c) = (*fid, use_context::<Chat>().expect("inside the chat"));
                                    let name = f.get_untracked().1.name;
                                    removable_chip(&name, move || c.detach(fid)).id(format!("file-{fid}")).into()
                                },
                            ))
                            .into()
                    },
                ))
                .child(
                    text_field(chat.draft)
                        .id("composer")
                        .label("Message")
                        .multiline()
                        .rows(1, 8)
                        .submit_on_enter()
                        .border_color(Paint::Color(Color::TRANSPARENT))
                        .bg(Paint::Color(Color::TRANSPARENT))
                        .on_submit(move || send.send())
                        .on_key(move |k| {
                            if k.key == Key::Escape && editing.get_untracked().is_some() {
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
                        .child(icon_button(icons::PAPERCLIP, "Attach files").id("attach").on_press(move || {
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
                            })
                            .detach();
                        }))
                        .child(spacer())
                        .child(show_else(
                            answering,
                            move || {
                                let stop = stop.clone();
                                icon_button(icons::SQUARE, "Stop")
                                    .id("stop")
                                    .on_press(move || {
                                        if let Some(t) = stop.current.get_untracked() {
                                            stop.stop(t);
                                        }
                                    })
                                    .into()
                            },
                            move || {
                                let send = busy2.clone();
                                let draft = send.draft;
                                primary_icon(
                                    icon_button(icons::ARROW_UP, "Send")
                                        .id("send")
                                        .disabled(move || draft.get().trim().is_empty())
                                        .on_press(move || send.send()),
                                )
                                .into()
                            },
                        )),
                ),
        )
}

/// A send button's look: the accent.
fn primary_icon(b: El) -> El {
    b.bg(Token::Accent).text_color(Token::AccentText).rounded_full().hover(|p| p.bg(Token::AccentHover).text_color(Token::AccentText))
}

fn queued_row(id: u64, q: ReadSignal<crate::chat::Queued>) -> View {
    let chat = chat();
    let (now, edit, remove) = (chat.clone(), chat.clone(), chat.clone());
    row()
        .id(format!("queued-{id}"))
        .items_center()
        .gap_2()
        .px_3()
        .py_1p5()
        .rounded_md()
        .bg(Token::Surface)
        .text_sm()
        .child(text("Queued").text_xs().text_color(Token::TextMuted))
        .child(text(move || q.with(|q| q.text.clone())).truncate().flex_1())
        .child(icon_button(icons::ARROW_UP, "Send now").id(format!("send-now-{id}")).on_press(move || now.send_now(id)))
        .child(icon_button(icons::PENCIL, "Edit").id(format!("edit-queued-{id}")).on_press(move || edit.edit_queued(id)))
        .child(icon_button(icons::X, "Remove").id(format!("unqueue-{id}")).on_press(move || remove.unqueue(id)))
        .into()
}
