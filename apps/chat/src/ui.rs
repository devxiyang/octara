//! Conversation navigation, virtual transcript, composer and nested panels.

use crate::chat::{Chat, Options, Queued};
use crate::model::{Author, Message, MessageId, Part, PartId, Status, Thread, ThreadId, ToolState};
use crate::rich;
use crate::settings::Generation;
use octara::prelude::*;
use octara_icons as icons;
use octara_kit::headless::{Choice, ContextMenuState, MenuState, Selection, close::close_button};
use octara_kit::parts::{button_part, editor_style};
use octara_kit::{
    ButtonVariant, ComboboxOptions, DialogOptions, Size as ControlSize, ToastOptions, ToastQueue, Tone, alert_dialog, button, callout,
    chip, choice_row, collapsible, combobox, dialog, icon_button, list, list_header, removable_chip, spinner, splitter, text_field,
    toast_viewport,
};
use octara_markdown::{
    Completion, DocumentStore, MarkdownComposer, ParseOptions, StreamingOptions, TriggerQuery, markdown, markdown_stream,
};
use std::cmp::Reverse;
use std::fmt;

const FULL: Length = rel(1.0);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Conversation,
    Conversations,
}

pub fn root(b: &mut BuildCx<'_>, options: Options) -> View {
    let chat = Chat::new(b, options);
    view(b, chat)
}

/// Builds the product UI in the same owner as its controller.
#[ui]
pub fn view(b: &mut BuildCx<'_>, chat: Chat) -> View {
    let width = b.signal(260.0_f32);
    let nav = b.navigator(Page::Conversation);
    let toasts = ToastQueue::new(b);
    let breakpoint = b.breakpoint();
    let compact = b.memo(move |r| r.get(breakpoint) == Breakpoint::Compact);
    let nav_model = b.binding(
        move |r| r.get(compact) && nav.current(r) == Page::Conversations,
        move |w, open| {
            if open {
                if w.read(|r| nav.current(r)) == Page::Conversation {
                    nav.push(w, Page::Conversations);
                }
            } else {
                nav.pop_to_root(w);
            }
        },
    );
    chat_commands(b);
    row()
        .test_id("panes")
        .tw(PANES)
        .on_command("chat.new", move |w| {
            chat.new_chat(w);
            if nav.pop_to_root(w) {
                w.request_command_after_commit("chat.focus_composer");
            }
        })
        .on_command("chat.stop", move |w| {
            if let Some(thread) = w.get(chat.current) {
                chat.stop(w, thread);
            }
        })
        .on_command("chat.focus_composer", move |w| {
            if nav.pop_to_root(w) {
                w.request_command_after_commit("chat.focus_composer");
            } else {
                w.focus("composer");
            }
        })
        .child((
            show(move |r| !r.get(compact), move |b| desktop_sidebar(b, chat, nav, width)),
            conversation(b, chat, nav_model, compact, toasts),
            compact_sidebar(chat, nav, nav_model),
        ))
        .into_view()
}

const PANES: TwStyle = tw!("size-full bg-background");
const SIDEBAR_PANE: TwStyle = tw!("h-full shrink-0");

#[ui]
fn desktop_sidebar(b: &mut BuildCx<'_>, chat: Chat, nav: Navigator<Page>, width: Signal<f32>) -> (El, El) {
    let pane_width = b.binding(move |r| r.get(width), move |w, next: f32| w.set(width, next.clamp(180.0, 420.0)));
    (sidebar(b, chat, nav).tw(SIDEBAR_PANE).style_with(move |r, p| p.w(r.get(width))), splitter(b, "panes-divider", pane_width))
}

#[ui]
fn compact_sidebar(chat: Chat, nav: Navigator<Page>, open: Binding<bool>) -> View {
    dialog("chat-listing", "Conversations", open, DialogOptions::default(), move |b| {
        let viewport = b.viewport();
        sidebar(b, chat, nav)
            .on_command("navigation.back", move |w| {
                nav.pop(w);
            })
            .style_with(move |r, p| p.h((r.get(viewport).unobscured_size().height - 128.0).max(120.0)))
    })
}

fn chat_commands(b: &mut BuildCx<'_>) {
    b.menus(|_| {
        vec![
            Menu { name: "File".into(), items: vec![menu_action("New Chat", "chat.new", "mod+n")] },
            Menu {
                name: "Chat".into(),
                items: vec![
                    menu_action("Focus Composer", "chat.focus_composer", "mod+l"),
                    menu_action("Stop Answering", "chat.stop", "mod+."),
                ],
            },
        ]
    });
    b.on_chord("mod+n".parse().unwrap(), |w| w.request_command("chat.new"));
    b.on_chord("mod+l".parse().unwrap(), |w| w.request_command("chat.focus_composer"));
    b.on_chord("mod+.".parse().unwrap(), |w| w.request_command("chat.stop"));
}

fn menu_action(label: &str, id: &str, shortcut: &str) -> MenuItem {
    MenuItem::Action { id: id.into(), label: label.into(), enabled: true, checked: None, shortcut: Some(shortcut.into()) }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
enum Age {
    Today,
    Yesterday,
    Week,
    Older,
}

impl fmt::Display for Age {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Today => "Today",
            Self::Yesterday => "Yesterday",
            Self::Week => "Previous 7 Days",
            Self::Older => "Older",
        })
    }
}

