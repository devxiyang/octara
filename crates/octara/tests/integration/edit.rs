//! Text editing owned by the runtime: carets by grapheme and word, selections, clipboard,
//! undo, composition at the caret, pointer selection, and the platform text protocol.

use octara::accesskit;
use octara::app::{Phase, PointerEvent, TargetPlatform, byte_offset, utf16_offset};
use octara::prelude::*;
use octara_primitives::text::TextEdit;

fn app(initial: &'static str) -> Window {
    let mut app = Window::headless(move || {
        let v = state("v", move || initial.to_owned());
        col()
            .size_full()
            .child(text_input().id("field").w(px(200.0)).value(move || v.get()).on_input(move |s| v.set(s)))
            .child(text_input().id("narrow").w(px(60.0)).value("").on_input(|_| {}))
            .into()
    });
    app.app().set_target_platform(TargetPlatform::MacOs);
    app.tap("field").unwrap();
    app
}

fn value(app: &Window) -> String {
    app.kernel().value_of("field").unwrap()
}

fn selection(app: &Window) -> std::ops::Range<usize> {
    app.text_field().unwrap().selection
}

fn keys(app: &mut Window, chords: &[&str]) {
    for c in chords {
        app.press_key(c).unwrap();
    }
}

fn native_input(app: &Window) -> octara::platform::TextInput {
    app.platform_window().as_any().downcast_ref::<octara::platform::HeadlessWindow>().unwrap().text_input.clone().unwrap()
}

#[test]
fn native_reconversion_edits_the_displayed_document_and_commits_once() {
    use std::cell::Cell;
    use std::rc::Rc;
    let calls = Rc::new(Cell::new(0));
    let edits = calls.clone();
    let mut app = Window::headless(move || {
        let value = state("value", || "A😀LatinZ".to_owned());
        let edits = edits.clone();
        text_input()
            .id("field")
            .w(240)
            .value(move || value.get())
            .on_input(move |text| {
                edits.set(edits.get() + 1);
                value.set(text);
            })
            .into()
    });
    app.tap("field").unwrap();
    app.set_selection(10, 5);
    let input = native_input(&app);
    let mut mirror = input.document.unwrap();
    let mark = TextEdit::Mark { text: "nihao".into(), selection: Some(5..5), replacement: None };
    assert!(app.apply_text_edit(input.id, &mirror, &mark));
    mirror.apply(&mark);
    assert_eq!(native_input(&app).document.as_ref(), Some(&mirror));
    assert_eq!(value(&app), "A😀LatinZ");
    assert_eq!(calls.get(), 0, "marked text does not call the app's on_input");

    let partial = TextEdit::Mark { text: "你".into(), selection: Some(3..3), replacement: Some(6..8) };
    assert!(app.apply_text_edit(input.id, &mirror, &partial));
    mirror.apply(&partial);
    assert_eq!(mirror.text(), "A😀n你aoZ");
    assert_eq!(native_input(&app).document.as_ref(), Some(&mirror));
    assert_eq!(value(&app), "A😀LatinZ");
    let feedback = app.inspect().text_input.unwrap();
    assert_eq!(feedback.display_text, "A😀n你aoZ");
    assert_eq!(feedback.marked, Some([6, 9]));
    assert_eq!(feedback.display_selection_utf16, Some([5, 5]));
    let geometry = native_input(&app).geometry.unwrap();
    let (_, suffix) = geometry.first_rect(11..11).unwrap();
    assert_eq!(app.kernel().input_offset_at(input.id, suffix.x, suffix.y + suffix.h / 2.0, false), Some(10));
    let commit = TextEdit::Insert { text: "界".into(), replacement: Some(1..5) };
    assert!(app.apply_text_edit(input.id, &mirror, &commit));
    assert_eq!(value(&app), "A界n你aoZ", "an explicit replacement is in displayed coordinates, retaining the rest of the draft");
    assert_eq!(calls.get(), 1);
    app.press_key("mod+z").unwrap();
    assert_eq!(value(&app), "A😀LatinZ", "all preedit updates and the final replacement form one undo step");
    assert_eq!(selection(&app), 5..10);
    assert!(app.text_field().unwrap().reversed);
}

#[test]
fn native_cancel_restores_selection_but_unmark_accepts_the_draft() {
    let mut app = app("prefix Latin suffix");
    app.set_selection(12, 7);
    let input = native_input(&app);
    let before = input.document.unwrap();
    let mark = TextEdit::Mark { text: "中文".into(), selection: Some(0..6), replacement: Some(7..12) };
    assert!(app.apply_text_edit(input.id, &before, &mark));
    let composing = native_input(&app).document.unwrap();
    assert!(app.apply_text_edit(input.id, &composing, &TextEdit::Cancel));
    assert_eq!(native_input(&app).document.as_ref(), Some(&before));
    assert!(app.text_field().unwrap().reversed);
    assert!(app.apply_text_edit(input.id, &before, &mark));
    assert!(app.apply_text_edit(input.id, &composing, &TextEdit::Unmark));
    assert_eq!(value(&app), "prefix 中文 suffix");
    assert_eq!(selection(&app), 7..13);
    assert!(app.text_field().unwrap().composition.is_empty());
}

#[test]
fn queued_native_edits_reject_a_changed_owner_or_asynchronously_changed_value() {
    use std::time::Duration;
    let mut app = Window::headless(|| {
        let value = state("value", || "original".to_owned());
        spawn(async move {
            sleep(Duration::from_millis(10)).await;
            value.set("async update".into());
        });
        col()
            .child(text_input().id("field").w(240).value(move || value.get()).on_input(move |text| value.set(text)))
            .child(text_input().id("other").w(240))
            .into()
    });
    app.tap("field").unwrap();
    let input = native_input(&app);
    let before = input.document.unwrap();
    let commit = TextEdit::Insert { text: "late".into(), replacement: None };
    app.tap("other").unwrap();
    assert!(!app.apply_text_edit(input.id, &before, &commit));
    assert_eq!(value(&app), "original");
    app.dispatch(Event::Focus("field".into())).unwrap();
    let before = native_input(&app).document.unwrap();
    let mark = TextEdit::Mark { text: "ni".into(), selection: Some(2..2), replacement: None };
    assert!(app.apply_text_edit(input.id, &before, &mark));
    let composing = native_input(&app).document.unwrap();
    assert!(composing.marked().is_some());
    app.app().advance(Duration::from_millis(10));
    assert_eq!(native_input(&app).id, input.id, "the field still owns input after an asynchronous model update");
    let updated = native_input(&app).document.unwrap();
    assert_eq!(updated.text(), "async update", "the newer model value replaces the entire old displayed draft");
    assert!(updated.marked().is_none(), "the retired draft cannot overwrite the newer value on its next commit");
    assert!(app.text_field().unwrap().composition.is_empty());
    assert!(!app.apply_text_edit(input.id, &before, &commit));
    assert!(!app.apply_text_edit(input.id, &composing, &commit));
    assert_eq!(value(&app), "async update");
}

