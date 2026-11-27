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
    files: StateList<u32, File>,
    selected: Signal<Vec<u32>>,
    editing: Signal<Option<u32>>,
    edits: Signal<u32>,
    attempts: Signal<u32>,
    saves: Signal<u32>,
    status: Signal<String>,
}

impl Directory {
    fn begin(self, w: &mut WriteCx<'_>, id: u32) {
        w.set(self.editing, Some(id));
        w.set(self.status, format!("Editing {}", caption(id)));
    }

    fn cancel(self, w: &mut WriteCx<'_>, id: u32) {
        if w.get(self.editing) == Some(id) {
            w.set(self.editing, None);
            w.set(self.status, format!("Cancelled {}", caption(id)));
        }
    }

    fn submit(self, w: &mut WriteCx<'_>, id: u32, draft: File) {
        w.update(self.attempts, |n| *n += 1);
        let Some(current) = w.get_row(self.files, &id) else { return };
        if current.revision != draft.revision {
            w.set(self.status, format!("Conflict in {}: {}", caption(id), current.name));
            return;
        }
        w.update_row(self.files, &id, |file| {
            file.name = draft.name;
            file.revision += 1;
        });
        w.update(self.saves, |n| *n += 1);
        w.set(self.editing, None);
        w.set(self.status, format!("Saved {}", caption(id)));
    }
}

fn button(id: &str, label: &str, press: impl Fn(&mut WriteCx<'_>) + 'static) -> El {
    octara_kit::button(label, octara_kit::Size::Md, octara_kit::ButtonVariant::Secondary).test_id(id).on_press(press)
}

#[ui]
fn file_row(id: u32, file: RowSignal<u32, File>, model: Directory) -> El {
    let draft_source = file.clone();
    row()
        .test_id(format!("rename-file-{id}"))
        .role(Role::ListItem)
        .selected(move |r: &mut ReadCx<'_>| r.with(model.selected, |ids| ids.contains(&id)))
        .h(ROW_HEIGHT)
        .shrink(0.0)
        .align(AlignItems::Center)
        .gap(sp(3.0))
        .child(text(caption(id)).w(114).text_size(FontSizeToken::Sm))
        .child(el().w(300).child(keyed(
            move |r| r.get(model.editing) == Some(id),
            move |b, editing| -> View {
                if *editing {
                    let source = draft_source.clone();
                    let current = b.memo(move |r| source.read(r));
                    let initial = b.sample(current).expect("mounted row has a file");
                    let draft = b.signal(initial);
                    input()
                        .test_id(EDITOR)
                        .label(format!("File name in {}", caption(id)))
                        .h(32)
                        .w(rel(1.0))
                        .px(sp(2.0))
                        .border(1)
                        .value(move |r: &mut ReadCx<'_>| r.with(draft, |file| file.name.clone()))
                        .autofocus()
                        .on_input(move |w, name| {
                            w.update(model.edits, |n| *n += 1);
                            w.update(draft, |file| file.name = name.to_owned());
                        })
                        .on_submit(move |w| {
                            let submitted = w.get(draft);
                            model.submit(w, id, submitted);
                        })
                        .on_blur(move |w| model.cancel(w, id))
                        .on_key(move |w, key| {
                            if key.key == Key::Escape {
                                model.cancel(w, id);
                                true
                            } else {
                                false
                            }
                        })
                        .into_view()
                } else {
                    {
                        let file = file.clone();
                        text(move |r: &mut ReadCx<'_>| file.with(r, |file| file.map(|f| f.name.clone()).unwrap_or_default())).into_view()
                    }
                }
            },
        )))
        .child(show(
            move |r| id == B && r.get(model.editing) != Some(B),
            move |_| button("rename-start", "Rename B", move |w| model.begin(w, B)),
        ))
}

/// Drives a disposable in-memory directory and its retained row editor.
#[ui]
pub fn view(cx: &mut BuildCx<'_>) -> El {
    let model = Directory {
        files: cx.list(),
        selected: cx.signal(Vec::new()),
        editing: cx.signal(None),
        edits: cx.signal(0),
        attempts: cx.signal(0),
        saves: cx.signal(0),
        status: cx.signal("Directory ready".into()),
    };
    cx.spawn("seed directory", move |task| async move {
        task.write(|w| w.reconcile_list(model.files, directory()));
    });

    col()
        .size(rel(1.0))
        .p(sp(6.0))
        .gap(sp(3.0))
        .bg(ColorToken::Background)
        .on_command("rename.reorder", move |w| {
            let keys = w.keys(model.files);
            if let Some(first) = keys.first() {
                w.move_row(model.files, first, 1);
            }
        })
        .on_command("rename.refresh", move |w| {
            let rows = w.keys(model.files).into_iter().filter_map(|key| w.get_row(model.files, &key).map(|value| (key, value))).collect();
            w.reconcile_list(model.files, rows);
        })
        .on_command("rename.external", move |w| {
            w.update_row(model.files, &B, |file| {
                file.name = "External B.txt".into();
                file.revision += 1;
            });
            w.set(model.status, "Directory B was renamed externally".into());
        })
        .on_command("rename.replace", move |w| {
            if let Some(rank) = w.keys(model.files).iter().position(|id| *id == B) {
                w.remove_row(model.files, &B);
                w.insert_row(model.files, rank, C, File { name: SEED.into(), revision: 0 });
                w.update(model.selected, |ids| ids.retain(|id| *id != B));
                model.begin(w, C);
            }
        })
        .on_command("rename.reset", move |w| {
            w.set(model.editing, None);
            w.reconcile_list(model.files, directory());
            w.set(model.selected, Vec::new());
            w.set(model.edits, 0);
            w.set(model.attempts, 0);
            w.set(model.saves, 0);
            w.set(model.status, "Directory ready".into());
            w.reveal_row("rename-list", A);
        })
        .on_command("rename.scroll-away", |w| w.reveal_row("rename-list", 30))
        .on_command("rename.scroll-back", |w| w.reveal_row("rename-list", A))
        .child(text("Inline file rename").text_size(FontSizeToken::Xl).text_weight(600))
        .child(
            row().gap(sp(3.0)).child(button("rename-select", "Select A and B", move |w| w.set(model.selected, vec![A, B]))).child(button(
                "rename-outside",
                "Leave editor",
                |w| w.focus("rename-outside"),
            )),
        )
        .child(col().test_id("rename-list").role(Role::List).grow(1.0).min_h(0.0).overflow_y(Overflow::Scroll).child(virtual_each(
            model.files,
            VirtualOptions { estimated_row_height: ROW_HEIGHT, ..Default::default() },
            move |_, id, file| file_row(*id, file, model),
        )))
        .child(text(model.status).test_id("rename-status").role(Role::Status).live(Live::Polite))
        .child(
            text(move |r: &mut ReadCx<'_>| {
                format!("Edits: {} · Attempts: {} · Saves: {}", r.get(model.edits), r.get(model.attempts), r.get(model.saves))
            })
            .test_id("rename-counts"),
        )
        .child(
            text(move |r: &mut ReadCx<'_>| {
                [A, B, C]
                    .into_iter()
                    .filter_map(|id| r.get_row(model.files, &id).map(|file| format!("{id}: {}", file.name)))
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
            .test_id("rename-records")
            .text_size(FontSizeToken::Sm),
        )
}