fn age(updated: u64, now: u64) -> Age {
    const DAY: u64 = 86_400;
    match (now / DAY).saturating_sub(updated / DAY) {
        0 => Age::Today,
        1 => Age::Yesterday,
        2..=7 => Age::Week,
        _ => Age::Older,
    }
}

type Threads = ReadList<FlatKey<Age, ThreadId>, FlatItem<Age, Thread>>;

fn sidebar(b: &mut BuildCx<'_>, chat: Chat, nav: Navigator<Page>) -> El {
    let now = chat.now(b);
    let sorted = chat.threads.read_only().sort_by(b, |t| Reverse((t.updated, t.id)));
    let threads = sorted.group_by(b, move |t| age(t.updated, now)).flatten();
    let requested_focus = b.signal(None);
    let scroll = b.signal(Vec2::default());
    b.watch(
        move |r| {
            let rename = r.get(chat.renaming);
            let target = r.get(requested_focus).map(|id| (id, r.get(scroll)));
            (rename, target)
        },
        move |w, (rename, target), _| match (rename, target) {
            (Some(id), _) => w.focus(&format!("rename-{id}")),
            // Revealing a virtual row can take another cycle. The scroll offset updates
            // after projection, so this retries once the row has had a chance to mount.
            (None, Some((id, _))) => w.focus(&format!("thread-{id}")),
            (None, None) => {}
        },
    );
    col()
        .test_id("sidebar")
        .bg(ColorToken::Surface)
        .child(
            row()
                .align(AlignItems::Center)
                .gap(sp(3.0))
                .h(72.0)
                .shrink(0.0)
                .px(sp(5.0))
                .child(brand(32.0))
                .child(text("octara").text_size(FontSizeToken::Xl).text_weight(600)),
        )
        .child(
            button("New conversation", ControlSize::Lg, ButtonVariant::Secondary)
                .test_id("new-chat")
                .label("New chat")
                .h(40.0)
                .shrink(0.0)
                .mx(sp(3.0))
                .mb(sp(4.0))
                .bg(ColorToken::Background)
                .on_press(|w| w.request_command("chat.new")),
        )
        .child(text("Your conversations").text_size(FontSizeToken::Xs).text_color(ColorToken::TextMuted).px(sp(5.0)).pb(sp(2.0)))
        .child(
            list()
                .test_id("threads")
                .label("Conversations")
                .grow(1.0)
                .min_h(0.0)
                .px(sp(2.0))
                .overflow_y(Overflow::Scroll)
                .observe_scroll(scroll)
                .child(virtual_each(
                    threads,
                    VirtualOptions { estimated_row_height: 36.0, ..VirtualOptions::default() },
                    move |b, key, _| match *key {
                        FlatKey::Header(age) => list_header(age.to_string()).test_id(format!("age-{age:?}")),
                        FlatKey::Item(id) => thread_row(b, chat, nav, id, threads, requested_focus),
                    },
                )),
        )
        .child(
            row()
                .align(AlignItems::Center)
                .gap(sp(3.0))
                .p(sp(4.0))
                .shrink(0.0)
                .child(icon(icons::HARD_DRIVE.into()).size(sp(4.0)).a11y_hidden())
                .child(
                    text(if chat.has_store(b) { "History saved on this device" } else { "History kept for this session" })
                        .text_size(FontSizeToken::Xs)
                        .text_color(ColorToken::TextMuted),
                ),
        )
}

fn brand(size: f32) -> El {
    el().size(size)
        .shrink(0.0)
        .align(AlignItems::Center)
        .justify(JustifyContent::Center)
        .rounded(RadiusToken::Lg)
        .bg(ColorToken::Accent)
        .child(icon(icons::SPARKLES.into()).size(sp(4.0)).text_color(ColorToken::AccentText))
        .a11y_hidden()
}