#[test]
fn native_geometry_reports_marked_offsets_and_the_actual_first_line_fragment() {
    let mut app = Window::headless(|| text_input().id("field").w(80).multiline().value("A😀Z").into());
    app.tap("field").unwrap();
    app.set_selection(1, 5);
    app.event(InputEvent::Preedit { text: "中文中文中文中文".into(), selection: Some((3, 3)) });
    let input = native_input(&app);
    let document = input.document.unwrap();
    let geometry = input.geometry.unwrap();
    assert!(geometry.lines.len() > 1);
    let marked = document.marked().unwrap();
    let (actual, rect) = geometry.first_rect(marked.clone()).unwrap();
    assert!(actual.start <= marked.start && actual.end < marked.end, "only the first line fragment is returned");
    assert!(rect.w > 0.0 && rect.h > 0.0);
    let (line, stop) = geometry
        .lines
        .iter()
        .find_map(|line| line.stops.iter().find(|(b, _)| *b > marked.start && *b < marked.end && *b < line.end).map(|stop| (line, stop)))
        .unwrap();
    assert_eq!(geometry.index_at(geometry.origin.0 + stop.1, geometry.origin.1 + line.top + line.height / 2.0), Some(stop.0));
    let (actual, caret) = geometry.first_rect(stop.0..stop.0).unwrap();
    assert_eq!(actual, stop.0..stop.0);
    assert_eq!(caret.w, 0.0, "native insertion point geometry has zero width");
}

#[test]
fn native_layout_predicts_fresh_preedit_geometry_before_the_core_receives_its_edit() {
    for multiline in [false, true] {
        let mut app = Window::headless(move || {
            let value = state("value", || "A😀Z".to_owned());
            let field = text_input().id("field").w(80).h(48).value(move || value.get()).on_input(move |text| value.set(text));
            (if multiline { field.multiline() } else { field }).into()
        });
        app.tap("field").unwrap();
        app.set_selection(6, 6);
        let before = native_input(&app);
        let expected = before.document.unwrap();
        let text = if multiline { "中😀nihao 中😀nihao\n中😀nihao 中😀nihao" } else { "nihaonihaonihaonihaonihao" };
        let edit = TextEdit::Mark { text: text.into(), selection: Some(text.len()..text.len()), replacement: None };
        let mut mirror = expected.clone();
        mirror.apply(&edit);
        let predicted = before.layout.unwrap().layout(&mirror).unwrap();
        let end = mirror.text().len();
        let (actual, last_o) = predicted.first_rect(end - 1..end).unwrap();
        assert_eq!(actual, end - 1..end);
        assert!(last_o.w > 0.0 && last_o.h > 0.0, "the input method can locate its new last character before a core frame");
        assert_eq!(value(&app), "A😀Z", "synchronous native layout does not apply the queued edit");
        assert!(app.text_field().unwrap().composition.is_empty());
        assert!(app.apply_text_edit(before.id, &expected, &edit));
        let painted = native_input(&app);
        assert_eq!(painted.geometry.as_ref(), Some(&predicted), "native layout and the field use the same shaping, wrapping and scroll");
        let (x, y) = predicted.origin;
        if multiline {
            assert!(predicted.lines.len() > 2);
            assert!(y < painted.caret.y, "the multiline draft scrolls to reveal its end");
        } else {
            assert!(x < 0.0, "the single-line draft scrolls horizontally to reveal its end");
        }

        // The text is unchanged, but moving the composition cursor must reveal its start.
        let at_start = TextEdit::Mark { text: text.into(), selection: Some(0..0), replacement: None };
        let expected = painted.document.unwrap();
        let mut mirror = expected.clone();
        mirror.apply(&at_start);
        assert_eq!(mirror.text(), expected.text());
        let predicted_start = painted.layout.unwrap().layout(&mirror).unwrap();
        assert_ne!(predicted_start.origin, predicted.origin, "geometry cache keys must include selection, not only text");
        assert!(app.apply_text_edit(before.id, &expected, &at_start));
        assert_eq!(native_input(&app).geometry.as_ref(), Some(&predicted_start));
    }
}

#[test]
fn native_and_committed_fields_agree_after_resize_scroll_and_composition_transitions() {
    for multiline in [false, true] {
        let mut app = Window::headless(move || {
            let value = state("value", || "中文😀 one two\n三四 five six\n七八 nine ten".to_owned());
            let field = text_input().id("field").w_full().h(64).p_2().value(value).on_input(move |s| value.set(s));
            (if multiline { field.multiline() } else { field }).into()
        });
        app.tap("field").unwrap();
        app.set_selection(value(&app).len(), value(&app).len());
        for width in [90.0, 240.0, 110.0] {
            app.resize(width, 160.0);
            if multiline {
                let n = app.kernel().find("field").unwrap();
                let (x, y, ..) = app.kernel().rect(n);
                app.wheel(x + 12.0, y + 12.0, 0.0, -20.0);
            }
            let current = native_input(&app);
            let document = current.document.unwrap();
            let predicted = current.layout.unwrap().layout(&document).unwrap();
            assert_eq!(Some(&predicted), current.geometry.as_ref(), "query preserves resize and manual scroll");
            let n = app.kernel().find("field").unwrap();
            let scroll = app.kernel().scroll_offset(n);
            let caret = app.kernel().caret(n);
            let generation = app.kernel().paint_generation();
            app.inspect();
            app.display_list();
            app.kernel().accessibility_tree(Some(n), 1.0);
            app.kernel().input_offset_at(n, current.caret.x, current.caret.y, false);
            assert_eq!(app.kernel().scroll_offset(n), scroll);
            assert_eq!(app.kernel().caret(n), caret);
            assert_eq!(app.kernel().paint_generation(), generation, "observation does not settle editing state");
            assert_eq!(native_input(&app).geometry, current.geometry);
            for edit in [
                TextEdit::Mark { text: "中文😀组合".into(), selection: Some(3..7), replacement: None },
                TextEdit::Mark { text: "中文😀组合".into(), selection: None, replacement: None },
                TextEdit::Cancel,
                TextEdit::Mark { text: "你好😀".into(), selection: Some(0..0), replacement: None },
                TextEdit::Unmark,
            ] {
                let before = native_input(&app);
                let expected = before.document.unwrap();
                let mut mirror = expected.clone();
                mirror.apply(&edit);
                let predicted = before.layout.unwrap().layout(&mirror).unwrap();
                assert!(app.apply_text_edit(before.id, &expected, &edit));
                assert_eq!(native_input(&app).geometry.as_ref(), Some(&predicted));
            }
        }
        let before = native_input(&app);
        let expected = before.document.unwrap();
        let edit = TextEdit::Insert { text: String::new(), replacement: Some(0..expected.text().len()) };
        let mut mirror = expected.clone();
        mirror.apply(&edit);
        let predicted = before.layout.unwrap().layout(&mirror).unwrap();
        assert!(app.apply_text_edit(before.id, &expected, &edit));
        assert_eq!(native_input(&app).geometry.as_ref(), Some(&predicted));
        assert_eq!(predicted.lines.len(), 1, "empty fields retain one insertion line");
        assert!(predicted.first_rect(0..0).is_some());
        assert!(app.app().diagnostics().is_empty());
    }
}

