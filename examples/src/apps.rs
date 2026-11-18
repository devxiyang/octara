//! Example apps ([running the examples](../README.md)).

use octara::app::Window;
use octara::prelude::*;
use octara::reactive::list::{FlatItem, FlatKey};
use octara_kit::{button, checkbox, ghost_button, primary_button, switch, text_field, toggle_button};

#[ui]
pub fn counter() -> View {
    let count = state("count", || 0i64);
    let double = memo(move || count.get() * 2);
    col()
        .child(text(move || format!("Count: {}", count.get())).id("count"))
        .child(text(move || format!("Double: {}", double.get())).id("double"))
        .child(
            row()
                .child(button("+1").id("inc").on_press(move || count.update(|n| *n += 1)))
                .child(button("Reset").id("reset").on_press(move || count.set(0))),
        )
        .size_full()
        .into()
}

/// Declaration-order and patch replacement checks in a real retained window.
#[ui]
pub fn styles() -> View {
    let early = state("early", || 64);
    let late = state("late", || 96);
    let patch = state("patch", || true);
    col()
        .size_full()
        .p_4()
        .gap_2()
        .child(text("Style declaration order"))
        .child(text("Later static width; wider on medium screens"))
        .child(el().id("static-last").h(24).bg(Token::Accent).style_with(move |p| p.w(early.get())).w(80).when_medium(|p| p.w(120)))
        .child(text("Later dynamic width"))
        .child(el().id("dynamic-last").h(24).bg(Token::Accent).style_with(move |p| p.w(early.get())).style_with(move |p| p.w(late.get())))
        .child(text("Optional patch; removal restores the earlier width"))
        .child(
            el().id("patch")
                .h(24)
                .bg(Token::Accent)
                .style_with(move |p| p.w(early.get()))
                .style_with(move |p| if patch.get() { p.w(160).bg(Token::Danger) } else { p }),
        )
        .child(
            button("Change earlier width")
                .id("early")
                .self_start()
                .on_press(move || early.update(|v| *v = if *v == 64 { 112 } else { 64 })),
        )
        .child(button("Change later width").id("late").self_start().on_press(move || late.update(|v| *v = if *v == 96 { 144 } else { 96 })))
        .child(button("Toggle patch").id("toggle-patch").self_start().on_press(move || patch.update(|v| *v = !*v)))
        .into()
}

/// A small pointer playground used by the real-window Inspector smoke check.
#[ui]
pub fn gestures() -> View {
    let presses = state("presses", || 0u32);
    let doubles = state("doubles", || 0u32);
    let short = state("short", || 0u32);
    let long = state("long", || 0u32);
    let vanished = state("vanished", || false);
    let vanished_doubles = state("vanished_doubles", || 0u32);
    let volatile = state("volatile", || true);
    let volatile_longs = state("volatile_longs", || 0u32);
    col()
        .id("gesture-root")
        .size_full()
        .p_4()
        .gap_2()
        .child(text(move || format!("Presses: {}", presses.get())).id("presses"))
        .child(text(move || format!("Doubles: {}", doubles.get())).id("doubles"))
        .child(text(move || format!("Short: {}", short.get())).id("short"))
        .child(text(move || format!("Long: {}", long.get())).id("long"))
        .child(
            button("Double click")
                .id("double-target")
                .self_start()
                .on_press(move || presses.update(|n| *n += 1))
                .on_double_press(move || doubles.update(|n| *n += 1)),
        )
        .child(
            button("Hold")
                .id("hold-target")
                .self_start()
                .on_press(move || short.update(|n| *n += 1))
                .on_long_press(move || long.update(|n| *n += 1)),
        )
        .child(text(move || format!("Removed doubles: {}", vanished_doubles.get())).id("vanished-doubles"))
        .child(show(
            move || !vanished.get(),
            move || {
                button("Remove on first click")
                    .id("vanishing-double")
                    .self_start()
                    .on_press(move || vanished.set(true))
                    .on_double_press(move || vanished_doubles.update(|n| *n += 1))
            },
        ))
        .child(text(move || format!("Removed long presses: {}", volatile_longs.get())).id("volatile-longs"))
        .child(button("Remove hold target soon").id("arm-remove").self_start().on_press(move || {
            spawn(async move {
                sleep(std::time::Duration::from_millis(150)).await;
                volatile.set(false);
            });
        }))
        .child(show(
            move || volatile.get(),
            move || {
                button("Hold while being removed")
                    .id("volatile-hold")
                    .self_start()
                    .on_long_press(move || volatile_longs.update(|n| *n += 1))
            },
        ))
        .into()
}