fn thread_row(
    b: &mut BuildCx<'_>,
    chat: Chat,
    nav: Navigator<Page>,
    id: ThreadId,
    threads: Threads,
    requested_focus: Signal<Option<ThreadId>>,
) -> El {
    let open = b.signal(false);
    let actions = b.list_from([("rename", Choice::new("Rename")), ("delete", Choice::new("Delete…"))]);
    let menu = MenuState::new(b, open, actions);
    let context_menu = ContextMenuState::new(b, menu);
    let menu_group = LayerGroupId::new();
    let confirm = b.signal(false);
    let thread = b.row_signal(chat.threads, id);
    let title = b.memo(move |r| r.try_with(thread, |thread| thread.title.clone()).unwrap_or_default());
    row()
        .gap(sp(1.0))
        .align(AlignItems::Center)
        .min_w(0.0)
        .min_h(34.0)
        .child(keyed(
            move |r| r.get(chat.renaming) == Some(id),
            move |b, editing| {
                if *editing {
                    let initial = b.sample(title);
                    let draft = b.signal(initial);
                    text_field(draft.into())
                        .test_id(format!("rename-{id}"))
                        .label("Conversation name")
                        .submit_on_enter()
                        .grow(1.0)
                        .min_w(0.0)
                        .on_submit(move |w| {
                            let title = w.get(draft);
                            chat.rename(w, id, &title);
                        })
                        .on_blur(move |w| {
                            if w.get(chat.renaming) == Some(id) {
                                let title = w.get(draft);
                                chat.rename(w, id, &title);
                            }
                        })
                        .on_key(move |w, key| {
                            if key.key != Key::Escape {
                                return false;
                            }
                            w.set(chat.renaming, None);
                            true
                        })
                        .into_view()
                } else {
                    let trigger = button(title, ControlSize::Md, ButtonVariant::Ghost)
                        .test_id(format!("thread-{id}"))
                        .selected(move |r: &mut ReadCx<'_>| chat.active_thread(r) == Some(id))
                        .justify(JustifyContent::Start)
                        .grow(1.0)
                        .min_w(0.0)
                        .h(34.0)
                        .max_lines(1)
                        .overflow_x(Overflow::Hidden)
                        .style_with(move |r, p| if chat.active_thread(r) == Some(id) { p.bg(ColorToken::Background) } else { p })
                        .on_focus(move |w| {
                            if w.get(requested_focus) == Some(id) {
                                w.set(requested_focus, None);
                            }
                        })
                        .on_press(move |w| {
                            chat.open(w, id);
                            nav.pop_to_root(w);
                            w.request_command_after_commit("chat.focus_composer");
                        })
                        .on_key(move |w, key| {
                            if !key.modifiers.is_empty() {
                                return false;
                            }
                            let mut threads = w.keys(threads).into_iter().filter_map(|key| match key {
                                FlatKey::Item(id) => Some(id),
                                FlatKey::Header(_) => None,
                            });
                            let next = match key.key {
                                Key::Home => threads.next(),
                                Key::End => threads.next_back(),
                                Key::Up => threads.rev().skip_while(|next| *next != id).nth(1),
                                Key::Down => threads.skip_while(|next| *next != id).nth(1),
                                _ => return false,
                            };
                            let Some(next) = next else { return false };
                            w.reveal_row("threads", FlatKey::<Age, _>::Item(next));
                            if next == id {
                                w.focus(&format!("thread-{id}"));
                            } else {
                                w.set(requested_focus, Some(next));
                            }
                            true
                        });
                    context_menu
                        .trigger(trigger, Placement::BELOW.fit(true), None, move |_| {
                            menu.items(
                                col()
                                    .test_id(format!("thread-menu-{id}-popover"))
                                    .layer_group(menu_group)
                                    .gap(sp(1.0))
                                    .p(sp(2.0))
                                    .rounded(RadiusToken::Md)
                                    .border(1.0)
                                    .border_color(ColorToken::Border)
                                    .bg(ColorToken::Surface)
                                    .shadow(ShadowToken::Lg)
                                    .child(
                                        menu.item(
                                            button("Rename", ControlSize::Md, ButtonVariant::Ghost)
                                                .test_id(format!("thread-rename-{id}"))
                                                .on_press(move |w| w.set(chat.renaming, Some(id))),
                                            &"rename",
                                        ),
                                    )
                                    .child(
                                        menu.item(
                                            button("Delete…", ControlSize::Md, ButtonVariant::Ghost)
                                                .test_id(format!("thread-delete-{id}"))
                                                .on_press(move |w| w.set(confirm, true)),
                                            &"delete",
                                        ),
                                    ),
                            )
                        })
                        .into_view()
                }
            },
        ))
        .child(show(move |r| chat.is_answering(r, id), spinner))
        .child(
            icon_button(icons::ELLIPSIS, "Conversation actions")
                .test_id(format!("thread-menu-{id}"))
                .layer_group(menu_group)
                .expanded(open)
                .on_press(move |w| {
                    let next = !w.get(open);
                    menu.set_open(w, next);
                }),
        )
        .child(alert_dialog(
            &format!("delete-thread-{id}"),
            "Delete conversation?",
            confirm.into(),
            "The conversation and its messages will be deleted.",
            move |_| {
                col().gap(sp(2.0)).child(text(title)).child(
                    row()
                        .gap(sp(2.0))
                        .justify(JustifyContent::End)
                        .child(close_button(button("Cancel", ControlSize::Md, ButtonVariant::Secondary).autofocus()))
                        .child(close_button(
                            button("Delete", ControlSize::Md, ButtonVariant::Destructive)
                                .test_id(format!("confirm-delete-{id}"))
                                .on_press(move |w| chat.delete(w, id)),
                        )),
                )
            },
        ))
}

const CONVERSATION: TwStyle = tw!("h-full grow basis-[0px] min-w-[0px]");
const CONVERSATION_HEADER: TwStyle = tw!("items-center gap-2 h-[64px] shrink-0 px-4");
const CONVERSATION_TITLE: TwStyle = tw!("font-medium text-sm grow min-w-[0px]");
const CONVERSATION_BODY: TwStyle = tw!("relative grow basis-[0px] min-h-[0px] min-w-[0px]");

#[ui]
fn conversation(b: &mut BuildCx<'_>, chat: Chat, listing: Binding<bool>, compact: ReadSignal<bool>, toasts: ToastQueue<String>) -> El {
    let settings_open = b.signal(false);
    let choices = chat.model_choices(b);
    let no_models = choices.is_empty();
    let choices = b.list_from(choices.into_iter().map(|(id, label)| (id, Choice::new(label)))).read_only();
    col().test_id("conversation").tw(CONVERSATION).child((
        conversation_header(chat, listing, compact, settings_open, choices, no_models),
        el().tw(CONVERSATION_BODY).child((
            keyed(move |r| chat.active_thread(r).unwrap_or(0), move |b, thread| transcript(b, chat, *thread, compact, toasts)),
            toast_viewport("chat-toasts", toasts).top(sp(4.0)).bottom(Length::Auto),
        )),
        composer(b, chat, no_models),
        settings_dialog(chat, settings_open, choices),
    ))
}

#[ui]
fn conversation_header(
    chat: Chat,
    listing: Binding<bool>,
    compact: ReadSignal<bool>,
    settings_open: Signal<bool>,
    choices: ReadList<String, Choice>,
    no_models: bool,
) -> El {
    row().test_id("conversation-header").tw(CONVERSATION_HEADER).child((
        show(
            move |r| r.get(compact),
            move |_| icon_button(icons::MENU, "Conversations").test_id("conversations").on_press(move |w| w.set(listing, true)),
        ),
        text(move |r: &mut ReadCx<'_>| {
            chat.active_thread(r)
                .and_then(|id| r.with_row(chat.threads, &id, |t| t.map(|t| t.title.clone())))
                .unwrap_or_else(|| "New chat".into())
        })
        .test_id("title")
        .role(Role::Heading)
        .level(1)
        .max_lines(1)
        .tw(CONVERSATION_TITLE),
        model_picker(chat, compact, choices, no_models),
        icon_button(icons::SETTINGS, "Settings").test_id("settings-open").on_press(move |w| w.set(settings_open, true)),
    ))
}