#[test]
fn carets_move_by_grapheme_and_word() {
    let mut app = app("");
    app.type_text("cafe\u{301} 👍🏽 ok");
    keys(&mut app, &["left", "left", "left"]);
    let after = "cafe\u{301} 👍🏽".len();
    assert_eq!(selection(&app), after..after);
    keys(&mut app, &["left"]);
    let before = "cafe\u{301} ".len();
    assert_eq!(selection(&app), before..before, "the thumbs-up with its skin tone is one step");
    keys(&mut app, &["home", "right", "right", "right", "right"]);
    assert_eq!(selection(&app).start, "cafe\u{301}".len(), "e + combining acute is one step");
    keys(&mut app, &["alt+left"]);
    assert_eq!(selection(&app).start, 0);
    keys(&mut app, &["alt+right", "alt+right"]);
    assert_eq!(&value(&app)[..selection(&app).start], "cafe\u{301} 👍🏽 ok", "words skip the emoji and spaces");
    keys(&mut app, &["cmd+left"]);
    assert_eq!(selection(&app).start, 0);
}

#[test]
fn typing_replaces_the_selection_and_deletes_by_word() {
    let mut app = app("hello brave world");
    keys(&mut app, &["end", "alt+shift+left"]);
    assert_eq!(selection(&app), 12..17);
    app.type_text("there");
    assert_eq!(value(&app), "hello brave there");
    keys(&mut app, &["alt+backspace"]);
    assert_eq!(value(&app), "hello brave ");
    keys(&mut app, &["home", "delete"]);
    assert_eq!(value(&app), "ello brave ");
    // Off Apple platforms words move with Control.
    app.app().set_target_platform(TargetPlatform::Windows);
    keys(&mut app, &["ctrl+delete"]);
    assert_eq!(value(&app), " brave ");
}

#[test]
fn cut_copy_paste() {
    let mut app = app("one two");
    keys(&mut app, &["cmd+a", "cmd+c"]);
    assert_eq!(app.clipboard_text().as_deref(), Some("one two"));
    keys(&mut app, &["end", "alt+shift+left", "cmd+x"]);
    assert_eq!(value(&app), "one ");
    assert_eq!(app.clipboard_text().as_deref(), Some("two"));
    keys(&mut app, &["home", "cmd+v"]);
    assert_eq!(value(&app), "twoone ");
    assert_eq!(selection(&app), 3..3);
    app.app().run(|| write_to_clipboard("a\nb"));
    keys(&mut app, &["cmd+v"]);
    assert_eq!(value(&app), "twoa bone ", "one line: breaks become spaces");
}

#[test]
fn undo_takes_back_a_run_of_typing_at_once() {
    let mut app = app("");
    for c in ["h", "e", "l", "l", "o"] {
        app.press_key(c).unwrap();
    }
    keys(&mut app, &["space"]);
    app.type_text("you");
    keys(&mut app, &["backspace", "backspace"]);
    assert_eq!(value(&app), "hello y");
    keys(&mut app, &["cmd+z"]);
    assert_eq!(value(&app), "hello you", "the deletions were one step");
    keys(&mut app, &["cmd+z"]);
    assert_eq!(value(&app), "", "the typing was one step");
    keys(&mut app, &["cmd+shift+z"]);
    assert_eq!(value(&app), "hello you");
    assert_eq!(selection(&app), 9..9);
    // Moving the caret ends a run.
    keys(&mut app, &["left"]);
    app.type_text("!");
    keys(&mut app, &["end"]);
    app.type_text("?");
    keys(&mut app, &["cmd+z"]);
    assert_eq!(value(&app), "hello yo!u");
}

#[test]
fn a_value_the_app_refuses_keeps_the_caret_in_range() {
    let mut app = Window::headless(|| col().child(text_input().id("fixed").w(px(200.0)).value("abc").on_input(|_| {})).into());
    app.tap("fixed").unwrap();
    app.press_key("end").unwrap();
    app.type_text("zzzz");
    assert_eq!(app.kernel().value_of("fixed").unwrap(), "abc");
    let f = app.text_field().unwrap();
    assert!(f.selection.end <= 3, "{:?}", f.selection);
}

#[test]
fn composition_shows_at_the_caret_and_replaces_the_selection() {
    let mut app = app("ab");
    keys(&mut app, &["home", "right"]);
    let before = app.text_field().unwrap().caret.0;
    app.event(InputEvent::Composition(("ni").into()));
    let f = app.text_field().unwrap();
    assert_eq!((f.text.as_str(), f.composition.as_str()), ("ab", "ni"));
    assert!(f.caret.0 > before, "the caret follows the composition");
    app.type_text("你");
    assert_eq!(value(&app), "a你b");
    keys(&mut app, &["shift+right"]);
    app.event(InputEvent::Composition(("x").into()));
    assert_eq!(value(&app), "a你b", "preedit replaces the selection on screen without committing an edit");
    app.type_text("y");
    assert_eq!(value(&app), "a你y");
}

