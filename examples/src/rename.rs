//! A directory refresh fixture for testing an inline editor with the desktop input method.

use octara::prelude::*;

const A: u32 = 1;
const B: u32 = 2;
const C: u32 = 3;
const SEED: &str = "A😀Z";
const EDITOR: &str = "rename-editor";
const ROW_HEIGHT: f32 = 44.0;

#[derive(Clone, PartialEq, Eq)]
struct File {
    name: String,
    revision: u32,
}

fn directory() -> Vec<(u32, File)> {
    [A, B]
        .into_iter()
        .chain(10..42)
        .map(|id| {
            let name = if id < 10 { SEED.to_owned() } else { format!("Reference {id}.txt") };
            (id, File { name, revision: 0 })
        })
        .collect()
}

fn caption(id: u32) -> String {
    match id {
        A => "Directory A".into(),
        B => "Directory B".into(),
        C => "Directory C".into(),
        _ => format!("Directory {id}"),
    }
}

#[derive(Clone, Copy)]
struct Directory {
    files: Signal<Vec<(u32, File)>>,
    selected: Signal<Vec<u32>>,
    editing: Signal<Option<u32>>,
    edits: Signal<u32>,
    attempts: Signal<u32>,
    saves: Signal<u32>,
    status: Signal<String>,
}

impl Directory {
    fn begin(self, id: u32) {
        self.editing.set(Some(id));
        self.status.set(format!("Editing {}", caption(id)));
        focus(EDITOR);
    }

    fn cancel(self, id: u32) {
        if self.editing.get() == Some(id) {
            self.editing.set(None);
            self.status.set(format!("Cancelled {}", caption(id)));
        }
    }

    fn submit(self, id: u32, draft: File) {
        self.attempts.update(|n| *n += 1);
        let current = self.files.with(|files| files.iter().find(|(key, _)| *key == id).map(|(_, file)| file.clone()));
        let Some(current) = current else { return };
        if current.revision != draft.revision {
            self.status.set(format!("Conflict in {}: {}", caption(id), current.name));
            return;
        }
        self.files.update(|files| {
            let (_, file) = files.iter_mut().find(|(key, _)| *key == id).expect("the file was checked in this handler");
            file.name = draft.name;
            file.revision += 1;
        });
        self.saves.update(|n| *n += 1);
        self.editing.set(None);
        self.status.set(format!("Saved {}", caption(id)));
    }
}

fn button(id: &str, label: &str, press: impl Fn() + 'static) -> El {
    text(label)
        .id(id.to_owned())
        .role(Role::Button)
        .focusable()
        .px_3()
        .py_2()
        .min_h(28.0)
        .bg(Token::Surface)
        .rounded_md()
        .focus_visible(|p| p.border_color(Token::Focus))
        .border()
        .on_press(press)
}

#[ui]
fn file_row(id: u32, file: ReadSignal<File>, model: Directory) -> El {
    row()
        .id(format!("rename-file-{id}"))
        .role(Role::ListItem)
        .selected(move || model.selected.with(|ids| ids.contains(&id)))
        .h(ROW_HEIGHT)
        .flex_shrink_0()
        .items_center()
        .gap_3()
        .child(text(caption(id)).w(114).text_sm())
        .child(el().w(300).child(keyed(
            move || model.editing.get() == Some(id),
            move |editing| -> View {
                if *editing {
                    let draft = state("draft", || file.get());
                    text_input()
                        .id(EDITOR)
                        .label(format!("File name in {}", caption(id)))
                        .h(32)
                        .w_full()
                        .px_2()
                        .border()
                        .value(move || draft.with(|file| file.name.clone()))
                        .on_input(move |name| {
                            model.edits.update(|n| *n += 1);
                            draft.update(|file| file.name = name);
                        })
                        .on_submit(move || model.submit(id, draft.get()))
                        .on_blur(move || model.cancel(id))
                        .on_key(move |key| {
                            if key.key == Key::Escape {
                                model.cancel(id);
                                true
                            } else {
                                false
                            }
                        })
                        .into()
                } else {
                    text(move || file.with(|file| file.name.clone())).into()
                }
            },
        )))
        .child(show(move || id == B && model.editing.get() != Some(B), move || button("rename-start", "Rename B", move || model.begin(B))))
}

/// Drives a disposable in-memory directory and its retained row editor.
#[ui]
pub fn view() -> El {
    let model = Directory {
        files: state("files", directory),
        selected: state("selected", Vec::new),
        editing: state("editing", || None),
        edits: state("edits", || 0),
        attempts: state("attempts", || 0),
        saves: state("saves", || 0),
        status: state("status", || "Directory ready".into()),
    };
    on_command("rename.reorder", move || model.files.update(|files| files.swap(0, 1)));
    on_command("rename.refresh", move || model.files.set(model.files.get()));
    on_command("rename.external", move || {
        model.files.update(|files| {
            if let Some((_, file)) = files.iter_mut().find(|(id, _)| *id == B) {
                file.name = "External B.txt".into();
                file.revision += 1;
            }
        });
        model.status.set("Directory B was renamed externally".into());
    });
    on_command("rename.replace", move || {
        model.files.update(|files| {
            if let Some(row) = files.iter_mut().find(|(id, _)| *id == B) {
                *row = (C, File { name: SEED.into(), revision: 0 });
            }
        });
        model.selected.update(|ids| ids.retain(|id| *id != B));
        model.begin(C);
    });
    on_command("rename.reset", move || {
        model.editing.set(None);
        model.files.set(directory());
        model.selected.set(Vec::new());
        model.edits.set(0);
        model.attempts.set(0);
        model.saves.set(0);
        model.status.set("Directory ready".into());
        scroll_to("rename-list", 0.0, 0.0);
    });
    on_command("rename.scroll-away", || {
        scroll_to("rename-list", 0.0, 720.0);
    });
    on_command("rename.scroll-back", || {
        scroll_to("rename-list", 0.0, 0.0);
    });

    col()
        .size_full()
        .p_6()
        .gap_3()
        .bg(Token::Background)
        .child(text("Inline file rename").text_xl().font_semibold())
        .child(row().gap_3().child(button("rename-select", "Select A and B", move || model.selected.set(vec![A, B]))).child(button(
            "rename-outside",
            "Leave editor",
            || focus("rename-outside"),
        )))
        .child(col().id("rename-list").role(Role::List).flex_1().min_h(0.0).overflow_y_scroll().child(virtual_each(
            move || model.files.get(),
            |_| ROW_HEIGHT,
            move |id, file| file_row(*id, file, model),
        )))
        .child(text(move || model.status.get()).id("rename-status").role(Role::Status).live(Live::Polite))
        .child(
            text(move || format!("Edits: {} · Attempts: {} · Saves: {}", model.edits.get(), model.attempts.get(), model.saves.get()))
                .id("rename-counts"),
        )
        .child(
            text(move || {
                model.files.with(|files| {
                    files.iter().filter(|(id, _)| *id < 10).map(|(id, file)| format!("{id}: {}", file.name)).collect::<Vec<_>>().join(" | ")
                })
            })
            .id("rename-records")
            .text_sm(),
        )
}