#[ui]
fn model_picker(chat: Chat, compact: ReadSignal<bool>, choices: ReadList<String, Choice>, no_models: bool) -> View {
    combobox(
        "model",
        "Model",
        Selection::Single(chat.model.into()),
        choices,
        ComboboxOptions::default(),
        move |caption| {
            let caption = move |r: &mut ReadCx<'_>| if no_models { "No models".into() } else { r.get(caption) };
            button(caption, ControlSize::Md, ButtonVariant::Secondary)
                .style_with(move |r, p| p.w(if r.get(compact) { 124.0 } else { 206.0 }))
                .min_w(0.0)
                .max_lines(1)
                .overflow_x(Overflow::Hidden)
                .disabled(no_models)
                .tooltip("Applies to the next message or regenerated answer.")
        },
        choice_row,
    )
}

fn settings_dialog(chat: Chat, open: Signal<bool>, choices: ReadList<String, Choice>) -> View {
    dialog(
        "settings",
        "Conversation settings",
        open.into(),
        DialogOptions { description: "Choose how your next answer is generated.".into(), click_outside: false },
        move |b| {
            let close = b.close_handle().expect("settings dialog close handle");
            let initial = b.sample(chat.generation);
            let selection = b.sample(chat.model);
            let model = b.signal(selection);
            let temperature = b.signal(initial.temperature.map_or_else(String::new, |n| n.to_string()));
            let tokens = b.signal(initial.max_completion_tokens.map_or_else(String::new, |n| n.to_string()));
            let error = b.signal(String::new());
            let default_model = b.read(|r| r.row(choices, 0));
            let valid = b.memo(move |r| {
                r.get(model).is_some_and(|id| r.with_row(choices, &id, |choice| choice.is_some()))
                    && r.with2(temperature, tokens, |t, n| Generation::parse(t, n).is_ok())
            });
            col()
                .gap(sp(3.0))
                .child(text("Model").text_weight(500))
                .child(combobox(
                    "settings-model",
                    "Model",
                    Selection::Single(model.into()),
                    choices,
                    ComboboxOptions::default(),
                    |caption| button(caption, ControlSize::Md, ButtonVariant::Secondary).w(FULL).min_w(0.0).max_lines(1),
                    choice_row,
                ))
                .child(text("Temperature (0–2)").text_weight(500))
                .child(
                    text_field(temperature.into())
                        .test_id("settings-temperature")
                        .label("Temperature")
                        .invalid(move |r: &mut ReadCx<'_>| r.with(temperature, |t| Generation::parse(t, "").is_err()))
                        .w(FULL),
                )
                .child(text("Maximum output tokens (1–131072)").text_weight(500))
                .child(
                    text_field(tokens.into())
                        .test_id("settings-tokens")
                        .label("Maximum output tokens")
                        .invalid(move |r: &mut ReadCx<'_>| r.with(tokens, |n| Generation::parse("", n).is_err()))
                        .w(FULL),
                )
                .child(
                    text("Leave either field blank to use the model’s default.")
                        .text_size(FontSizeToken::Sm)
                        .text_color(ColorToken::TextMuted),
                )
                .child(
                    text(move |r: &mut ReadCx<'_>| {
                        r.with2(temperature, tokens, |t, n| Generation::parse(t, n))
                            .err()
                            .unwrap_or_else(|| if r.get(valid) { r.get(error) } else { "Choose an available model.".into() })
                    })
                    .test_id("settings-error")
                    .role(Role::Status)
                    .live(Live::Polite)
                    .text_size(FontSizeToken::Sm)
                    .text_color(ColorToken::Danger),
                )
                .child(
                    row()
                        .gap(sp(2.0))
                        .wrap(FlexWrap::Wrap)
                        .child(button("Restore defaults", ControlSize::Md, ButtonVariant::Ghost).test_id("settings-reset").on_press(
                            move |w| {
                                w.set(model, default_model.clone());
                                w.set(temperature, String::new());
                                w.set(tokens, String::new());
                                w.set(error, String::new());
                            },
                        ))
                        .child(spacer().grow(1.0))
                        .child(close_button(button("Cancel", ControlSize::Md, ButtonVariant::Secondary).test_id("settings-cancel")))
                        .child(
                            button("Save", ControlSize::Md, ButtonVariant::Primary)
                                .test_id("settings-save")
                                .disabled(move |r: &mut ReadCx<'_>| !r.get(valid))
                                .on_press(move |w| {
                                    let selected = w.get(model);
                                    let t = w.get(temperature);
                                    let n = w.get(tokens);
                                    match Generation::parse(&t, &n).and_then(|generation| chat.configure(w, selected, generation)) {
                                        Ok(()) => close.close(w),
                                        Err(message) => w.set(error, message),
                                    }
                                }),
                        ),
                )
        },
    )
}

fn transcript(b: &mut BuildCx<'_>, chat: Chat, thread: ThreadId, compact: ReadSignal<bool>, toasts: ToastQueue<String>) -> El {
    let lightbox = rich::transcript(b, compact);
    let messages = chat.messages.read_only().filter(b, move |m| m.thread == thread);
    col()
        .size(FULL)
        .min_h(0.0)
        .min_w(0.0)
        .child(
            col()
                .test_id("transcript")
                .label("Transcript")
                .w(FULL)
                .grow(1.0)
                .basis(0.0)
                .min_h(0.0)
                .overflow_y(Overflow::Scroll)
                .follow_tail()
                .selectable(true)
                .child(virtual_each_with_copy(
                    messages,
                    VirtualOptions { estimated_row_height: 248.0, ..VirtualOptions::default() },
                    move |b, id, message| message_row(b, chat, *id, message, toasts),
                    |_, message| transcript_text(message),
                ))
                .child(show(move |r| r.len(messages) == 0, move |_| welcome(chat.draft, compact))),
        )
        .child(lightbox)
        .child(show(
            move |r| r.len(messages) > 0,
            move |_| {
                icon_button(icons::ARROW_DOWN, "Jump to latest")
                    .test_id("jump")
                    .align_self(AlignItems::Center)
                    .preserve_focus_on_press()
                    .on_press(|w| w.scroll_to_end("transcript"))
            },
        ))
}

fn welcome(draft: Signal<String>, compact: ReadSignal<bool>) -> El {
    col()
        .w(FULL)
        .max_w(760.0)
        .align_self(AlignItems::Center)
        .grow(1.0)
        .align(AlignItems::Center)
        .justify(JustifyContent::Center)
        .gap(sp(4.0))
        .p(sp(5.0))
        .child(brand(48.0))
        .child(text("What would you like to explore?").text_size(FontSizeToken::Xl2).text_weight(500))
        .child(show(
            move |r| !r.get(compact),
            move |_| {
                row()
                    .w(FULL)
                    .gap(sp(2.0))
                    .wrap(FlexWrap::Wrap)
                    .child(starter(
                        draft,
                        "starter-write",
                        icons::PENCIL,
                        "Help me write",
                        "Help me write a thoughtful, concise introduction. Ask me what it is for first.",
                    ))
                    .child(starter(
                        draft,
                        "starter-learn",
                        icons::BOOK_OPEN,
                        "Explain a concept",
                        "Explain a complex idea in simple terms. Ask me which topic I want to understand.",
                    ))
                    .child(starter(
                        draft,
                        "starter-build",
                        icons::CODE,
                        "Explore an idea",
                        "Help me turn an idea into a small, practical project. Ask me about my idea first.",
                    ))
                    .child(starter(draft, "starter-transit", icons::TELESCOPE, "Explore exoplanets", rich::TRANSIT_PROMPT))
            },
        ))
}

fn starter(draft: Signal<String>, id: &str, symbol: icons::LucideIcon, title: &str, prompt: &'static str) -> El {
    button_part(col(), ControlSize::Lg, ButtonVariant::Secondary)
        .test_id(id)
        .label(title)
        .grow(1.0)
        .min_w(140.0)
        .p(sp(4.0))
        .gap(sp(2.0))
        .align(AlignItems::Start)
        .rounded(RadiusToken::Lg)
        .bg(ColorToken::Surface)
        .child(icon(symbol.into()).size(sp(4.0)).text_color(ColorToken::Accent).a11y_hidden())
        .child(text(title).text_size(FontSizeToken::Sm))
        .on_press(move |w| {
            w.set(draft, prompt.into());
            w.focus("composer");
        })
}

#[derive(Clone, Copy, PartialEq)]
enum PartKind {
    Text,
    Reasoning,
    Tool,
    File,
}

impl From<&Part> for PartKind {
    fn from(part: &Part) -> Self {
        match part {
            Part::Text(_) => Self::Text,
            Part::Reasoning { .. } => Self::Reasoning,
            Part::Tool(_) => Self::Tool,
            Part::File(_) => Self::File,
        }
    }
}

impl fmt::Display for PartKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Text => "text",
            Self::Reasoning => "reasoning",
            Self::Tool => "tool",
            Self::File => "file",
        })
    }
}