#[test]
fn cancelling_preedit_preserves_the_selected_value_and_undo_history() {
    let mut app = app("before selected after");
    app.set_selection(7, 15);
    app.event(InputEvent::Preedit { text: "候选".into(), selection: Some((0, 6)) });
    let composing = app.text_field().unwrap();
    assert_eq!(composing.text, "before selected after");
    assert_eq!(composing.selection, 7..15);
    assert_eq!(composing.composition_selection, Some(0..6));
    app.event(InputEvent::Preedit { text: String::new(), selection: None });
    let cancelled = app.text_field().unwrap();
    assert_eq!(cancelled.text, "before selected after");
    assert_eq!(cancelled.selection, 7..15);
    assert!(cancelled.composition.is_empty());
    app.press_key("cmd+z").unwrap();
    assert_eq!(value(&app), "before selected after", "uncommitted composition created no undo step");

    app.event(InputEvent::Preedit { text: "ni".into(), selection: Some((2, 2)) });
    app.type_text("你");
    assert_eq!(value(&app), "before 你 after");
    app.press_key("cmd+z").unwrap();
    assert_eq!(value(&app), "before selected after", "the committed replacement is one edit");
    assert_eq!(selection(&app), 7..15);
}

#[test]
fn the_preedit_cursor_and_selection_follow_input_method_feedback() {
    let mut app = app("a");
    app.press_key("end").unwrap();
    let before = app.text_field().unwrap().caret.0;
    app.event(InputEvent::Preedit { text: "nihao".into(), selection: Some((0, 0)) });
    assert_eq!(app.text_field().unwrap().caret.0, before);
    app.event(InputEvent::Preedit { text: "nihao".into(), selection: Some((2, 2)) });
    let middle = app.text_field().unwrap();
    assert_eq!(middle.composition_selection, Some(2..2));
    assert!(middle.caret.0 > before);
    app.event(InputEvent::Preedit { text: "nihao".into(), selection: Some((5, 5)) });
    let end = app.text_field().unwrap();
    assert!(end.caret.0 > middle.caret.0, "same preedit text with a different cursor updates its geometry");
    assert!(end.caret_visible);
    app.event(InputEvent::Preedit { text: "nihao".into(), selection: None });
    let hidden = app.text_field().unwrap();
    assert_eq!(hidden.composition_selection, None);
    assert!(!hidden.caret_visible, "the input method asked to hide its cursor");
    let id = app.kernel().find("field").unwrap();
    assert!(!app.display_list().quads().any(|q| q.source == id && q.rect.w <= 1.1 && q.rect.h > 10.0), "no caret is painted");
    app.event(InputEvent::Preedit { text: "nihao".into(), selection: Some((1, 4)) });
    assert_eq!(app.text_field().unwrap().composition_selection, Some(1..4));
    assert!(!app.text_field().unwrap().caret_visible, "a selected preedit segment is highlighted instead of a caret");
    assert_eq!(value(&app), "a");
}

#[test]
fn preedit_ranges_are_clamped_to_utf8_boundaries_and_ordered() {
    let mut app = app("");
    app.event(InputEvent::Preedit { text: "你🙂好".into(), selection: Some((usize::MAX, 4)) });
    assert_eq!(app.text_field().unwrap().composition_selection, Some(3..10));
    let state = app.inspect().text_input.unwrap();
    assert_eq!(state.composition_selection, Some([3, 10]));
    assert_eq!(state.composition_selection_utf16, Some([1, 4]));
    app.event(InputEvent::Preedit { text: "你🙂好".into(), selection: Some((2, 8)) });
    assert_eq!(app.text_field().unwrap().composition_selection, Some(0..7));
    app.event(InputEvent::Composition("你🙂好".into()));
    assert_eq!(app.text_field().unwrap().composition_selection, Some(10..10), "legacy composition keeps its cursor at the end");
}

#[test]
fn moving_focus_cancels_the_old_fields_composition_and_reports_the_new_owner() {
    let mut app = app("original");
    app.set_selection(2, 6);
    app.event(InputEvent::Preedit { text: "pinyin".into(), selection: Some((6, 6)) });
    let field = app.kernel().find("field").unwrap();
    let narrow = app.kernel().find("narrow").unwrap();
    let host_input = |app: &Window| {
        app.platform_window().as_any().downcast_ref::<octara::platform::HeadlessWindow>().unwrap().text_input.clone().unwrap()
    };
    assert_eq!(host_input(&app).id, field);
    app.tap("narrow").unwrap();
    assert_eq!(host_input(&app).id, narrow, "input-method ownership changes between ordinary text fields too");
    assert!(app.kernel().preedit(field).is_empty());
    assert_eq!(value(&app), "original");
    assert_eq!(app.kernel().selection(field), (2, 6), "cancelling the old composition preserves its selected original text");
    app.dispatch(Event::Focus("field".into())).unwrap();
    let restored = app.text_field().unwrap();
    assert_eq!(restored.text, "original");
    assert_eq!(restored.selection, 2..6);
    assert!(restored.composition.is_empty(), "returning focus cannot revive the old input method's text");
    assert_eq!(host_input(&app).id, field);
}

#[test]
fn pointer_places_the_caret_and_selects() {
    let mut app = app("alpha beta gamma");
    let (x, y, w, h) = {
        let k = app.kernel();
        k.rect(k.find("field").unwrap())
    };
    let cy = y + h / 2.0;
    // Far right: the end. Far left: the start.
    app.click_at(x + w - 2.0, cy);
    assert_eq!(selection(&app), 16..16);
    app.app().advance(std::time::Duration::from_secs(1));
    app.click_at(x + 1.0, cy);
    assert_eq!(selection(&app), 0..0);
    // Shift-click extends; double-click selects a word; triple-click everything.
    app.app().advance(std::time::Duration::from_secs(1));
    app.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Down, x + w - 2.0, cy).modifiers(Modifiers::SHIFT)));
    app.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Up, x + w - 2.0, cy)));
    assert_eq!(selection(&app), 0..16);
    app.app().advance(std::time::Duration::from_secs(1));
    let caret_after_beta = {
        app.press_key("home").unwrap();
        keys(&mut app, &["alt+right", "alt+right"]);
        app.text_field().unwrap().caret.0
    };
    app.click_at(caret_after_beta - 4.0, cy);
    app.click_at(caret_after_beta - 4.0, cy);
    assert_eq!(selection(&app), 6..10, "the word `beta`");
    app.click_at(caret_after_beta - 4.0, cy);
    assert_eq!(selection(&app), 0..16);
    // A drag selects from where it started.
    app.app().advance(std::time::Duration::from_secs(1));
    app.pointer_down(x + 1.0, cy);
    app.pointer_move(caret_after_beta, cy);
    app.pointer_up(caret_after_beta, cy);
    assert_eq!(selection(&app), 0..10);
}

