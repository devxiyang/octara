//! Reveal requests use the mounted viewport and the settled business key.

use octara::app::Event;
use octara::prelude::*;
use octara::view::IntoRows;
use std::cell::Cell;
use std::fmt;
use std::rc::Rc;

const ROWS: u32 = 512;
const FAR_ROW: u32 = 240;
const NEXT_ROW: u32 = 416;
const ESTIMATED_HEIGHT: f32 = 24.0;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct FileKey(u32);

impl fmt::Display for FileKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("file")
    }
}

#[derive(Clone, Copy)]
struct File {
    key: FileKey,
    order: u32,
    height: f32,
}

impl File {
    fn new(number: u32) -> Self {
        let height = if number.is_multiple_of(2) { 112.0 } else { 28.0 };
        Self { key: FileKey(number), order: number * 2, height }
    }
}

fn snapshot() -> Vec<(FileKey, File)> {
    (0..ROWS).map(|number| (FileKey(number), File::new(number))).collect()
}

fn row_id(list: &str, number: u32) -> String {
    format!("{list}-file-{number}")
}

fn file_row(list: &str, key: FileKey, file: ReadSignal<File>) -> El {
    el().id(row_id(list, key.0))
        .role(Role::ListItem)
        .label(format!("File {}", key.0))
        .focusable()
        .flex_shrink_0()
        .style_with(move |patch| patch.h(file.with(|file| file.height)))
}

fn file_rows<S: IntoRows<FileKey, File> + 'static>(id: &'static str, source: impl Fn() -> S + 'static) -> View {
    virtual_each(source, |_| ESTIMATED_HEIGHT, move |key, file| file_row(id, *key, file))
}

fn file_viewport(id: &'static str) -> El {
    col().id(id).w(220).h(160).gap_y(4).flex_shrink_0().overflow_y_scroll()
}

fn offset(window: &Window, id: &str) -> f32 {
    let kernel = window.kernel();
    kernel.scroll_offset(kernel.find(id).unwrap()).1
}

fn target(window: &Window, id: &str) -> String {
    window.inspect().find(id).unwrap().target.clone()
}

fn assert_visible(window: &Window, list: &str, number: u32) {
    let kernel = window.kernel();
    let viewport = kernel.find(list).unwrap();
    let id = row_id(list, number);
    let row = kernel.find(&id).unwrap_or_else(|| panic!("{id} did not mount"));
    let top = kernel.shown_rect(viewport).1;
    let bottom = top + kernel.client_size(viewport).1;
    let (_, y, _, height) = kernel.shown_rect(row);
    assert!(y >= top - 0.5 && y + height <= bottom + 0.5, "{id} at {y} with height {height} must fit in {top}..{bottom}");
}