type MessageRow = RowSignal<Message>;

fn message_row(b: &mut BuildCx<'_>, chat: Chat, id: MessageId, message: MessageRow, toasts: ToastQueue<String>) -> El {
    let user = b.read(|r| r.with(message, |m| m.author == Author::User));
    let status = b.memo(move |r| r.try_with(message, |m| m.status.clone()).unwrap_or(Status::Done));
    let parts = b.derive_list(move |r| {
        r.try_with(message, |m| m.parts.iter().map(|(id, part)| (*id, PartKind::from(part))).collect()).unwrap_or_default()
    });
    col()
        .test_id(format!("msg-{id}"))
        .role(Role::Group)
        .label(if user { "Your message" } else { "Assistant" })
        .w(FULL)
        .max_w(800.0)
        .align_self(AlignItems::Center)
        .px(sp(4.0))
        .py(sp(2.5))
        .gap(sp(1.0))
        .child(
            col()
                .w(if user { rel(0.82) } else { FULL })
                .min_w(0.0)
                .align_self(if user { AlignItems::End } else { AlignItems::Stretch })
                .style_with(move |_, p| if user { p.bg(ColorToken::Surface).rounded(RadiusToken::Lg).p(sp(3.0)) } else { p })
                .gap(sp(2.0))
                .child(each(parts, move |_, part_id, kind| {
                    let part_id = *part_id;
                    el().test_id(format!("part-{id}-{part_id}")).w(FULL).min_w(0.0).child(keyed(
                        move |r| r.try_get(kind).unwrap_or(PartKind::Text),
                        move |b, kind| render_part(b, id, part_id, message, *kind, user),
                    ))
                })),
        )
        .child(if user {
            row()
                .justify(JustifyContent::End)
                .select_none()
                .child(copy_message(chat, id, "Copy message", toasts))
                .child(icon_button(icons::PENCIL, "Edit message").test_id(format!("edit-{id}")).on_press(move |w| chat.edit(w, id)))
                .into_view()
        } else {
            answer_actions(chat, id, status, toasts)
        })
}