#[test]
fn a_long_line_scrolls_to_keep_the_caret_in_view() {
    let mut app = app("");
    app.tap("narrow").unwrap();
    app.type_text("a fairly long line of text for a narrow field");
    let (x, _, w, _) = {
        let k = app.kernel();
        k.rect(k.find("narrow").unwrap())
    };
    let caret = app.text_field().unwrap().caret.0;
    assert!(caret >= x && caret <= x + w, "caret {caret} inside {x}..{}", x + w);
    app.press_key("home").unwrap();
    let caret = app.text_field().unwrap().caret.0;
    assert!(caret >= x && caret < x + 10.0, "back at the start: {caret}");
}

#[test]
fn the_platform_text_protocol() {
    let mut app = app("héllo 👋");
    let f = app.text_field().unwrap();
    assert_eq!(utf16_offset(&f.text, f.text.len()), 8, "👋 is two UTF-16 units");
    assert_eq!(byte_offset(&f.text, 2), "hé".len());
    assert_eq!(byte_offset(&f.text, 7), "héllo ".len(), "inside a surrogate pair: its start");
    app.set_selection(1, 3);
    assert_eq!(selection(&app), 1..3);
    app.replace_text(1..3, "E");
    assert_eq!(value(&app), "hEllo 👋");
    assert_eq!(selection(&app), 2..2);
}

#[test]
fn assistive_technology_sets_the_value() {
    let mut app = app("old");
    let id = octara::kernel::a11y::a11y_id(app.kernel().find("field").unwrap());
    let done = app.event(InputEvent::Accessibility {
        target: id,
        action: accesskit::Action::SetValue,
        data: Some(accesskit::ActionData::Value("new".into())),
    });
    assert!(done);
    assert_eq!(value(&app), "new");
    assert_eq!(selection(&app), 3..3);
}

/// The glyphs a field draws, as text: one glyph per character with the bundled Latin fonts.
fn drawn_glyphs(app: &Window, id: &str) -> usize {
    let n = app.kernel().find(id).unwrap();
    app.display_list().glyph_runs().filter(|r| r.source == n).map(|r| r.glyphs.len()).sum()
}

#[test]
fn placeholder_is_presentation_not_an_editing_document_or_name() {
    for multiline in [false, true] {
        let mut app = Window::headless(move || {
            let value = state("value", String::new);
            let hint = state("hint", || "Write here".to_owned());
            on_command("hint", move || hint.set("中文提示很长但不改变输入高度\n第二行".into()));
            on_command("clear-hint", move || hint.set(String::new()));
            let field = text_input().id("field").w(100).placeholder(hint).value(value).on_input(move |v| value.set(v));
            (if multiline { field.rows(1, 3) } else { field }).into()
        });
        app.tap("field").unwrap();
        let id = app.kernel().find("field").unwrap();
        let initial = app.inspect();
        let native = native_input(&app);
        let frame = app.kernel().rect(id);
        assert_eq!(initial.find("field").unwrap().placeholder.as_deref(), Some("Write here"));
        assert_eq!(initial.find("field").unwrap().name, None, "a hint never names a control");
        assert!(app.kernel().accessibility_issues().iter().any(|i| i.code == "UNNAMED_CONTROL"));
        let ax = app.accessibility_update();
        let node = &ax.nodes.iter().find(|(n, _)| *n == octara::kernel::a11y::a11y_id(id)).unwrap().1;
        assert_eq!(node.placeholder(), Some("Write here"));
        assert_eq!(node.value(), Some(""));
        assert!(drawn_glyphs(&app, "field") > 0);
        assert!(app.display_list().glyph_runs().filter(|r| r.source == id).all(|r| r.color == Theme::light().get(Token::TextMuted)));
        assert_eq!(native.document.as_ref().unwrap().text(), "");
        assert_eq!(native.geometry.as_ref().unwrap().lines[0].end, 0);
        assert_eq!(app.kernel().input_offset_at(id, frame.0 + 80.0, frame.1 + 8.0, false), Some(0));
        app.invoke("hint").unwrap();
        let changed = app.inspect();
        assert_eq!(changed.text_input, initial.text_input);
        assert_eq!(changed.find("field").unwrap().target, initial.find("field").unwrap().target);
        assert_eq!(app.kernel().rect(id), frame, "hint text never grows the field");
        assert_eq!(native_input(&app).geometry, native.geometry);
        let drawing = app.display_list();
        let mut baselines: Vec<_> =
            drawing.glyph_runs().filter(|r| r.source == id).flat_map(|r| r.glyphs.iter().map(|g| g.y).collect::<Vec<_>>()).collect();
        baselines.sort_by(f32::total_cmp);
        baselines.dedup();
        assert_eq!(baselines.len() > 1, multiline, "single-line hints fit one line; multiline hints wrap and clip");
        app.event(InputEvent::Preedit { text: "ni".into(), selection: Some((0, 2)) });
        assert_eq!(drawn_glyphs(&app, "field"), 2, "composition hides the hint");
        let composing = app.inspect().text_input;
        app.invoke("clear-hint").unwrap();
        assert_eq!(app.inspect().text_input, composing, "hint updates leave composition alone");
        assert_eq!(app.inspect().find("field").unwrap().placeholder, None);
        let ax = app.accessibility_update();
        assert_eq!(ax.nodes.iter().find(|(n, _)| *n == octara::kernel::a11y::a11y_id(id)).unwrap().1.placeholder(), None);
        app.event(InputEvent::Composition(String::new()));
        assert_eq!(drawn_glyphs(&app, "field"), 0);
        app.invoke("hint").unwrap();
        assert!(drawn_glyphs(&app, "field") > 0);
        app.type_text("X");
        assert_eq!(value(&app), "X");
        assert_eq!(drawn_glyphs(&app, "field"), 1);
        app.press_key("mod+z").unwrap();
        assert_eq!(value(&app), "");
        assert!(drawn_glyphs(&app, "field") > 0, "undo restores empty value, never the hint as text");
    }
}