fn assert_clean(window: &Window) {
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

#[test]
fn revealing_an_unmounted_row_uses_its_measured_height_and_allows_focus_after_mount() {
    let middle_was_built = Rc::new(Cell::new(false));
    let observed = middle_was_built.clone();
    let mut window = Window::headless(move || {
        let observed = observed.clone();
        on_command("reveal", || assert!(reveal_row("files", FileKey(FAR_ROW))));
        on_command("reveal-and-focus", || {
            assert!(reveal_row("files", FileKey(FAR_ROW)));
            assert!(reveal_row("files", FileKey(NEXT_ROW)));
            assert!(reveal_row("other", FileKey(FAR_ROW)));
            focus(&row_id("files", NEXT_ROW));
        });
        col().child(input().id("search").label("Search files").w(220)).child(
            row()
                .child(file_viewport("files").child(virtual_each(
                    snapshot,
                    |_| ESTIMATED_HEIGHT,
                    move |key, file| {
                        if key.0 == FAR_ROW / 2 {
                            observed.set(true);
                        }
                        file_row("files", *key, file)
                    },
                )))
                .child(file_viewport("other").child(file_rows("other", snapshot))),
        )
    });
    window.tap("search").unwrap();
    window.dispatch(Event::ScrollTo("other".into(), 0.0, 60.0)).unwrap();
    let other_offset = offset(&window, "other");
    assert!(!window.kernel().exists(&row_id("files", FAR_ROW)));

    window.invoke("reveal").unwrap();

    assert_visible(&window, "files", FAR_ROW);
    assert!(offset(&window, "files") > 0.0);
    assert_eq!(offset(&window, "other"), other_offset);
    assert_eq!(window.inspect().focus.as_deref(), Some("search"));
    assert!(!middle_was_built.get(), "the jump leaves intervening rows unbuilt");
    assert!(!window.kernel().exists(&row_id("files", NEXT_ROW)));

    window.invoke("reveal-and-focus").unwrap();

    assert_visible(&window, "files", NEXT_ROW);
    assert_visible(&window, "other", FAR_ROW);
    assert_eq!(window.inspect().focus, Some(row_id("files", NEXT_ROW)));
    assert_clean(&window);
}

#[test]
fn revealing_a_reordered_snapshot_row_preserves_its_mount() {
    let mut window = Window::headless(|| {
        let files = state("files", snapshot);
        on_command("move-file", move || {
            assert!(reveal_row("files", FileKey(0)));
            files.update(|files| {
                let moved = files.remove(0);
                files.insert(FAR_ROW as usize, (FileKey(ROWS), File::new(ROWS)));
                files.insert(FAR_ROW as usize, moved);
            });
        });
        file_viewport("files").child(file_rows("files", move || files.get()))
    });
    let before = target(&window, &row_id("files", 0));

    window.invoke("move-file").unwrap();

    assert_visible(&window, "files", 0);
    assert!(offset(&window, "files") > 0.0);
    assert_eq!(target(&window, &row_id("files", 0)), before);
    assert_clean(&window);
}

#[test]
fn reveal_uses_same_handler_insertions_and_discards_requests_for_removed_rows() {
    let mut window = Window::headless(|| {
        let files = state_list("files", || (0..ROWS).map(File::new).collect(), |file| file.key);
        let sorted = files.read_only().sort_by(|file| file.order);
        on_command("insert-file", move || {
            assert!(reveal_row("files", FileKey(ROWS)));
            files.push(File { order: FAR_ROW * 2 + 1, ..File::new(ROWS) });
        });
        on_command("remove-file", move || {
            assert!(reveal_row("files", FileKey(NEXT_ROW)));
            files.remove(&FileKey(NEXT_ROW));
        });
        on_command("restore-file", move || files.push(File::new(NEXT_ROW)));
        file_viewport("files").child(file_rows("files", move || sorted.clone()))
    });

    window.invoke("insert-file").unwrap();
    assert_visible(&window, "files", ROWS);
    let before = offset(&window, "files");

    for command in ["remove-file", "restore-file"] {
        window.invoke(command).unwrap();
        assert_eq!(offset(&window, "files"), before);
        assert!(!window.kernel().exists(&row_id("files", NEXT_ROW)));
    }
    assert_clean(&window);
}

#[test]
fn a_queued_reveal_does_not_follow_a_replaced_viewport_name() {
    let mut window = Window::headless(|| {
        let replacement = state("replacement", || false);
        on_command("replace-viewport", move || {
            assert!(reveal_row("files", FileKey(FAR_ROW)));
            replacement.set(true);
        });
        on_command("reveal", || assert!(reveal_row("files", FileKey(FAR_ROW))));
        col().child(keyed(move || replacement.get(), |_| file_viewport("files").child(file_rows("files", snapshot))))
    });
    let original = target(&window, "files");

    window.invoke("replace-viewport").unwrap();

    assert_ne!(target(&window, "files"), original);
    assert_eq!(offset(&window, "files"), 0.0);
    assert!(!window.kernel().exists(&row_id("files", FAR_ROW)));
    window.invoke("reveal").unwrap();
    assert_visible(&window, "files", FAR_ROW);
    assert_clean(&window);
}

#[test]
fn retiring_the_requester_discards_its_reveal_without_replacing_the_list() {
    let mut window = Window::headless(|| {
        let shown = state("requester", || true);
        col()
            .child(show(
                move || shown.get(),
                move || {
                    el().id("retire").size_12().on_press(move || {
                        assert!(reveal_row("files", FileKey(FAR_ROW)));
                        shown.set(false);
                    })
                },
            ))
            .child(file_viewport("files").child(file_rows("files", snapshot)))
    });
    let original = target(&window, "files");

    window.tap("retire").unwrap();

    assert!(!window.kernel().exists("retire"));
    assert_eq!(target(&window, "files"), original);
    assert_eq!(offset(&window, "files"), 0.0);
    assert!(!window.kernel().exists(&row_id("files", FAR_ROW)));
    assert_clean(&window);
}

#[test]
fn reveal_requires_one_mounted_viewport_and_one_matching_virtual_region() {
    let mut window = Window::headless(|| {
        on_command("refuse", || {
            for id in ["missing", "ordinary", "duplicate", "multiple-regions"] {
                assert!(!reveal_row(id, FileKey(FAR_ROW)), "{id} cannot accept a row reveal");
            }
            assert!(!reveal_row("files", FAR_ROW), "the key type must match");
        });
        let content = || el().h(400).flex_shrink_0();
        col()
            .child(
                row()
                    .child(file_viewport("files").child(file_rows("files", snapshot)))
                    .child(file_viewport("ordinary").child(content()))
                    .child(file_viewport("duplicate").child(file_rows("duplicate", snapshot))),
            )
            .child(
                row()
                    .child(file_viewport("duplicate").child(content()))
                    .child(file_viewport("multiple-regions").child(file_rows("first", snapshot)).child(file_rows("second", snapshot))),
            )
    });

    window.invoke("refuse").unwrap();

    let kernel = window.kernel();
    let duplicates = kernel.find_all("duplicate");
    assert_eq!(duplicates.len(), 2);
    let others = ["files", "ordinary", "multiple-regions"].map(|id| kernel.find(id).unwrap());
    for node in duplicates.into_iter().chain(others) {
        assert_eq!(kernel.scroll_offset(node), (0.0, 0.0));
    }
    drop(kernel);
    assert_clean(&window);
}