fn transcript_text(message: &Message) -> String {
    if message.author == Author::User {
        return message.text();
    }
    let mut doc = DocumentStore::new(true, ParseOptions::default());
    message
        .parts
        .iter()
        .filter_map(|(_, part)| {
            let Part::Text(text) = part else { return None };
            doc.update(text);
            let text = doc.row_text().text;
            (!text.is_empty()).then_some(text)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn copy_message(chat: Chat, id: MessageId, label: &str, toasts: ToastQueue<String>) -> El {
    icon_button(icons::COPY, label).test_id(format!("copy-{id}")).on_press(move |w| {
        if let Some(text) = w.with_row(chat.messages, &id, |m| m.map(Message::text)) {
            w.request_service(ServiceRequest::WriteClipboardText { text }, move |w, outcome| {
                let message =
                    if outcome == ServiceOutcome::ClipboardWritten { "Copied to clipboard" } else { "Couldn’t copy to clipboard" };
                toasts.push(w, message.into(), ToastOptions::default());
            });
        }
    })
}

fn answer_actions(chat: Chat, id: MessageId, status: ReadSignal<Status>, toasts: ToastQueue<String>) -> View {
    keyed(
        move |r| match r.get(status) {
            Status::Streaming => "streaming",
            Status::Failed(_) => "failed",
            Status::Stopped => "stopped",
            Status::Done => "done",
        },
        move |b, state| {
            if *state == "streaming" {
                return row()
                    .align(AlignItems::Center)
                    .gap(sp(2.0))
                    .test_id(format!("streaming-{id}"))
                    .child(spinner(b))
                    .child(text("Answering…").text_size(FontSizeToken::Sm).text_color(ColorToken::TextMuted));
            }
            let mut actions = col().gap(sp(1.0)).select_none();
            if *state == "failed" {
                actions = actions.child(
                    callout(
                        Tone::Warning,
                        move |r: &mut ReadCx<'_>| match r.get(status) {
                            Status::Failed(reason) => reason,
                            _ => String::new(),
                        },
                        None,
                    )
                    .test_id(format!("failure-{id}")),
                );
            }
            if *state == "stopped" {
                actions = actions
                    .child(text("Stopped").test_id(format!("stopped-{id}")).text_size(FontSizeToken::Xs).text_color(ColorToken::TextMuted));
            }
            actions.child(
                row().gap(sp(1.0)).child(copy_message(chat, id, "Copy answer", toasts)).child(
                    icon_button(icons::REFRESH_CW, if *state == "failed" { "Retry" } else { "Regenerate" })
                        .test_id(format!("regenerate-{id}"))
                        .disabled(move |r: &mut ReadCx<'_>| !chat.has_model(r))
                        .on_press(move |w| chat.regenerate(w, id)),
                ),
            )
        },
    )
}

fn part_value<R>(r: &mut ReadCx<'_>, row: MessageRow, id: PartId, f: impl Fn(Option<&Part>) -> R) -> R {
    r.try_with(row, |m| f(m.parts.iter().find(|(part_id, _)| *part_id == id).map(|(_, part)| part))).unwrap_or_else(|| f(None))
}

fn render_part(b: &mut BuildCx<'_>, id: MessageId, part: PartId, message: MessageRow, kind: PartKind, user: bool) -> El {
    match kind {
        PartKind::Text => {
            let source = move |r: &mut ReadCx<'_>| {
                part_value(r, message, part, |p| match p {
                    Some(Part::Text(text)) => text.clone(),
                    _ => String::new(),
                })
            };
            if user {
                markdown(b, source).test_id(format!("text-{id}-{part}"))
            } else {
                let source = stream_text(b, source);
                let finished = b.memo(move |r| {
                    r.try_with(message, |m| m.status != Status::Streaming || m.parts.last().is_some_and(|(last, _)| *last != part))
                        .unwrap_or(true)
                });
                // Chat coalesces provider deltas before they reach the transcript.
                markdown_stream(
                    b,
                    source,
                    finished,
                    StreamingOptions { effects: Some(rich::REVEAL), pace: false, ..StreamingOptions::default() },
                )
                .test_id(format!("text-{id}-{part}"))
            }
        }
        PartKind::Reasoning => {
            let open = b.signal(false);
            collapsible(
                b,
                &format!("reasoning-{id}-{part}"),
                open.into(),
                text(move |r: &mut ReadCx<'_>| {
                    part_value(r, message, part, |p| match p {
                        Some(Part::Reasoning { started, ended: Some(end), .. }) => {
                            format!("Thought for {:.1}s", end.saturating_sub(*started).as_secs_f64())
                        }
                        _ => "Thinking…".into(),
                    })
                }),
                move |b| {
                    let source = stream_text(b, move |r| {
                        part_value(r, message, part, |p| match p {
                            Some(Part::Reasoning { text, .. }) => text.clone(),
                            _ => String::new(),
                        })
                    });
                    markdown(b, source)
                        .test_id(format!("reasoning-text-{id}-{part}"))
                        .theme_patch(ThemePatch::default().color(ColorToken::Text, ColorToken::TextMuted))
                },
            )
        }
        PartKind::Tool => {
            let open = b.signal(false);
            collapsible(
                b,
                &format!("tool-{id}-{part}"),
                open.into(),
                text(move |r: &mut ReadCx<'_>| {
                    part_value(r, message, part, |p| match p {
                        Some(Part::Tool(t)) => {
                            let status = match t.state {
                                ToolState::Streaming => "Receiving input",
                                ToolState::Running => "Running",
                                ToolState::Done => "Done",
                                ToolState::Failed(_) => "Failed",
                            };
                            format!("{} · {status}", t.name)
                        }
                        _ => String::new(),
                    })
                }),
                move |b| {
                    col()
                        .p(sp(2.0))
                        .gap(sp(2.0))
                        .bg(ColorToken::Surface)
                        .rounded(RadiusToken::Md)
                        .child(
                            text(stream_text(b, move |r| {
                                part_value(r, message, part, |p| match p {
                                    Some(Part::Tool(t)) => t.input.clone(),
                                    _ => String::new(),
                                })
                            }))
                            .test_id(format!("tool-input-{id}-{part}")),
                        )
                        .child(
                            text(move |r: &mut ReadCx<'_>| {
                                part_value(r, message, part, |p| match p {
                                    Some(Part::Tool(t)) => match &t.state {
                                        ToolState::Failed(error) => error.clone(),
                                        _ => t.output.clone().unwrap_or_default(),
                                    },
                                    _ => String::new(),
                                })
                            })
                            .test_id(format!("tool-output-{id}-{part}")),
                        )
                },
            )
        }
        PartKind::File => chip(move |r: &mut ReadCx<'_>| {
            part_value(r, message, part, |p| match p {
                Some(Part::File(file)) => file.name.clone(),
                _ => String::new(),
            })
        }),
    }
}