#[test]
fn secure_placeholder_remains_a_hint_and_never_exposes_the_value() {
    let mut app = Window::headless(|| {
        let value = state("value", String::new);
        text_input()
            .id("field")
            .w(180)
            .secure()
            .label("Password")
            .placeholder("Test password")
            .value(value)
            .on_input(move |s| value.set(s))
            .into()
    });
    app.tap("field").unwrap();
    assert!(drawn_glyphs(&app, "field") > 0);
    app.type_text("secret");
    let snapshot = app.inspect();
    assert_eq!(snapshot.text_input, None);
    assert_eq!(snapshot.find("field").unwrap().placeholder.as_deref(), Some("Test password"));
    assert!(!snapshot.to_json().contains("secret"));
    assert_eq!(drawn_glyphs(&app, "field"), 6);
}

#[test]
fn what_a_field_draws_follows_every_edit_at_once() {
    let mut app = app("hello");
    assert_eq!(drawn_glyphs(&app, "field"), 5);
    // Deleting shortens the text: the old shape must not be reused.
    keys(&mut app, &["end", "backspace", "backspace"]);
    assert_eq!(drawn_glyphs(&app, "field"), 3);
    // Replacing a character with another of the same length.
    keys(&mut app, &["shift+left"]);
    app.type_text("X");
    assert_eq!(value(&app), "heX");
    let glyphs_before = app.display_list().glyph_runs().flat_map(|r| r.glyphs.iter().map(|g| g.id).collect::<Vec<_>>()).collect::<Vec<_>>();
    keys(&mut app, &["shift+left"]);
    app.type_text("Y");
    let glyphs_after = app.display_list().glyph_runs().flat_map(|r| r.glyphs.iter().map(|g| g.id).collect::<Vec<_>>()).collect::<Vec<_>>();
    assert_ne!(glyphs_before, glyphs_after, "the new character is drawn, not the old one");
    // A composition moving with the caret.
    keys(&mut app, &["home"]);
    app.event(InputEvent::Composition(("ab").into()));
    assert_eq!(drawn_glyphs(&app, "field"), 5);
    keys(&mut app, &["right"]);
    app.event(InputEvent::Composition(("a").into()));
    assert_eq!(drawn_glyphs(&app, "field"), 4);
}

#[test]
fn the_caret_blinks_and_shows_while_typing() {
    use std::time::Duration;
    let mut app = Window::headless(|| {
        let v = state("v", String::new);
        col()
            .child(text_input().id("f").value(move || v.get()).on_input(move |s| v.set(s)))
            .child(el().id("other").size_8().focusable())
            .into()
    });
    app.tap("f").unwrap();
    let caret = |app: &Window| {
        let f = app.kernel().find("f").unwrap();
        app.display_list().quads().any(|q| q.source == f && q.rect.w <= 2.0)
    };
    assert!(caret(&app), "shown when focused");
    assert!(app.app().next_deadline().is_some(), "a blink is due");
    app.app().advance(Duration::from_millis(550));
    assert!(!caret(&app), "hidden half a second later");
    app.app().advance(Duration::from_millis(500));
    assert!(caret(&app), "and shown again");
    app.app().advance(Duration::from_millis(500));
    assert!(!caret(&app));
    app.type_text("a");
    assert!(caret(&app), "typing shows it at once");
    app.app().advance(Duration::from_millis(300));
    assert!(caret(&app), "and its blink starts over");
    app.tap("other").unwrap();
    assert!(!caret(&app) && app.app().next_deadline().is_none(), "no field focused: no blink");
}

#[test]
fn the_platform_is_told_when_the_caret_blinks() {
    use std::time::Duration;
    let mut app = Window::headless(|| text_input().id("f").into());
    app.tap("f").unwrap();
    let frames = |app: &Window| app.platform_window().as_any().downcast_ref::<octara::platform::HeadlessWindow>().unwrap().frames;
    let before = frames(&app);
    app.app().advance(Duration::from_millis(550));
    assert!(frames(&app) > before, "the blink is presented, not only drawn when asked");
}

#[test]
fn an_inactive_window_shows_no_caret() {
    use std::time::Duration;
    let mut app = Window::headless(|| text_input().id("f").into());
    app.tap("f").unwrap();
    let caret = |app: &Window| {
        let f = app.kernel().find("f").unwrap();
        app.display_list().quads().any(|q| q.source == f && q.rect.w <= 2.0)
    };
    assert!(app.is_active() && caret(&app));
    app.event(InputEvent::Active(false));
    assert!(!app.is_active() && !caret(&app), "another window is active: no caret");
    assert!(app.app().next_deadline().is_none(), "and no blink to wake for");
    app.app().advance(Duration::from_millis(1200));
    app.event(InputEvent::Active(true));
    assert!(caret(&app), "back: shown at once");
}

#[test]
fn rejected_proposal_restores_selection_and_preserves_redo() {
    let mut app = Window::headless(|| {
        let v = state("value", || "abcd".to_owned());
        let reject = state("reject", || false);
        on_command("reject", move || reject.set(true));
        on_command("accept", move || reject.set(false));
        text_input()
            .id("field")
            .value(v)
            .on_input(move |s| {
                if !reject.get() {
                    v.set(s);
                }
            })
            .into()
    });
    app.tap("field").unwrap();
    app.set_selection(4, 4);
    app.type_text("!");
    app.press_key("mod+z").unwrap();
    app.set_selection(3, 1);
    app.invoke("reject").unwrap();
    app.type_text("X");
    assert_eq!(value(&app), "abcd");
    let field = app.text_field().unwrap();
    assert_eq!(field.selection, 1..3);
    assert!(field.reversed);
    app.invoke("accept").unwrap();
    app.press_key("mod+shift+z").unwrap();
    assert_eq!(value(&app), "abcd!");
}

#[test]
fn normalized_proposal_maps_unicode_selection_and_retires_history() {
    let mut app = Window::headless(|| {
        let v = state("value", || "ab".to_owned());
        text_input().id("field").value(v).on_input(move |s| v.set(s.replace('!', ""))).into()
    });
    app.tap("field").unwrap();
    app.set_selection(1, 1);
    app.type_text("!😀");
    assert_eq!(value(&app), "a😀b");
    assert_eq!(selection(&app), 5..5);
    app.press_key("mod+z").unwrap();
    assert_eq!(value(&app), "a😀b");
}

