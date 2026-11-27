//! Stateful examples shared by native windows, reference captures and tests.

use crate::window_options;
use octara::prelude::*;
use octara_kit::{ButtonVariant, Size as KitSize, button, checkbox, switch, text_field};
use std::time::Duration;

pub fn counter_app() -> AppDefinition {
    AppDefinition::new().state_schema("counter/1").window(WindowDefinition::new(window_options("Octara — counter", 800.0, 600.0), counter))
}

#[ui]
pub fn counter(cx: &mut BuildCx<'_>) -> El {
    let count = cx.persisted("count", || 0_i64);
    let double = cx.memo(move |r| r.get(count) * 2);
    col()
        .size(rel(1.0))
        .child(text(move |r: &mut ReadCx<'_>| format!("Count: {}", r.get(count))).test_id("count"))
        .child(text(move |r: &mut ReadCx<'_>| format!("Double: {}", r.get(double))).test_id("double"))
        .child(
            row()
                .child(button("+1", KitSize::Md, ButtonVariant::Secondary).test_id("inc").on_press(move |w| w.update(count, |n| *n += 1)))
                .child(button("Reset", KitSize::Md, ButtonVariant::Secondary).test_id("reset").on_press(move |w| w.set(count, 0))),
        )
}

#[ui]
pub fn styles(cx: &mut BuildCx<'_>) -> El {
    let early = cx.signal(64);
    let late = cx.signal(96);
    let patch = cx.signal(true);
    col()
        .size(rel(1.0))
        .p(sp(4.0))
        .gap(sp(2.0))
        .child(text("Style declaration order"))
        .child(text("Later static width; wider on medium screens"))
        .child(
            el().test_id("static-last")
                .h(24)
                .bg(ColorToken::Accent)
                .style_with(move |r, p| p.w(r.get(early)))
                .w(80)
                .when(Breakpoint::Medium, |p| p.w(120)),
        )
        .child(text("Later dynamic width"))
        .child(
            el().test_id("dynamic-last")
                .h(24)
                .bg(ColorToken::Accent)
                .style_with(move |r, p| p.w(r.get(early)))
                .style_with(move |r, p| p.w(r.get(late))),
        )
        .child(text("Optional patch; removal restores the earlier width"))
        .child(
            el().test_id("patch")
                .h(24)
                .bg(ColorToken::Accent)
                .style_with(move |r, p| p.w(r.get(early)))
                .style_with(move |r, p| if r.get(patch) { p.w(160).bg(ColorToken::Danger) } else { p }),
        )
        .child(
            button("Change earlier width", KitSize::Md, ButtonVariant::Secondary)
                .test_id("early")
                .align_self(AlignItems::Start)
                .on_press(move |w| w.update(early, |v| *v = if *v == 64 { 112 } else { 64 })),
        )
        .child(
            button("Change later width", KitSize::Md, ButtonVariant::Secondary)
                .test_id("late")
                .align_self(AlignItems::Start)
                .on_press(move |w| w.update(late, |v| *v = if *v == 96 { 144 } else { 96 })),
        )
        .child(
            button("Toggle patch", KitSize::Md, ButtonVariant::Secondary)
                .test_id("toggle-patch")
                .align_self(AlignItems::Start)
                .on_press(move |w| w.update(patch, |v| *v = !*v)),
        )
}

#[ui]
pub fn gestures(cx: &mut BuildCx<'_>) -> El {
    let presses = cx.signal(0_u32);
    let doubles = cx.signal(0_u32);
    let vanished = cx.signal(false);
    let vanished_doubles = cx.signal(0_u32);
    col()
        .test_id("gesture-root")
        .size(rel(1.0))
        .p(sp(4.0))
        .gap(sp(2.0))
        .child(text(move |r: &mut ReadCx<'_>| format!("Presses: {}", r.get(presses))).test_id("presses"))
        .child(text(move |r: &mut ReadCx<'_>| format!("Doubles: {}", r.get(doubles))).test_id("doubles"))
        .child(
            button("Double click", KitSize::Md, ButtonVariant::Secondary)
                .test_id("double-target")
                .align_self(AlignItems::Start)
                .on_press(move |w| w.update(presses, |n| *n += 1))
                .on_double_press(move |w| w.update(doubles, |n| *n += 1)),
        )
        .child(text(move |r: &mut ReadCx<'_>| format!("Removed doubles: {}", r.get(vanished_doubles))).test_id("vanished-doubles"))
        .child(show(
            move |r| !r.get(vanished),
            move |_| {
                button("Remove on first click", KitSize::Md, ButtonVariant::Secondary)
                    .test_id("vanishing-double")
                    .align_self(AlignItems::Start)
                    .on_press(move |w| w.set(vanished, true))
                    .on_double_press(move |w| w.update(vanished_doubles, |n| *n += 1))
            },
        ))
}

#[derive(Clone, PartialEq)]
struct Todo {
    title: String,
    done: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Show {
    All,
    Active,
    Done,
}

#[ui]
pub fn todo(cx: &mut BuildCx<'_>, n: usize) -> El {
    let todos = cx.list::<usize, Todo>();
    let buffer = cx.signal(String::new());
    let showing = cx.signal(Show::All);
    let next = cx.signal(n + 1);
    let active = todos.read_only().filter(cx, |todo| !todo.done);
    let done = todos.read_only().filter(cx, |todo| todo.done);
    let visible = todos.read_only().filter_with(cx, showing, |todo, show| match show {
        Show::All => true,
        Show::Active => !todo.done,
        Show::Done => todo.done,
    });
    cx.spawn("seed todos", move |task| async move {
        task.write(|w| {
            w.reconcile_list(todos, (1..=n).map(|id| (id, Todo { title: format!("Item {id}"), done: false })).collect());
        });
    });
    let add = move |w: &mut WriteCx<'_>| {
        let title = w.get(buffer).trim().to_owned();
        if !title.is_empty() {
            let id = w.get(next);
            w.set(next, id + 1);
            w.push_row(todos, id, Todo { title, done: false });
            w.set(buffer, String::new());
        }
    };
    let filter = move |label, id, selected| {
        button(label, KitSize::Md, ButtonVariant::Ghost)
            .selected(move |r: &mut ReadCx<'_>| r.get(showing) == selected)
            .when(State::SELECTED, |p| p.bg(ColorToken::Surface).border_color(ColorToken::Border).text_color(ColorToken::Accent))
            .test_id(id)
            .on_press(move |w| w.set(showing, selected))
    };

    col()
        .size(rel(1.0))
        .gap(sp(2.0))
        .child(
            row()
                .child(text_field(buffer.into()).test_id("new-todo").label("New todo").on_submit(add))
                .child(button("Add", KitSize::Md, ButtonVariant::Primary).test_id("add").on_press(add)),
        )
        .child(text(move |r: &mut ReadCx<'_>| format!("{} left", r.len(active))).test_id("left"))
        .child(
            row()
                .gap(sp(1.0))
                .child(filter("All", "filter-all", Show::All))
                .child(filter("Active", "filter-active", Show::Active))
                .child(filter("Done", "filter-done", Show::Done))
                .child(show(
                    move |r| r.len(done) != 0,
                    move |_| {
                        button("Clear done", KitSize::Md, ButtonVariant::Secondary).test_id("clear-done").on_press(move |w| {
                            for id in w.keys(done) {
                                w.remove_row(todos, &id);
                            }
                        })
                    },
                )),
        )
        .child(col().test_id("todos").grow(1.0).min_h(0).overflow_y(Overflow::Scroll).child(each(visible, move |b, &id, todo| {
            row()
                .test_id(format!("todo-{id}"))
                .shrink(0.0)
                .gap(sp(2.0))
                .align(AlignItems::Center)
                .child(
                    checkbox(b.binding(
                        move |r| r.try_with(todo, |row| row.done).unwrap_or_default(),
                        move |w, done| w.update_row(todos, &id, |todo| todo.done = done),
                    ))
                    .test_id(format!("toggle-{id}")),
                )
                .child(
                    text(move |r: &mut ReadCx<'_>| r.try_with(todo, |row| row.title.clone()).unwrap_or_default())
                        .test_id(format!("title-{id}"))
                        .text_color(move |r: &mut ReadCx<'_>| {
                            if r.try_with(todo, |row| row.done).unwrap_or_default() { ColorToken::TextMuted } else { ColorToken::Text }
                        }),
                )
                .child(button("Remove", KitSize::Md, ButtonVariant::Ghost).test_id(format!("remove-{id}")).on_press(move |w| {
                    w.remove_row(todos, &id);
                }))
        })))
}

#[derive(Clone, PartialEq)]
struct Message {
    author: &'static str,
    body: String,
}

pub const MSG_ROW_HEIGHT: f32 = 44.0;

pub fn chat(cx: &mut BuildCx<'_>, n: usize) -> El {
    chat_view(cx, n, false)
}

pub fn chat_virtual(cx: &mut BuildCx<'_>, n: usize) -> El {
    chat_view(cx, n, true)
}

#[ui]
fn chat_view(cx: &mut BuildCx<'_>, n: usize, virtualized: bool) -> El {
    let messages = cx.list::<usize, Message>();
    let next = cx.signal(n + 1);
    let streaming = cx.signal(false);
    let composer = cx.signal(String::new());
    cx.spawn("seed messages", move |task| async move {
        task.write(|w| {
            w.reconcile_list(
                messages,
                (1..=n)
                    .map(|id| (id, Message { author: if id % 2 == 1 { "user" } else { "assistant" }, body: format!("message {id}") }))
                    .collect(),
            );
        });
    });
    let send = move |w: &mut WriteCx<'_>| {
        let body = w.get(composer);
        if body.trim().is_empty() {
            return;
        }
        let id = w.get(next);
        w.set(next, id + 2);
        w.push_row(messages, id, Message { author: "user", body });
        w.push_row(messages, id + 1, Message { author: "assistant", body: String::new() });
        w.set(composer, String::new());
        w.set(streaming, true);
        w.spawn("stream reply", move |task| async move {
            let start = task.now();
            for (index, token) in ["Got ", "your ", "message."].into_iter().enumerate() {
                task.sleep_until(start + Duration::from_millis(80 * (index as u64 + 1))).await;
                task.write(|w| w.update_row(messages, &(id + 1), |message| message.body.push_str(token)));
            }
            task.write(|w| {
                if w.get(next) == id + 2 {
                    w.set(streaming, false);
                }
            });
        });
    };
    let render = |_: &mut BuildCx<'_>, &id: &usize, msg: RowSignal<Message>| {
        col()
            .test_id(format!("msg-{id}"))
            .shrink(0.0)
            .p(sp(1.0))
            .child(text(move |r: &mut ReadCx<'_>| r.try_with(msg, |row| row.author).unwrap_or_default()).test_id(format!("msg-role-{id}")))
            .child(
                text(move |r: &mut ReadCx<'_>| r.try_with(msg, |row| row.body.clone()).unwrap_or_default())
                    .test_id(format!("msg-body-{id}")),
            )
    };
    let rows = if virtualized {
        virtual_each(messages, VirtualOptions { estimated_row_height: MSG_ROW_HEIGHT, ..Default::default() }, render)
    } else {
        each(messages, render)
    };
    col()
        .size(rel(1.0))
        .gap(sp(2.0))
        .child(text(move |r: &mut ReadCx<'_>| format!("{} messages", r.len(messages))).test_id("header"))
        .child(text(move |r: &mut ReadCx<'_>| if r.get(streaming) { "streaming" } else { "idle" }).test_id("status"))
        .child(col().test_id("messages").grow(1.0).min_h(0).overflow_y(Overflow::Scroll).child(rows))
        .child(
            row()
                .gap(sp(2.0))
                .child(text_field(composer.into()).test_id("composer").label("Message").on_submit(send))
                .child(button("Send", KitSize::Md, ButtonVariant::Primary).test_id("send").on_press(send)),
        )
}

#[derive(Clone, PartialEq)]
struct Contact {
    name: String,
    team: &'static str,
    starred: bool,
}

const TEAMS: [&str; 3] = ["blue", "green", "red"];

pub fn contacts(cx: &mut BuildCx<'_>, n: usize) -> El {
    contacts_view(cx, n, false)
}

pub fn contacts_virtual(cx: &mut BuildCx<'_>, n: usize) -> El {
    contacts_view(cx, n, true)
}

#[ui]
fn contacts_view(cx: &mut BuildCx<'_>, n: usize, virtualized: bool) -> El {
    let people = cx.list::<usize, Contact>();
    let sorted = people.read_only().sort_by(cx, |p| p.name.clone());
    let groups = sorted.group_by(cx, |p| p.team);
    let headers = groups.read_only();
    let rows = groups.flatten();
    cx.spawn("seed contacts", move |task| async move {
        task.write(|w| {
            w.reconcile_list(
                people,
                (1..=n)
                    .map(|id| {
                        (id, Contact { name: format!("p{:06}", id as u64 * 7919 % (n as u64 * 10)), team: TEAMS[id % 3], starred: false })
                    })
                    .collect(),
            );
        });
    });
    let render = move |b: &mut BuildCx<'_>, key: &FlatKey<&'static str, usize>, _| match *key {
        FlatKey::Header(team) => text(move |r: &mut ReadCx<'_>| {
            r.with_row(headers, &team, |header| header.map(|header| format!("{} ({})", header.key, header.size)).unwrap_or_default())
        })
        .test_id(format!("team-{team}"))
        .text_weight(600)
        .p(sp(1.0))
        .shrink(0.0),
        FlatKey::Item(id) => {
            let open = b.signal(false);
            let person = b.row_signal(people, id);
            row()
                .gap(sp(2.0))
                .shrink(0.0)
                .align(AlignItems::Center)
                .test_id(format!("contact-{id}"))
                .child(
                    text(move |r: &mut ReadCx<'_>| r.try_with(person, |person| person.name.clone()).unwrap_or_default())
                        .test_id(format!("name-{id}")),
                )
                .child(
                    checkbox(b.binding(
                        move |r| r.try_with(person, |person| person.starred).unwrap_or_default(),
                        move |w, value| w.update_row(people, &id, |p| p.starred = value),
                    ))
                    .test_id(format!("star-{id}"))
                    .label(move |r: &mut ReadCx<'_>| r.try_with(person, |person| format!("Star {}", person.name)).unwrap_or_default()),
                )
                .child(
                    button("Open", KitSize::Md, ButtonVariant::Ghost)
                        .test_id(format!("open-{id}"))
                        .on_press(move |w| w.update(open, |v| *v = !*v)),
                )
                .child(show(move |r| r.get(open), move |_| text("details").test_id(format!("detail-{id}"))))
        }
    };
    let rows = if virtualized {
        virtual_each(rows, VirtualOptions { estimated_row_height: 32.0, ..Default::default() }, render)
    } else {
        each(rows, render)
    };
    col()
        .size(rel(1.0))
        .child(text(move |r: &mut ReadCx<'_>| format!("{} teams", r.len(headers))).test_id("teams"))
        .child(col().test_id("contacts").grow(1.0).min_h(0).overflow_y(Overflow::Scroll).child(rows))
}

#[ui]
pub fn settings(cx: &mut BuildCx<'_>) -> El {
    let notifications = cx.signal(false);
    let sound = cx.signal(true);
    let volume = cx.signal("50".to_owned());
    let theme = cx.signal("system");
    let telemetry = cx.signal(false);
    let account = cx.signal(false);
    let advanced = cx.signal(false);
    col()
        .size(rel(1.0))
        .p(sp(4.0))
        .gap(sp(2.0))
        .child(switch(notifications.into()).test_id("notifications").label("Notifications"))
        .child(show(
            move |r| r.get(notifications),
            move |_| {
                col()
                    .test_id("notification-options")
                    .gap(sp(2.0))
                    .child(switch(sound.into()).test_id("sound").label("Sound"))
                    .child(text_field(volume.into()).test_id("volume").label("Volume"))
            },
        ))
        .child(row().gap(sp(2.0)).children(["system", "light", "dark"].map(|choice| {
            button(choice, KitSize::Md, ButtonVariant::Ghost)
                .selected(move |r: &mut ReadCx<'_>| r.get(theme) == choice)
                .when(State::SELECTED, |p| p.bg(ColorToken::Surface).border_color(ColorToken::Border).text_color(ColorToken::Accent))
                .test_id(format!("theme-{choice}"))
                .on_press(move |w| w.set(theme, choice))
        })))
        .child(
            button("Account", KitSize::Md, ButtonVariant::Secondary)
                .test_id("account-toggle")
                .on_press(move |w| w.update(account, |open| *open = !*open)),
        )
        .child(show(move |r| r.get(account), |_| col().test_id("account-body").child(text("Signed in as dev").test_id("account-name"))))
        .child(
            button("Advanced", KitSize::Md, ButtonVariant::Secondary)
                .test_id("advanced-toggle")
                .on_press(move |w| w.update(advanced, |open| *open = !*open)),
        )
        .child(show(
            move |r| r.get(advanced),
            move |_| col().test_id("advanced-body").child(switch(telemetry.into()).test_id("telemetry").label("Telemetry")),
        ))
        .child(
            text(move |r: &mut ReadCx<'_>| {
                let flag = |value| if value { "on" } else { "off" };
                format!(
                    "notifications={} sound={} volume={} theme={} telemetry={}",
                    flag(r.get(notifications)),
                    flag(r.get(sound)),
                    r.get(volume),
                    r.get(theme),
                    flag(r.get(telemetry))
                )
            })
            .test_id("summary"),
        )
}

#[ui]
pub fn windows(cx: &mut BuildCx<'_>) -> El {
    let opened = cx.signal(0_u32);
    let picked = cx.signal(String::new());
    col()
        .size(rel(1.0))
        .p(sp(6.0))
        .gap(sp(3.0))
        .align(AlignItems::Start)
        .child(text("Windows").text_size(FontSizeToken::Xl).text_weight(600))
        .child(
            text(move |r: &mut ReadCx<'_>| format!("{} notes opened", r.get(opened))).test_id("opened").text_color(ColorToken::TextMuted),
        )
        .child(button("New note", KitSize::Md, ButtonVariant::Primary).test_id("new").on_press(move |w| {
            let n = w.get(opened) + 1;
            w.set(opened, n);
            w.open_window(window_options(format!("Note {n}"), 360.0, 240.0), move |b| note(b, n));
        }))
        .child(button("Open files…", KitSize::Md, ButtonVariant::Secondary).test_id("open").on_press(move |w| {
            w.ask(Ask::pick_files(Some("Open".into()), true), move |w, answer| {
                let selected = match answer {
                    Answer::Paths(Some(paths)) => paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n"),
                    _ => "Cancelled".into(),
                };
                w.set(picked, selected);
            });
        }))
        .child(text(picked).test_id("picked").text_size(FontSizeToken::Sm).text_color(ColorToken::TextMuted))
}

fn note(cx: &mut BuildCx<'_>, n: u32) -> El {
    let body = cx.signal(String::new());
    let window = cx.window();
    col()
        .size(rel(1.0))
        .p(sp(4.0))
        .gap(sp(3.0))
        .child(text(format!("Note {n}")).text_weight(600))
        .child(text_field(body.into()).test_id("body").label("Note"))
        .child(
            button("Close", KitSize::Md, ButtonVariant::Secondary)
                .test_id("close")
                .align_self(AlignItems::Start)
                .on_press(move |w| w.close_window(window)),
        )
}

pub const UNDO_DOCUMENT_TITLE: &str = "Octara Undo Document";
pub const UNDO_DOCUMENT_SIZE: (f32, f32) = (420.0, 232.0);

#[ui]
pub fn undo_document(cx: &mut BuildCx<'_>, label: &'static str) -> El {
    let draft = cx.signal(label.to_owned());
    let edits = cx.signal(0_u32);
    let mut page = col()
        .size(rel(1.0))
        .p(sp(4.0))
        .gap(sp(3.0))
        .child(text(format!("Document {label}")).test_id("document").text_weight(600))
        .child(input().test_id("editor").label("Draft").w(rel(1.0)).value(draft).on_input(move |w, value| {
            w.set(draft, value.into());
            w.update(edits, |count| *count += 1);
        }))
        .child(
            row()
                .gap(sp(3.0))
                .align(AlignItems::Center)
                .child(
                    button("Undo", KitSize::Md, ButtonVariant::Secondary)
                        .test_id("undo")
                        .preserve_focus_on_press()
                        .on_press(|w| w.request_command("edit.undo")),
                )
                .child(text(move |r: &mut ReadCx<'_>| format!("Edits: {}", r.get(edits))).test_id("edits")),
        );
    if label == "A" {
        page = page.child(button("Open Document B", KitSize::Md, ButtonVariant::Secondary).test_id("open-b").on_press(|w| {
            let (width, height) = UNDO_DOCUMENT_SIZE;
            w.open_window(window_options(UNDO_DOCUMENT_TITLE, width, height), |b| undo_document(b, "B"));
        }));
    }
    page
}