/// The row owns the render buffer. Its authoritative text remains available while unmounted.
fn stream_text(b: &mut BuildCx<'_>, source: impl Fn(&mut ReadCx<'_>) -> String + 'static) -> TextBuffer {
    let value = b.memo(source);
    let initial = b.sample(value);
    let buffer = b.text_buffer(initial);
    b.watch(
        move |r| r.get(value),
        move |w, next, previous| {
            let Some(previous) = previous else { return };
            if let Some(suffix) = next.strip_prefix(previous) {
                w.append_text(buffer, suffix);
            } else {
                w.splice_text(buffer, TextBufferEdit::Splice { range: 0..previous.len(), insert: next.clone() });
            }
        },
    );
    buffer
}

const COMPOSER_AREA: TwStyle = tw!("w-full max-w-[800px] self-center shrink-0 px-4 pb-4 gap-2");
const COMPOSER_PANEL: TwStyle = tw!("gap-2 p-3 rounded-2xl border border-border bg-background");
const COMPOSER_TOOLBAR: TwStyle = tw!("items-center gap-2");
const SEND_BUTTON: TwStyle = tw!("bg-accent text-accent-text hover:bg-accent-hover hover:text-accent-text rounded-full");

#[ui]
fn composer(b: &mut BuildCx<'_>, chat: Chat, no_models: bool) -> El {
    let composer = MarkdownComposer::bind(b, chat.draft);
    let input = editor_style(textarea())
        .test_id("composer")
        .label("Message")
        .placeholder("Ask anything…")
        .w(FULL)
        .text_size(FontSizeToken::Base)
        .rows(1, 8)
        .submit_on_enter()
        .on_submit(move |w| chat.send(w))
        .on_key(move |w, key| {
            if key.key != Key::Escape || w.get(chat.editing).is_none() {
                return false;
            }
            chat.cancel_edit(w);
            true
        });
    let input = composer.editor_with_completion(
        b,
        input,
        |_, query| chat_completions(query),
        move |w, completion| match completion.action_key() {
            Some("transit") => {
                w.set(chat.draft, rich::TRANSIT_PROMPT.into());
                w.focus("composer");
            }
            Some("attach") => attach_files(chat, w),
            _ => {}
        },
    );
    col().test_id("composer-area").on_file_drop(move |w, files| chat.attach_dropped(w, files)).tw(COMPOSER_AREA).child((
        keyed(move |r| chat.active_thread(r).unwrap_or(0), move |b, thread| queued_messages(b, chat, *thread)),
        show(
            move |r| !chat.has_model(r),
            move |_| {
                text(if no_models { "No models available. Your draft is kept." } else { "Choose a model to send. Your draft is kept." })
                    .test_id("model-status")
                    .role(Role::Status)
                    .live(Live::Polite)
                    .tw(tw!("text-xs text-muted"))
            },
        ),
        col().tw(COMPOSER_PANEL).child((
            show(move |r| r.get(chat.editing).is_some(), move |_| edit_banner(chat)),
            show(move |r| r.len(chat.files) > 0, move |_| attachments(chat)),
            input,
            composer_toolbar(chat),
        )),
    ))
}