#[test]
fn external_value_after_caret_move_retires_history() {
    let mut app = Window::headless(|| {
        let v = state("value", || "a".to_owned());
        on_command("external", move || v.set("😀".into()));
        text_input().id("field").value(v).on_input(move |s| v.set(s)).into()
    });
    app.tap("field").unwrap();
    app.press_key("end").unwrap();
    app.type_text("b");
    app.press_key("left").unwrap();
    app.invoke("external").unwrap();
    app.press_key("mod+z").unwrap();
    assert_eq!(value(&app), "😀");
    assert!(value(&app).is_char_boundary(selection(&app).start));
}

#[test]
fn rejected_edits_share_selection_and_history_rules_across_input_routes() {
    for route in ["typing", "paste", "native", "accessibility", "semantic", "replace", "no-handler"] {
        let mut app = Window::headless(move || {
            let input = text_input().id("field").value("abcd");
            (if route == "no-handler" { input } else { input.on_input(|_| {}) }).into()
        });
        app.tap("field").unwrap();
        app.set_selection(3, 1);
        match route {
            "typing" | "no-handler" => app.type_text("X"),
            "paste" => {
                app.app().run(|| write_to_clipboard("X"));
                app.press_key("mod+v").unwrap();
            }
            "replace" => app.replace_text(1..3, "X"),
            "semantic" => app.dispatch(Event::Input("field".into(), "X".into())).unwrap(),
            "native" => {
                let input = native_input(&app);
                let mark = TextEdit::Mark { text: "你好".into(), selection: Some(6..6), replacement: None };
                assert!(app.apply_text_edit(input.id, &input.document.unwrap(), &mark));
                let composing = native_input(&app).document.unwrap();
                assert!(app.apply_text_edit(input.id, &composing, &TextEdit::Unmark));
            }
            "accessibility" => {
                let n = app.kernel().find("field").unwrap();
                assert!(app.event(InputEvent::Accessibility {
                    target: octara::kernel::a11y::a11y_id(n),
                    action: accesskit::Action::SetValue,
                    data: Some(accesskit::ActionData::Value("X".into())),
                }));
            }
            _ => unreachable!(),
        }
        assert_eq!(value(&app), "abcd", "{route}");
        let field = app.text_field().unwrap();
        assert_eq!(field.selection, 1..3, "{route}");
        assert!(field.reversed, "{route}");
        assert!(field.composition.is_empty(), "{route}");
        app.press_key("mod+z").unwrap();
        assert_eq!(value(&app), "abcd");
        assert!(app.app().diagnostics().is_empty());
    }
}

#[test]
fn rejected_undo_and_redo_leave_their_stacks_intact() {
    let mut app = Window::headless(|| {
        let v = state("value", || "abc".to_owned());
        let reject = state("reject", || false);
        on_command("reject", move || reject.set(true));
        on_command("accept", move || reject.set(false));
        text_input()
            .id("field")
            .value(v)
            .on_input(move |s| {
                if !reject.get() {
                    v.set(s);
                }
            })
            .into()
    });
    app.tap("field").unwrap();
    app.set_selection(3, 3);
    app.type_text("!");
    app.invoke("reject").unwrap();
    app.press_key("mod+z").unwrap();
    assert_eq!(value(&app), "abc!");
    assert_eq!(selection(&app), 4..4);
    app.invoke("accept").unwrap();
    app.press_key("mod+z").unwrap();
    assert_eq!(value(&app), "abc");
    app.invoke("reject").unwrap();
    app.press_key("mod+shift+z").unwrap();
    assert_eq!(value(&app), "abc");
    assert_eq!(selection(&app), 3..3);
    app.invoke("accept").unwrap();
    app.press_key("mod+shift+z").unwrap();
    assert_eq!(value(&app), "abc!");
}

#[test]
fn edit_callback_may_close_or_replace_its_editor_without_finishing_on_a_new_owner() {
    let mut closed = Window::headless(|| text_input().id("field").on_input(|_| App::current().close_window(current_window())).into());
    closed.tap("field").unwrap();
    closed.type_text("X");
    assert!(closed.is_closed());

    let mut app = Window::headless(|| {
        let replaced = state("replaced", || false);
        show_else(
            move || replaced.get(),
            || text_input().id("field").value("replacement").into(),
            move || text_input().id("field").value("original").on_input(move |_| replaced.set(true)).into(),
        )
    });
    app.tap("field").unwrap();
    let old = native_input(&app);
    app.type_text("X");
    let replacement = app.kernel().find("field").unwrap();
    assert_ne!(replacement, old.id);
    app.dispatch(Event::Focus("field".into())).unwrap();
    assert!(!app.apply_text_edit(old.id, &old.document.unwrap(), &TextEdit::Insert { text: "late".into(), replacement: None }));
    app.press_key("mod+z").unwrap();
    assert_eq!(value(&app), "replacement");
    assert!(app.app().diagnostics().is_empty());
}

#[test]
fn reentered_input_is_refused_without_overwriting_the_outer_proposal() {
    use std::{cell::RefCell, rc::Rc};
    let holder: Rc<RefCell<Option<Window>>> = Rc::default();
    let captured = holder.clone();
    let mut app = Window::headless(move || {
        let v = state("value", String::new);
        let captured = captured.clone();
        text_input()
            .id("field")
            .value(v)
            .on_input(move |s| {
                let mut nested = captured.borrow().as_ref().unwrap().clone();
                let frames = nested.frames_presented();
                nested.type_text("nested");
                assert_eq!(nested.frames_presented(), frames, "an unresolved edit cannot publish");
                v.set(s);
            })
            .into()
    });
    *holder.borrow_mut() = Some(app.clone());
    app.tap("field").unwrap();
    app.type_text("outer");
    assert_eq!(value(&app), "outer");
    assert_eq!(selection(&app), 5..5);
    let diagnostics = app.app().diagnostics();
    assert_eq!(diagnostics.iter().filter(|d| d.code == "EDIT_PENDING").count(), 1);
    holder.borrow_mut().take();
    app.app().shutdown();
}