#[derive(Clone, PartialEq, Debug)]
pub struct Todo {
    pub id: u32,
    pub title: String,
    pub done: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Show {
    All,
    Active,
    Done,
}

#[ui]
pub fn todo(n: usize) -> View {
    let todos =
        state_list("todos", || (1..=n as u32).map(|i| Todo { id: i, title: format!("Item {i}"), done: false }).collect(), |t: &Todo| t.id);
    let active = todos.filter(|t| !t.done);
    let done = todos.filter(|t| t.done);
    let next_id = state("next_id", || n as u32 + 1);
    let draft = state("draft", String::new);
    let showing = state("showing", || Show::All);
    let shown_active = active.clone();
    let shown_done = done.clone();
    let visible = memo(move || match showing.get() {
        Show::All => todos.read_only(),
        Show::Active => shown_active.clone(),
        Show::Done => shown_done.clone(),
    });
    let add = move || {
        let title = draft.get().trim().to_owned();
        if title.is_empty() {
            return;
        }
        let id = next_id.get();
        next_id.set(id + 1);
        todos.push(Todo { id, title, done: false });
        draft.set(String::new());
    };
    let filter_button = move |label: &'static str, id: &'static str, s: Show| {
        toggle_button(label, move || showing.get() == s).id(id).on_press(move || showing.set(s))
    };
    col()
        .child(
            row()
                .child(text_field(draft).id("new-todo").label("New todo").on_submit(add))
                .child(primary_button("Add").id("add").on_press(add)),
        )
        .child(text(move || format!("{} left", active.len())).id("left"))
        .child(
            row()
                .child(filter_button("All", "filter-all", Show::All))
                .child(filter_button("Active", "filter-active", Show::Active))
                .child(filter_button("Done", "filter-done", Show::Done)),
        )
        .child(col().child(each(move || visible.get(), move |id, t| todo_row(*id, t, todos))).id("todos").flex_1().overflow_y_scroll())
        .child(show(move || !done.is_empty(), move || button("Clear done").id("clear-done").on_press(move || todos.retain(|t| !t.done))))
        .size_full()
        .into()
}

#[ui]
fn todo_row(id: u32, t: ReadSignal<Todo>, todos: StateList<u32, Todo>) -> El {
    row()
        .child(
            checkbox((move || t.with(|t| t.done), move |v| todos.update(&id, |t| t.done = v)))
                .id(format!("toggle-{id}"))
                .label(move || t.with(|t| format!("Done: {}", t.title))),
        )
        .child(
            text(move || t.with(|t| t.title.clone()))
                .id(format!("title-{id}"))
                .text_color(move || if t.with(|t| t.done) { Token::TextMuted } else { Token::Text }),
        )
        .child(ghost_button("Delete").id(format!("remove-{id}")).on_press(move || todos.remove(&id)))
        .id(format!("todo-{id}"))
}

#[derive(Clone, PartialEq, Debug)]
pub struct Msg {
    pub id: u32,
    pub role: &'static str,
    pub body: String,
}

#[ui]
pub fn chat(n: usize) -> View {
    let messages = state_list(
        "messages",
        || {
            (1..=n as u32)
                .map(|i| Msg { id: i, role: if i % 2 == 1 { "user" } else { "assistant" }, body: format!("message {i}") })
                .collect()
        },
        |m: &Msg| m.id,
    );
    let next_id = state("next_id", || n as u32 + 1);
    let streaming = state("streaming", || false);
    let composer = state("composer", String::new);
    let send = move |text: String| {
        let id = next_id.get();
        next_id.set(id + 2);
        messages.push(Msg { id, role: "user", body: text });
        messages.push(Msg { id: id + 1, role: "assistant", body: String::new() });
        streaming.set(true);
    };
    listen("user", send);
    listen("token", move |s| {
        if let Some(k) = messages.last_key() {
            messages.update(&k, |m| m.body.push_str(&s));
        }
    });
    listen("done", move |_: String| streaming.set(false));
    col()
        .child(text(move || format!("{} messages", messages.len())).id("header"))
        .child(text(move || if streaming.get() { "streaming" } else { "idle" }.to_string()).id("status"))
        .child(
            col()
                .child(each(
                    move || messages.read_only(),
                    |id, m| {
                        let id = *id;
                        col()
                            .child(text(move || m.with(|m| m.role.to_string())).id(format!("msg-role-{id}")))
                            .child(text(move || m.with(|m| m.body.clone())).id(format!("msg-body-{id}")))
                            .id(format!("msg-{id}"))
                    },
                ))
                .id("messages")
                .flex_1()
                .overflow_y_scroll(),
        )
        .child(row().child(text_field(composer).id("composer").label("Message")).child(primary_button("Send").id("send").on_press(
            move || {
                let text = composer.get();
                if !text.is_empty() {
                    send(text);
                    composer.set(String::new());
                }
            },
        )))
        .size_full()
        .into()
}

#[ui]
pub fn settings() -> View {
    let notifications = state("notifications", || false);
    let sound = state("sound", || true);
    let volume = state("volume", || "50".to_string());
    let theme = state("theme", || "system");
    let telemetry = state("telemetry", || false);
    let on_off = |b: bool| if b { "on" } else { "off" };
    let theme_button =
        move |t: &'static str| toggle_button(t, move || theme.get() == t).id(format!("theme-{t}")).on_press(move || theme.set(t));
    col()
        .child(switch(notifications).id("notifications").label("Notifications"))
        .child(show(
            move || notifications.get(),
            move || {
                col()
                    .child(switch(sound).id("sound").label("Sound"))
                    .child(text_field(volume).id("volume").label("Volume"))
                    .id("notification-options")
            },
        ))
        .child(row().child(theme_button("light")).child(theme_button("dark")).child(theme_button("system")))
        .child(disclosure("account", "Account", || col().child(text("Signed in as dev").id("account-name")).id("account-body").into()))
        .child(disclosure("advanced", "Advanced", move || {
            col().child(checkbox(telemetry).id("telemetry").label("Send usage data")).id("advanced-body").into()
        }))
        .child(
            text(move || {
                format!(
                    "notifications={} sound={} volume={} theme={} telemetry={}",
                    on_off(notifications.get()),
                    on_off(sound.get()),
                    volume.get(),
                    theme.get(),
                    on_off(telemetry.get())
                )
            })
            .id("summary"),
        )
        .size_full()
        .into()
}

#[ui]
fn disclosure(id: &'static str, title: &'static str, body: impl Fn() -> View + 'static) -> View {
    scope(format!("Disclosure[{id}]"), move || -> View {
        let open = state("open", || false);
        col()
            .child(ghost_button(title).id(format!("{id}-toggle")).expanded(move || open.get()).on_press(move || open.update(|o| *o = !*o)))
            .child(show(move || open.get(), body))
            .into()
    })
}

/// Message row height: two 20 px lines (16 px text × 1.25), while bodies stay on one line.
pub const MSG_ROW_HEIGHT: f32 = 40.0;

/// The chat app with a virtualized message list (600 px viewport). Same behaviour as [`chat`];
/// only messages near the visible area exist as nodes. `Event::ScrollTo("messages", 0, y)` scrolls.
#[ui]
pub fn chat_virtual(n: usize) -> View {
    let messages = state_list(
        "messages",
        || {
            (1..=n as u32)
                .map(|i| Msg { id: i, role: if i % 2 == 1 { "user" } else { "assistant" }, body: format!("message {i}") })
                .collect()
        },
        |m: &Msg| m.id,
    );
    let next_id = state("next_id", || n as u32 + 1);
    let streaming = state("streaming", || false);
    let composer = state("composer", String::new);
    let send = move |text: String| {
        let id = next_id.get();
        next_id.set(id + 2);
        messages.push(Msg { id, role: "user", body: text });
        messages.push(Msg { id: id + 1, role: "assistant", body: String::new() });
        streaming.set(true);
    };
    listen("user", send);
    listen("token", move |s| {
        if let Some(k) = messages.last_key() {
            messages.update(&k, |m| m.body.push_str(&s));
        }
    });
    listen("done", move |_: String| streaming.set(false));
    col()
        .child(text(move || format!("{} messages", messages.len())).id("header"))
        .child(text(move || if streaming.get() { "streaming" } else { "idle" }.to_string()).id("status"))
        .child(
            col()
                .child(virtual_each(
                    move || messages.read_only(),
                    |_| MSG_ROW_HEIGHT,
                    |id, m| {
                        let id = *id;
                        col()
                            .child(text(move || m.with(|m| m.role.to_string())).id(format!("msg-role-{id}")))
                            .child(text(move || m.with(|m| m.body.clone())).id(format!("msg-body-{id}")))
                            .id(format!("msg-{id}"))
                    },
                ))
                .id("messages")
                .flex_1()
                .overflow_y_scroll(),
        )
        .child(row().child(text_field(composer).id("composer").label("Message")).child(primary_button("Send").id("send").on_press(
            move || {
                let text = composer.get();
                if !text.is_empty() {
                    send(text);
                    composer.set(String::new());
                }
            },
        )))
        .size_full()
        .into()
}

// ---- Contacts: sort_by + group_by ----

#[derive(Clone, PartialEq, Debug)]
pub struct Contact {
    pub id: u32,
    pub name: String,
    pub team: &'static str,
    pub starred: bool,
}

pub const TEAMS: [&str; 3] = ["blue", "green", "red"];

/// Contacts sorted by name, grouped by team; each group has a header with its size. Events:
/// `Send("rename", "id:name")`, `Send("team", "id:team")`, `Send("add", "name:team")`,
/// `Toggle("star-{id}")`, `Press("open-{id}")` (row-local state).
#[ui]
pub fn contacts(n: usize) -> View {
    let people = state_list(
        "people",
        || {
            (1..=n as u32)
                .map(|i| Contact {
                    id: i,
                    name: format!("p{:06}", (i as u64 * 7919) % (n as u64 * 10)),
                    team: TEAMS[i as usize % 3],
                    starred: false,
                })
                .collect()
        },
        |c: &Contact| c.id,
    );
    let next_id = state("next_id", || n as u32 + 1);
    let groups = people.sort_by(|c| c.name.clone()).group_by(|c| c.team);
    let split = |s: &str| s.split_once(':').map(|(a, b)| (a.to_owned(), b.to_owned())).unwrap_or_default();
    listen("rename", move |p| {
        let (id, name) = split(&p);
        people.update(&id.parse().unwrap_or(0), |c| c.name = name);
    });
    listen("team", move |p| {
        let (id, team) = split(&p);
        let team = TEAMS.into_iter().find(|t| *t == team).unwrap_or("blue");
        people.update(&id.parse().unwrap_or(0), |c| c.team = team);
    });
    listen("add", move |p| {
        let (name, team) = split(&p);
        let id = next_id.get();
        next_id.set(id + 1);
        let team = TEAMS.into_iter().find(|t| *t == team).unwrap_or("blue");
        people.push(Contact { id, name, team, starred: false });
    });
    col()
        .child(text(move || format!("{} teams", groups.len())).id("teams"))
        .child(
            col()
                .child(each(
                    move || groups.read_only(),
                    move |team, info| {
                        let team = *team;
                        let members = groups.members(&team).expect("group exists while its row does");
                        col()
                            .child(text(move || info.with(|i| format!("{} ({})", i.key, i.len))).id(format!("team-{team}")))
                            .child(
                                col()
                                    .child(each(move || members.clone(), move |id, c| contact_row(*id, c, people)))
                                    .id(format!("members-{team}")),
                            )
                            .id(format!("group-{team}"))
                    },
                ))
                .id("groups")
                .flex_1()
                .overflow_y_scroll(),
        )
        .size_full()
        .into()
}

#[ui]
fn contact_row(id: u32, c: ReadSignal<Contact>, people: StateList<u32, Contact>) -> El {
    let open = state("open", || false);
    row()
        .child(text(move || c.with(|c| c.name.clone())).id(format!("name-{id}")))
        .child(
            checkbox((move || c.with(|c| c.starred), move |v| people.update(&id, |c| c.starred = v)))
                .id(format!("star-{id}"))
                .label(move || c.with(|c| format!("Star {}", c.name))),
        )
        .child(ghost_button("Open").id(format!("open-{id}")).on_press(move || open.update(|o| *o = !*o)))
        .child(show(move || open.get(), move || text("details").id(format!("detail-{id}"))))
        .id(format!("contact-{id}"))
}

/// Header row height (a 20 px line + 2 × 4 px padding) and contact row height (its kit button:
/// a 17.5 px `text_sm` line + 2 × 6 px padding + 2 × 1 px border, rounded).
pub const HEADER_HEIGHT: f32 = 28.0;
pub const CONTACT_HEIGHT: f32 = 32.0;

/// [`contacts`] with the grouped, sorted list flattened and virtualized: headers and contacts
/// in one virtual list with two row heights. Same events, plus `Event::ScrollTo("contacts", 0, y)`.
#[ui]
pub fn contacts_virtual(n: usize) -> View {
    let people = state_list(
        "people",
        || {
            (1..=n as u32)
                .map(|i| Contact {
                    id: i,
                    name: format!("p{:06}", (i as u64 * 7919) % (n as u64 * 10)),
                    team: TEAMS[i as usize % 3],
                    starred: false,
                })
                .collect()
        },
        |c: &Contact| c.id,
    );
    let next_id = state("next_id", || n as u32 + 1);
    let groups = people.sort_by(|c| c.name.clone()).group_by(|c| c.team);
    let flat = groups.flatten();
    let split = |s: &str| s.split_once(':').map(|(a, b)| (a.to_owned(), b.to_owned())).unwrap_or_default();
    listen("rename", move |p| {
        let (id, name) = split(&p);
        people.update(&id.parse().unwrap_or(0), |c| c.name = name);
    });
    listen("team", move |p| {
        let (id, team) = split(&p);
        let team = TEAMS.into_iter().find(|t| *t == team).unwrap_or("blue");
        people.update(&id.parse().unwrap_or(0), |c| c.team = team);
    });
    listen("add", move |p| {
        let (name, team) = split(&p);
        let id = next_id.get();
        next_id.set(id + 1);
        let team = TEAMS.into_iter().find(|t| *t == team).unwrap_or("blue");
        people.push(Contact { id, name, team, starred: false });
    });
    let height = |it: &FlatItem<&'static str, Contact>| if it.header().is_some() { HEADER_HEIGHT } else { CONTACT_HEIGHT };
    col()
        .child(text(move || format!("{} teams", groups.len())).id("teams"))
        .child(
            col()
                .child(virtual_each(
                    move || flat.clone(),
                    height,
                    move |key, item| match key.clone() {
                        FlatKey::Header(team) => {
                            text(move || item.with(|f| f.header().map(|h| format!("{} ({})", h.key, h.len)).unwrap_or_default()))
                                .id(format!("team-{team}"))
                                .p_1()
                        }
                        FlatKey::Item(id) => {
                            let open = state("open", || false);
                            let name = move || item.with(|f| f.item().map(|c| c.name.clone()).unwrap_or_default());
                            row()
                                .child(text(name).id(format!("name-{id}")))
                                .child(
                                    checkbox((
                                        move || item.with(|f| f.item().is_some_and(|c| c.starred)),
                                        move |v| people.update(&id, |c| c.starred = v),
                                    ))
                                    .id(format!("star-{id}"))
                                    .label(move || format!("Star {}", name())),
                                )
                                .child(ghost_button("Open").id(format!("open-{id}")).on_press(move || open.update(|o| *o = !*o)))
                                .child(show(move || open.get(), move || text("details").id(format!("detail-{id}"))))
                                .id(format!("contact-{id}"))
                        }
                    },
                ))
                .id("contacts")
                .flex_1()
                .overflow_y_scroll(),
        )
        .size_full()
        .into()
}

// ---- Chat with streamed text ----

/// A message whose body is a text buffer: tokens are appended as edits.
#[derive(Clone, PartialEq, Debug)]
pub struct StreamMsg {
    pub id: u32,
    pub role: &'static str,
    pub body: TextBuffer,
}

/// [`chat_virtual`] with bodies as `TextBuffer` rendered by `text(body)`: a token is an append edit
/// that reaches the kernel as an append, O(token) instead of O(body).
#[ui]
pub fn chat_stream(n: usize) -> View {
    let messages = state_list(
        "messages",
        || {
            (1..=n as u32)
                .map(|i| StreamMsg {
                    id: i,
                    role: if i % 2 == 1 { "user" } else { "assistant" },
                    body: TextBuffer::new(format!("message {i}")),
                })
                .collect()
        },
        |m: &StreamMsg| m.id,
    );
    let next_id = state("next_id", || n as u32 + 1);
    let streaming = state("streaming", || false);
    let composer = state("composer", String::new);
    let send = move |text: String| {
        let id = next_id.get();
        next_id.set(id + 2);
        messages.push(StreamMsg { id, role: "user", body: TextBuffer::new(text) });
        messages.push(StreamMsg { id: id + 1, role: "assistant", body: TextBuffer::new("") });
        streaming.set(true);
    };
    listen("user", send);
    listen("token", move |s| {
        if let Some(m) = messages.last_key().and_then(|k| messages.item(&k)) {
            untrack(|| m.with(|m| m.body.append(&s)));
        }
    });
    listen("done", move |_: String| streaming.set(false));
    col()
        .child(text(move || format!("{} messages", messages.len())).id("header"))
        .child(text(move || if streaming.get() { "streaming" } else { "idle" }.to_string()).id("status"))
        .child(
            col()
                .child(virtual_each(
                    move || messages.read_only(),
                    |_| MSG_ROW_HEIGHT,
                    |id, m| {
                        let id = *id;
                        let StreamMsg { role, body, .. } = untrack(|| m.get());
                        col()
                            .child(text(role).id(format!("msg-role-{id}")))
                            .child(text(body).id(format!("msg-body-{id}")))
                            .id(format!("msg-{id}"))
                    },
                ))
                .id("messages")
                .flex_1()
                .overflow_y_scroll(),
        )
        .child(row().child(text_field(composer).id("composer").label("Message")).child(primary_button("Send").id("send").on_press(
            move || {
                let text = composer.get();
                if !text.is_empty() {
                    send(text);
                    composer.set(String::new());
                }
            },
        )))
        .size_full()
        .into()
}

/// The four spec apps, for the shared scenarios.
pub struct Apps;

impl crate::scenarios::Factory for Apps {
    const NAME: &'static str = "examples";
    fn counter() -> Window {
        Window::headless(counter)
    }
    fn todo_with(n: usize) -> Window {
        Window::headless(move || todo(n))
    }
    fn chat_with(n: usize) -> Window {
        Window::headless(move || chat(n))
    }
    fn settings() -> Window {
        Window::headless(settings)
    }
}

/// Text from outside (a test, an agent, a model's stream) on the channel named `name`.
// ---- Inbox: join ----

#[derive(Clone, PartialEq, Debug)]
pub struct Person {
    pub id: u32,
    pub name: String,
    pub online: bool,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Message {
    pub id: u32,
    pub from: u32,
    pub body: String,
}

/// `n` messages from 20 people, each row showing its sender's name (a `join`): renaming someone
/// rewrites their rows only, and a change the rows do not show (going online) rewrites none.
/// Events: `Send("rename", "id:name")`, `Send("online", "id")`, `Send("forget", "id")` (the
/// person goes; their rows stay, from "unknown").
#[ui]
pub fn inbox(n: usize) -> View {
    let people = state_list(
        "people",
        || (1..=20u32).map(|id| Person { id, name: format!("Person {id}"), online: false }).collect(),
        |p: &Person| p.id,
    );
    let messages = state_list(
        "messages",
        || (1..=n as u32).map(|id| Message { id, from: id % 20 + 1, body: format!("Message {id}") }).collect(),
        |m: &Message| m.id,
    );
    let rows = messages.join(people, |m| m.from, |m, p| (p.map_or_else(|| "unknown".to_string(), |p| p.name.clone()), m.body.clone()));
    let split = |s: &str| s.split_once(':').map(|(a, b)| (a.to_owned(), b.to_owned())).unwrap_or_default();
    listen("rename", move |p| {
        let (id, name) = split(&p);
        people.update(&id.parse().unwrap_or(0), |p| p.name = name);
    });
    listen("online", move |id| people.update(&id.parse().unwrap_or(0), |p| p.online = !p.online));
    listen("forget", move |id| people.remove(&id.parse().unwrap_or(0)));
    col()
        .size_full()
        .child(col().id("messages").flex_1().overflow_y_scroll().child(each(
            move || rows.clone(),
            |id, msg| {
                row()
                    .gap_2()
                    .child(text(move || msg.with(|r| r.0.clone())).font_semibold().id(format!("from-{id}")))
                    .child(text(move || msg.with(|r| r.1.clone())))
                    .id(format!("message-{id}"))
            },
        )))
        .into()
}

fn listen(name: &str, f: impl Fn(String) + 'static) {
    let (_, rx) = channel::<String>();
    rx.named(name).on(f);
}

/// Several windows: the first opens notes, each in a window of its own that closes itself; the
/// desktop host ends the app with its last window. The opened count belongs to the App root.
pub fn windows() -> View {
    let opened = App::current().run(|| state("notes-opened", || 0u32));
    let picked = state("picked", String::new);
    col()
        .size_full()
        .p_6()
        .gap_3()
        .child(text("Windows").text_xl().font_semibold())
        .child(text(move || format!("{} notes opened", opened.get())).id("opened").text_color(Token::TextMuted))
        .child(primary_button("New note").id("new").self_start().on_press(move || {
            let n = opened.get() + 1;
            opened.set(n);
            let options = WindowOptions::new(format!("Note {n}")).size(360.0, 240.0);
            let _ = open_window(options, move || note(n));
        }))
        .child(button("Open files…").id("open").self_start().on_press(move || {
            spawn(async move {
                let options = PathPromptOptions { files: true, multiple: true, prompt: Some("Open".into()), ..Default::default() };
                let paths = prompt_for_paths(options).await;
                picked.set(match paths {
                    Some(p) => p.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n"),
                    None => "Cancelled".into(),
                });
            });
        }))
        .child(text(move || picked.get()).id("picked").text_sm().text_color(Token::TextMuted))
        .child(
            col()
                .id("drop")
                .h(100.0)
                .items_center()
                .justify_center()
                .rounded_lg()
                .border_2()
                .border_color(Token::Border)
                .drag_over(|p| p.border_color(Token::Accent).bg(Token::Surface))
                .on_drop(move |paths| picked.set(paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n")))
                .child(text("Drop files here").text_color(Token::TextMuted)),
        )
        .into()
}

fn note(n: u32) -> View {
    let body = state("body", String::new);
    col()
        .size_full()
        .p_4()
        .gap_3()
        .child(text(format!("Note {n}")).font_semibold())
        .child(text_field(body).id("body").label("Note"))
        .child(button("Close").id("close").self_start().on_press(|| close_window(current_window())))
        .into()
}