const QUEUED_MESSAGES: TwStyle = tw!("gap-2 max-h-[168px] overflow-y-scroll");
const ATTACHMENTS: TwStyle = tw!("gap-1 flex-wrap");

#[ui]
fn queued_messages(b: &mut BuildCx<'_>, chat: Chat, thread: ThreadId) -> El {
    let queue = chat.queue.read_only().filter(b, move |q| q.thread == thread);
    col().tw(QUEUED_MESSAGES).child(each(queue, move |b, id, q| queued_row(b, chat, *id, q)))
}

#[ui]
fn edit_banner(chat: Chat) -> El {
    row().tw(tw!("items-center")).child((
        text("Editing message").tw(tw!("grow text-sm")),
        button("Cancel edit", ControlSize::Sm, ButtonVariant::Ghost).test_id("cancel-edit").on_press(move |w| chat.cancel_edit(w)),
    ))
}

#[ui]
fn attachments(chat: Chat) -> El {
    row().tw(ATTACHMENTS).child(each(chat.files, move |b, id, file| {
        let id = *id;
        let name = b.read(|r| r.with(file, |file| file.name.clone()));
        removable_chip(b, &name, |remove| remove.test_id(format!("remove-file-{id}")).on_press(move |w| chat.detach(w, id)))
            .test_id(format!("file-{id}"))
    }))
}

#[ui]
fn composer_toolbar(chat: Chat) -> El {
    row().tw(COMPOSER_TOOLBAR).child((
        icon_button(icons::PAPERCLIP, "Attach files")
            .test_id("attach")
            .tooltip("Choose files or drop them on the composer. File contents stay on this device.")
            .on_press(move |w| attach_files(chat, w)),
        text("@ mentions · / commands · Enter to send").tw(tw!("text-xs text-muted")),
        spacer().grow(1.0),
        show(
            move |r| chat.active_thread(r).is_some_and(|id| chat.is_answering(r, id)),
            move |_| {
                icon_button(icons::SQUARE, "Stop").test_id("stop").on_press(move |w| {
                    if let Some(thread) = w.get(chat.current) {
                        chat.stop(w, thread);
                    }
                })
            },
        ),
        icon_button(icons::ARROW_UP, "Send")
            .test_id("send")
            .tw(SEND_BUTTON)
            .disabled(move |r: &mut ReadCx<'_>| !chat.can_send(r))
            .on_press(move |w| chat.send(w)),
    ))
}

fn chat_completions(query: &TriggerQuery) -> Vec<Completion> {
    let needle = query.query.to_lowercase();
    let matches = |label: &str| label.to_lowercase().starts_with(&needle);
    match query.trigger {
        '@' => [("Mira", "user://mira"), ("Rowan", "user://rowan")]
            .into_iter()
            .filter(|(name, _)| matches(name))
            .map(|(name, url)| Completion::new(format!("@{name}"), url))
            .collect(),
        '/' => [("transit", "/transit · Draft the exoplanet question"), ("attach", "/attach · Choose files")]
            .into_iter()
            .filter(|(command, _)| matches(command))
            .map(|(key, label)| Completion::action(label, key))
            .collect(),
        _ => Vec::new(),
    }
}

fn attach_files(chat: Chat, w: &mut WriteCx<'_>) {
    w.call_module(octara_picker::PickFiles { title: Some("Attach".into()), multiple: true, ..Default::default() }, move |w, result| {
        if let Ok(files) = result {
            for file in files {
                chat.attach_file(w, file.name, file.path);
            }
        }
    });
}

fn queued_row(b: &mut BuildCx<'_>, chat: Chat, id: u64, queued: RowSignal<Queued>) -> El {
    let choices = chat.model_choices(b);
    let model =
        b.memo(move |r| r.try_with(queued, |q| choices.iter().find(|(id, _)| *id == q.model_id).map(|(_, name)| name.clone())).flatten());
    row()
        .test_id(format!("queued-{id}"))
        .align(AlignItems::Center)
        .gap(sp(2.0))
        .p(sp(2.0))
        .bg(ColorToken::Surface)
        .rounded(RadiusToken::Md)
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .child(
                    text(move |r: &mut ReadCx<'_>| format!("Queued · {}", r.get(model).as_deref().unwrap_or("Model unavailable")))
                        .test_id(format!("queued-model-{id}"))
                        .text_size(FontSizeToken::Xs)
                        .text_color(ColorToken::TextMuted)
                        .max_lines(1),
                )
                .child(text(move |r: &mut ReadCx<'_>| r.try_with(queued, |q| q.text.clone()).unwrap_or_default()).max_lines(1)),
        )
        .child(
            icon_button(icons::ARROW_UP, "Send now")
                .test_id(format!("send-now-{id}"))
                .disabled(move |r: &mut ReadCx<'_>| r.get(model).is_none())
                .on_press(move |w| chat.send_now(w, id)),
        )
        .child(
            icon_button(icons::PENCIL, "Edit queued message")
                .test_id(format!("edit-queued-{id}"))
                .on_press(move |w| chat.edit_queued(w, id)),
        )
        .child(icon_button(icons::X, "Remove queued message").test_id(format!("unqueue-{id}")).on_press(move |w| chat.unqueue(w, id)))
}