#[test]
fn native_batch_continuations_require_the_accepted_document() {
    use std::{cell::Cell, rc::Rc};
    for (reject, composing) in [(false, false), (true, false), (false, true), (true, true)] {
        let calls = Rc::new(Cell::new(0));
        let counted = calls.clone();
        let mut app = Window::headless(move || {
            let value = state("value", || "ab".to_owned());
            let counted = counted.clone();
            text_input()
                .id("field")
                .value(value)
                .on_input(move |s| {
                    counted.set(counted.get() + 1);
                    if !reject {
                        value.set(s.replace('!', ""));
                    }
                })
                .into()
        });
        app.tap("field").unwrap();
        app.set_selection(1, 1);
        if composing {
            let before = native_input(&app);
            let mark = TextEdit::Mark { text: "!😀".into(), selection: Some(5..5), replacement: None };
            assert!(app.apply_text_edit(before.id, &before.document.unwrap(), &mark));
        }
        let before = native_input(&app);
        let expected = before.document.unwrap();
        let first = if composing { TextEdit::Unmark } else { TextEdit::Insert { text: "!😀".into(), replacement: None } };
        let mut native_result = expected.clone();
        native_result.apply(&first);
        assert!(app.apply_text_edit(before.id, &expected, &first), "delivery is not business acceptance");
        let accepted = native_input(&app).document.unwrap();
        assert_ne!(accepted, native_result);
        let tail = TextEdit::Insert { text: "late".into(), replacement: None };
        assert!(!app.apply_text_edit(before.id, &native_result, &tail));
        assert_eq!(calls.get(), 1, "the stale tail never reaches application code");
        assert_eq!(value(&app), if reject { "ab" } else { "a😀b" });
        assert_eq!(selection(&app), if reject { 1..1 } else { 5..5 });
        assert!(app.apply_text_edit(before.id, &accepted, &TextEdit::Insert { text: "新".into(), replacement: None }));
        assert_eq!(calls.get(), 2, "a fresh native document can start a new operation");
        assert!(app.app().diagnostics().is_empty());
    }
}

#[test]
fn same_value_binding_refresh_preserves_composition_and_cancel_preserves_redo() {
    let mut app = Window::headless(|| {
        let value = state("value", || "A😀Z".to_owned());
        let refresh = state("refresh", || 0);
        on_command("refresh", move || refresh.update(|v| *v += 1));
        text_input()
            .id("field")
            .value(move || {
                let _ = refresh.get();
                value.get()
            })
            .on_input(move |s| value.set(s))
            .into()
    });
    app.tap("field").unwrap();
    app.set_selection(6, 6);
    app.type_text("!");
    app.press_key("mod+z").unwrap();
    app.set_selection(5, 1);
    let input = native_input(&app);
    let original = input.document.unwrap();
    let mark = TextEdit::Mark { text: "你好".into(), selection: Some(0..6), replacement: None };
    assert!(app.apply_text_edit(input.id, &original, &mark));
    let composing = native_input(&app).document.unwrap();
    app.invoke("refresh").unwrap();
    assert_eq!(native_input(&app).document, Some(composing.clone()));
    assert!(app.apply_text_edit(input.id, &composing, &TextEdit::Cancel));
    assert_eq!(native_input(&app).document, Some(original));
    app.press_key("mod+shift+z").unwrap();
    assert_eq!(value(&app), "A😀Z!");
    assert!(app.app().diagnostics().is_empty());
}

#[test]
fn reload_and_close_retire_native_drafts_and_editor_history() {
    let mut app = app("before");
    app.set_selection(6, 6);
    app.type_text("!");
    let old = native_input(&app);
    let mark = TextEdit::Mark { text: "nihao".into(), selection: Some(5..5), replacement: None };
    assert!(app.apply_text_edit(old.id, &old.document.unwrap(), &mark));
    let draft = native_input(&app).document.unwrap();
    app.app().hot_reload();
    app.dispatch(Event::Focus("field".into())).unwrap();
    let fresh = native_input(&app);
    assert_ne!(fresh.id, old.id);
    assert!(!app.apply_text_edit(old.id, &draft, &TextEdit::Unmark));
    assert_eq!(value(&app), "before!");
    assert!(app.text_field().unwrap().composition.is_empty());
    app.press_key("mod+z").unwrap();
    assert_eq!(value(&app), "before!", "hot reload retains model state, not the old editor's history");
    app.app().close_window(app.id());
    assert!(app.is_closed());
    assert!(!app.apply_text_edit(fresh.id, &fresh.document.unwrap(), &mark));
    app.app().shutdown();
}

#[test]
fn explicit_selection_in_an_input_callback_takes_precedence_over_its_proposal() {
    use std::{cell::RefCell, rc::Rc};
    let holder: Rc<RefCell<Option<Window>>> = Rc::default();
    let captured = holder.clone();
    let mut app = Window::headless(move || {
        let value = state("value", || "abc".to_owned());
        let captured = captured.clone();
        text_input()
            .id("field")
            .value(value)
            .on_input(move |text| {
                captured.borrow().as_ref().unwrap().clone().set_selection(0, 0);
                value.set(text);
            })
            .into()
    });
    *holder.borrow_mut() = Some(app.clone());
    app.tap("field").unwrap();
    app.set_selection(3, 3);
    app.type_text("d");
    assert_eq!(value(&app), "abcd");
    assert_eq!(selection(&app), 0..0, "the later explicit selection wins over the proposed caret");
    app.press_key("mod+z").unwrap();
    assert_eq!(value(&app), "abcd", "the cancelled proposal creates no undo record");
    assert!(app.app().diagnostics().is_empty());
    holder.borrow_mut().take();
    app.app().shutdown();
}

#[test]
fn composition_terminal_order_and_blur_preserve_only_accepted_history() {
    for terminal in [None, Some(TextEdit::Cancel), Some(TextEdit::Unmark)] {
        let mut app = app("A😀Z");
        app.set_selection(6, 6);
        app.type_text("!");
        app.set_selection(5, 1);
        let input = native_input(&app);
        let mark = TextEdit::Mark { text: "你好".into(), selection: Some(6..6), replacement: None };
        assert!(app.apply_text_edit(input.id, &input.document.unwrap(), &mark));
        let draft = native_input(&app).document.unwrap();
        if let Some(edit) = &terminal {
            assert!(app.apply_text_edit(input.id, &draft, edit));
        }
        app.dispatch(Event::Focus("narrow".into())).unwrap();
        assert!(!app.apply_text_edit(input.id, &draft, &TextEdit::Unmark));
        app.dispatch(Event::Focus("field".into())).unwrap();
        let committed = terminal == Some(TextEdit::Unmark);
        assert_eq!(value(&app), if committed { "A你好Z!" } else { "A😀Z!" });
        assert!(app.text_field().unwrap().composition.is_empty());
        app.press_key("mod+z").unwrap();
        assert_eq!(value(&app), if committed { "A😀Z!" } else { "A😀Z" }, "only accepted commits add history");
        assert!(app.app().diagnostics().is_empty());
    }
}
