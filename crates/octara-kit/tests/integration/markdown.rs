//! Markdown ([text core](../../../../docs/design/text.md) step 4): blocks as elements with stable
//! keys; a stream re-parses its end and keeps every earlier block's node.

use octara::kernel::Kernel;
use octara::platform::{HeadlessPlatform, WindowOptions};
use octara::prelude::*;
use octara_kit::markdown::markdown;
use octara_primitives::identity::NodeId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

const DOC: &str = "# Title\n\nHello **world** and [link](https://x.dev/a).\n\n- alpha\n- [x] beta\n\n```rust\nfn main() {}\n\nlet x = 1;\n```\n\n> quoted\n\n| a | b |\n|---|--:|\n| 1 | `2` |\n\n---\n\n3. three\n4. four\n";

/// The top-level blocks: their keys and nodes.
fn blocks(k: &Kernel) -> Vec<(String, NodeId)> {
    let md = k.find("md").unwrap();
    k.children(md).iter().filter_map(|c| k.row_key(*c).map(|key| (key.to_string(), *c))).collect()
}

/// The texts under `n`, in tree order.
fn texts(k: &Kernel, n: NodeId) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![n];
    while let Some(n) = stack.pop() {
        if !k.text(n).is_empty() {
            out.push(k.text(n).to_owned());
        }
        stack.extend(k.children(n).iter().rev());
    }
    out
}

#[test]
fn blocks_are_elements_with_roles_and_keys() {
    let w = Window::headless(|| markdown(DOC).id("md").into());
    let k = &*w.kernel();
    let bs = blocks(k);
    let keys: Vec<&str> = bs.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(keys, ["b0", "b1", "b2", "b3", "b4", "b5", "b6", "b7"]);
    let role = |i: usize| k.role(bs[i].1);
    assert_eq!(
        [role(0), role(1), role(2), role(3), role(4), role(5), role(6), role(7)],
        [Role::Heading, Role::Paragraph, Role::List, Role::Code, Role::Blockquote, Role::Table, Role::GenericContainer, Role::List]
    );
    assert_eq!(k.role(k.find("md").unwrap()), Role::Document);
    assert_eq!(k.text(bs[1].1), "Hello world and link.");
    assert_eq!(texts(k, bs[2].1), ["•", "alpha", "☑", "beta"], "a marker; a task's box instead");
    // A code block: its language, the copy button, a text per line (an empty one kept).
    let code = texts(k, bs[3].1);
    assert_eq!(code, ["rust", "Copy", "fn main() {}", " ", "let x = 1;"]);
    assert_eq!(texts(k, bs[5].1), ["a", "b", "1", "2"]);
    assert_eq!(texts(k, bs[7].1), ["3.", "three", "4.", "four"], "an ordered list counts from its start");
}

#[test]
fn copy_buttons_write_the_code() {
    let mut w = Window::headless(|| markdown("```\nlet a = 1;\nlet b = 2;\n```\n").id("md").into());
    w.tap("Copy").unwrap();
    assert_eq!(w.clipboard_text().as_deref(), Some("let a = 1;\nlet b = 2;"));
}

#[test]
fn editing_an_earlier_code_line_updates_it_and_keeps_the_block() {
    let app = App::new(HeadlessPlatform::default());
    let live = app.run(|| TextBuffer::new("```rust\nlet first = 1;\nlet second = 2;\n```"));
    let stream = live.clone();
    let window = app.open_window(WindowOptions::new("Code"), move || markdown(&stream).id("md").into()).unwrap();
    let code = blocks(&window.kernel())[0].1;

    app.run(|| live.set("```rust\nlet first = 9;\nlet second = 2;\n```"));

    assert_eq!(blocks(&window.kernel())[0].1, code, "editing the code does not replace its block");
    assert_eq!(texts(&window.kernel(), code), ["rust", "Copy", "let first = 9;", "let second = 2;"]);
    app.run(|| live.set("```rust\nshorter\n```"));
    assert_eq!(texts(&window.kernel(), code), ["rust", "Copy", "shorter"], "a rewrite also removes old trailing lines");
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn a_stream_keeps_every_earlier_blocks_node() {
    // A 2 000-token answer: paragraphs, a list, a code block, a table.
    let mut answer = String::new();
    for i in 0..48 {
        answer
            .push_str(&format!("Paragraph {i} says **something** with `code` and [a link](https://x.dev/{i}) over a few words here.\n\n"));
        if i % 4 == 1 {
            answer.push_str("- first point\n- second point with *emphasis*\n\n");
        }
        if i % 4 == 2 {
            answer.push_str("```rust\nfn f() {\n    1\n}\n```\n\n");
        }
        if i % 4 == 3 {
            answer.push_str("| k | v |\n|---|---|\n| a | 1 |\n| b | 2 |\n\n");
        }
    }
    // Tokens as a model streams them: pieces of up to three characters.
    let chars: Vec<char> = answer.chars().collect();
    let tokens: Vec<String> = chars.chunks(3).map(|c| c.iter().collect()).collect();
    assert!(tokens.len() >= 2000, "{} tokens", tokens.len());
    let app = App::new(HeadlessPlatform::default());
    let live = app.run(|| TextBuffer::new(""));
    let stream = live.clone();
    let w = app.open_window(WindowOptions::new("Chat"), move || markdown(&stream).id("md").into()).unwrap();
    // Each block's node when it finished (another block started after it).
    let mut finished: Vec<(String, NodeId)> = Vec::new();
    let mut steps = 0;
    for t in &tokens {
        app.run(|| live.append(t));
        app.settle();
        steps += 1;
        let bs = blocks(&w.kernel());
        for (i, (key, node)) in bs.iter().enumerate().take(bs.len().saturating_sub(1)) {
            match finished.get(i) {
                Some((k, n)) => assert_eq!((k, n), (key, node), "block {key} kept its node (step {steps})"),
                None => finished.push((key.clone(), *node)),
            }
        }
    }
    let bs = blocks(&w.kernel());
    assert!(bs.len() > 20, "{} blocks", bs.len());
    // The whole answer drawn as a fresh render draws it.
    let fresh = Window::headless({
        let a = answer.clone();
        move || markdown(a.as_str()).id("md").into()
    });
    let all = |k: &Kernel| texts(k, k.find("md").unwrap());
    assert_eq!(all(&w.kernel()), all(&fresh.kernel()));
}

#[test]
fn a_fence_streams_as_code_and_prose_follows_its_close() {
    let app = App::new(HeadlessPlatform::default());
    let live = app.run(|| TextBuffer::new(""));
    let stream = live.clone();
    let w = app.open_window(WindowOptions::new("Chat"), move || markdown(&stream).id("md").into()).unwrap();
    let src = "Intro\n\n```py\nprint(1)\nx = **2**\n```\nAfter the *fence*.\n";
    let mut saw_open_code = false;
    for (i, _) in src.char_indices().skip(1) {
        app.run(|| live.set(&src[..i]));
        app.settle();
        let bs = blocks(&w.kernel());
        if src[..i].contains("x = **") && !src[..i].contains("\n```\n") {
            let code = bs.iter().find(|(_, n)| w.kernel().role(*n) == Role::Code).expect("an open fence is code");
            assert!(texts(&w.kernel(), code.1).iter().any(|t| t.starts_with("x = **")), "markers in code show as written");
            saw_open_code = true;
        }
    }
    assert!(saw_open_code);
    let bs = blocks(&w.kernel());
    assert_eq!(w.kernel().role(bs.last().unwrap().1), Role::Paragraph);
    assert_eq!(w.kernel().text(bs.last().unwrap().1), "After the fence.");
}

fn copy(w: &mut Window, chord: &str) -> String {
    w.press_key(chord).unwrap();
    w.clipboard_text().unwrap_or_default()
}

/// Presses at the start of `a`'s text and releases `to` px into `b`'s.
fn drag(w: &mut Window, a: NodeId, b: NodeId, to: f32) {
    let (ax, ay, _, ah) = w.kernel().shown_rect(a);
    let (bx, by, _, bh) = w.kernel().shown_rect(b);
    w.pointer_down(ax + 1.0, ay + ah / 2.0);
    w.pointer_move(bx + to, by + bh / 2.0);
    w.pointer_up(bx + to, by + bh / 2.0);
}

#[test]
fn the_pointer_selects_code_lines_as_it_selects_paragraphs() {
    let mut w = Window::headless(|| markdown("alpha one\n\n```text\nalpha one\n```\n").id("md").w(600.0).into());
    let nodes = {
        let k = w.kernel();
        let md = k.find("md").unwrap();
        let nodes = k.selectable_texts(md);
        assert_eq!(nodes.iter().map(|n| k.text(*n)).collect::<Vec<_>>(), ["alpha one", "alpha one"]);
        nodes
    };
    for (label, node) in ["paragraph", "code line"].into_iter().zip(nodes) {
        let (x, y, width, height) = w.kernel().shown_rect(node);
        assert!(width > 4.0 && height > 0.0, "{label} is laid out");
        let (x, y) = (x + 2.0, y + height / 2.0);
        assert_eq!(w.kernel().hit_test(x, y), Some(node), "the coordinates hit the {label} text itself");
        w.app().advance(std::time::Duration::from_secs(1));
        w.click_at(x, y);
        w.click_at(x, y);
        assert_eq!(w.selected_text().as_deref(), Some("alpha"), "double clicking the {label} selects its word");
        assert_eq!(copy(&mut w, "mod+c"), "alpha");

        w.app().advance(std::time::Duration::from_secs(1));
        drag(&mut w, node, node, width - 1.0);
        assert_eq!(w.selected_text().as_deref(), Some("alpha one"), "dragging the {label} selects its text");
        assert_eq!(copy(&mut w, "mod+c"), "alpha one");
    }
    assert!(w.layout_issues().is_empty(), "{:?}", w.layout_issues());
    assert!(w.app().diagnostics().is_empty(), "{:?}", w.app().diagnostics());
}

#[test]
fn copies_text_joined_by_roles_and_markdown_by_blocks() {
    let mut w = Window::headless(|| markdown(DOC).id("md").w(600.0).into());
    w.select_all("md").unwrap();
    assert_eq!(
        copy(&mut w, "mod+c"),
        "Title\n\nHello world and link.\n\nalpha\nbeta\n\nfn main() {}\n \nlet x = 1;\n\nquoted\n\na\tb\n1\t2\n\nthree\nfour",
        "cells by a tab, rows, items and lines by a newline, blocks by a blank line"
    );
    assert_eq!(copy(&mut w, "mod+alt+c"), DOC, "everything: the source as written");

    let bs = blocks(&w.kernel());
    // A whole paragraph (a triple press): its source.
    let (x, y, _, h) = w.kernel().shown_rect(bs[1].1);
    for _ in 0..3 {
        w.click_at(x + 2.0, y + h / 2.0);
    }
    assert_eq!(w.selected_text().as_deref(), Some("Hello world and link."));
    assert_eq!(copy(&mut w, "mod+alt+c"), "Hello **world** and [link](https://x.dev/a).");
    // Part of one block: the drawn text.
    w.app().advance(std::time::Duration::from_secs(1));
    w.click_at(x + 2.0, y + h / 2.0);
    w.click_at(x + 2.0, y + h / 2.0);
    assert_eq!(copy(&mut w, "mod+alt+c"), "Hello");
    // From the heading into the list: each touched block whole.
    w.app().advance(std::time::Duration::from_secs(1));
    let alpha = {
        let k = w.kernel();
        let list = bs[2].1;
        let mut stack = vec![list];
        let mut found = None;
        while let Some(n) = stack.pop() {
            if k.text(n) == "alpha" {
                found = Some(n);
            }
            stack.extend(k.children(n).iter().copied());
        }
        found.unwrap()
    };
    drag(&mut w, bs[0].1, alpha, 10.0);
    assert_eq!(copy(&mut w, "mod+alt+c"), "# Title\n\nHello **world** and [link](https://x.dev/a).\n\n- alpha\n- [x] beta");
}

#[test]
fn assistive_technology_hears_each_block_as_it_finishes() {
    let app = App::new(HeadlessPlatform::default());
    let live = app.run(|| TextBuffer::new(""));
    let stream = live.clone();
    let w = app.open_window(WindowOptions::new("Chat"), move || markdown(&stream).id("md").into()).unwrap();
    let heard = |w: &Window| {
        let k = w.kernel();
        let md = k.find("md").unwrap();
        let announcer = *k.children(md).last().unwrap();
        k.label(announcer).map(str::to_owned)
    };
    for t in ["The first ", "paragraph is ", "here.\n\n"] {
        app.run(|| live.append(t));
        app.settle();
        assert!(heard(&w).is_none_or(|h| h.is_empty()), "nothing while the first block streams: {:?}", heard(&w));
    }
    app.run(|| live.append("The sec"));
    app.settle();
    assert_eq!(heard(&w).as_deref(), Some("The first paragraph is here."), "the block that finished, once the next starts");
    app.run(|| live.append("ond one"));
    app.settle();
    assert_eq!(heard(&w).as_deref(), Some("The first paragraph is here."), "not each token");
}

#[test]
fn instances_keep_their_own_blocks() {
    let app = App::new(HeadlessPlatform::default());
    let live = app.run(|| TextBuffer::new("live one"));
    let stream = live.clone();
    let w = app
        .open_window(WindowOptions::new("Chat"), move || {
            col()
                .children((0..2).map(|i| markdown(format!("static {i}").as_str()).id(format!("s{i}"))))
                .child(markdown(&stream).id("t"))
                .into()
        })
        .unwrap();
    app.run(|| live.append(" two"));
    app.settle();
    let k = w.kernel();
    let first = |id: &str| texts(&k, k.find(id).unwrap())[0].clone();
    assert_eq!([first("s0"), first("s1"), first("t")], ["static 0", "static 1", "live one two"]);
}

#[test]
fn reload_releases_derived_markdown_state() {
    let window = Window::headless(|| markdown(DOC).id("md").into());
    let before = window.app().work().reactive_nodes;
    let mut counts = vec![before];
    for _ in 0..8 {
        window.app().hot_reload();
        counts.push(window.app().work().reactive_nodes);
    }
    assert!(counts.iter().all(|count| *count == before), "reload retained derived Markdown state: {counts:?}");
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

#[test]
fn reload_rebuilds_all_source_kinds_from_their_current_source() {
    let app = App::new(HeadlessPlatform::default());
    let dynamic = app.run(|| state("dynamic", String::new));
    let buffer = app.run(|| TextBuffer::new(""));
    let host = Rc::new(RefCell::new(String::new()));
    let (static_source, stream) = (host.clone(), buffer.clone());
    let mut window = app
        .open_window(WindowOptions::new("Sources"), move || {
            col()
                .child(markdown(static_source.borrow().as_str()).id("static"))
                .child(markdown(move || dynamic.get()).id("dynamic"))
                .child(markdown(&stream).id("buffer"))
                .into()
        })
        .unwrap();
    let sources = [DOC, "## Rebuilt\n\nA [new link](https://x.dev/new).\n", "> quoted\n>\n> ```\n> nested code\n> ```\n"];
    let mut counts = HashMap::new();
    for source in sources.into_iter().cycle().take(18) {
        // A static source receives no reactive notification: reload must rebuild its cache.
        *host.borrow_mut() = source.to_owned();
        app.run(|| {
            dynamic.set(source.to_owned());
            buffer.set(source);
        });
        app.hot_reload();
        let count = app.work().reactive_nodes;
        assert_eq!(*counts.entry(source).or_insert(count), count, "unclaimed cache paths accumulate across shape changes");
        let expected = {
            let fresh = Window::headless(move || markdown(source).id("fresh").into());
            let k = fresh.kernel();
            texts(&k, k.find("fresh").unwrap())
        };
        for id in ["static", "dynamic", "buffer"] {
            let k = window.kernel();
            assert_eq!(texts(&k, k.find(id).unwrap()), expected, "{id} rebuilt from its current source");
            drop(k);
            window.select_all(id).unwrap();
            assert_eq!(window.selected_markdown().as_deref(), Some(source), "{id} copies the same source it draws");
        }
    }
    app.run(|| {
        dynamic.set("dynamic keeps following".to_owned());
        buffer.append("\n\na buffer keeps following");
    });
    let k = window.kernel();
    assert_eq!(texts(&k, k.find("dynamic").unwrap()), ["dynamic keeps following"]);
    assert_eq!(texts(&k, k.find("buffer").unwrap()).last().map(String::as_str), Some("a buffer keeps following"));
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn unmount_releases_every_markdown_cache() {
    let app = App::new(HeadlessPlatform::default());
    let visible = app.run(|| state("visible", || false));
    let window = app
        .open_window(WindowOptions::new("Mount"), move || col().child(show(move || visible.get(), || markdown(DOC).id("md").into())).into())
        .unwrap();
    let empty = app.work().reactive_nodes;
    let mut mounted = None;
    for _ in 0..16 {
        app.run(|| visible.set(true));
        let count = app.work().reactive_nodes;
        assert_eq!(*mounted.get_or_insert(count), count);
        assert!(window.kernel().exists("md"));
        app.run(|| visible.set(false));
        assert!(!window.kernel().exists("md"));
        assert_eq!(app.work().reactive_nodes, empty, "derived work belongs to the mounted branch");
    }
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn retained_nested_rows_follow_task_number_and_alignment_changes() {
    let app = App::new(HeadlessPlatform::default());
    let source = app.run(|| TextBuffer::new("1. first\n2. second\n\n- task\n\n| a | b |\n|:---|---|\n| c | d |\n"));
    let stream = source.clone();
    let window = app.open_window(WindowOptions::new("Structure"), move || markdown(&stream).id("md").w(600.0).into()).unwrap();
    let before = blocks(&window.kernel());
    let (cell, text) = {
        let k = window.kernel();
        let row = k.children(before[2].1)[0];
        let cell = k.children(row)[0];
        (cell, k.children(cell)[0])
    };
    let left = window.kernel().shown_rect(text).0;
    app.run(|| source.set("4. first\n5. second\n\n- [x] task\n\n| a | b |\n|---:|---|\n| c | d |\n"));
    assert_eq!(blocks(&window.kernel()), before, "content and alignment retain the outer block nodes");
    assert_eq!(texts(&window.kernel(), before[0].1), ["4.", "first", "5.", "second"]);
    assert_eq!(texts(&window.kernel(), before[1].1), ["☑", "task"]);
    let k = window.kernel();
    assert_eq!(k.children(cell)[0], text, "alignment retains the cell's text");
    assert!(k.shown_rect(text).0 > left + 100.0, "right alignment follows a changed delimiter row");
    drop(k);
    app.run(|| source.set("4. first\n5. second\n\n- task\n\n| a | b |\n|:---:|---|\n| c | d |\n"));
    assert_eq!(texts(&window.kernel(), before[1].1), ["•", "task"]);
    let center = window.kernel().shown_rect(text).0;
    assert!(center > left + 20.0 && center < left + 200.0);
    app.run(|| source.set("> # Heading\n>\n> paragraph\n\n```\ncode\n```\n"));
    let changed = blocks(&window.kernel());
    assert_ne!(changed[0].1, before[0].1, "a new block kind receives a new element");
    assert_eq!(window.kernel().role(changed[0].1), Role::Blockquote);
    assert_eq!(texts(&window.kernel(), changed[0].1), ["Heading", "paragraph"]);
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn long_code_snapshots_only_write_the_changed_line() {
    for count in [8, 512] {
        let app = App::new(HeadlessPlatform::default());
        let mut source = "```text\n".to_owned();
        for line in 0..count {
            source.push_str(&format!("line {line}\n"));
        }
        source.push_str("tail");
        let source = app.run(|| TextBuffer::new(source));
        let stream = source.clone();
        let window = app.open_window(WindowOptions::new("Code work"), move || markdown(&stream).id("md").into()).unwrap();
        let nodes = {
            let k = window.kernel();
            k.selectable_texts(k.find("md").unwrap())
        };
        window.reset_counters();
        app.reset_work();
        app.run(|| source.append("x"));
        let counters = window.kernel().counters();
        assert_eq!(counters.nodes_created, 0);
        assert_eq!(counters.nodes_destroyed, 0);
        assert_eq!(counters.dirty_text, 1, "only the tail line needs text layout");
        assert_eq!(counters.text_bytes, 5, "only the changed tail is written to the kernel");
        assert_eq!(window.kernel().selectable_texts(window.kernel().find("md").unwrap()), nodes);
        eprintln!(
            "code lines={}: closures={}, text bytes={}, dirty texts={}",
            count + 1,
            app.work().closures_run,
            counters.text_bytes,
            counters.dirty_text
        );
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    }
}

/// A selectable transcript of selectable messages: one document.
fn transcript() -> View {
    let messages = state_list("messages", || (0..200u32).collect(), |i: &u32| *i);
    col()
        .child(virtual_each(
            move || messages.read_only(),
            |_| 40.0,
            |i, _| markdown(format!("**m{i}** says *hi*")).id(format!("m{i}")).into(),
        ))
        .id("t")
        .selectable()
        .h(300.0)
        .w(400.0)
        .overflow_y_scroll()
        .into()
}

#[test]
fn a_selection_crosses_the_regions_inside_a_region() {
    let mut w = Window::headless(transcript);
    {
        let k = w.kernel();
        assert_eq!(k.role(k.find("t").unwrap()), Role::Document);
        assert_ne!(k.role(k.find("m0").unwrap()), Role::Document, "a message is part of the transcript's document");
    }
    let first = |w: &Window, id: &str| {
        let k = w.kernel();
        let m = k.find(id).unwrap();
        let mut stack = vec![m];
        while let Some(n) = stack.pop() {
            if !k.text(n).is_empty() {
                return n;
            }
            stack.extend(k.children(n).iter().rev().copied());
        }
        panic!("no text in {id}")
    };
    let (a, b) = (first(&w, "m0"), first(&w, "m2"));
    drag(&mut w, a, b, 1000.0);
    assert_eq!(w.selected_text().as_deref(), Some("m0 says hi\n\nm1 says hi\n\nm2 says hi"));
    assert_eq!(copy(&mut w, "mod+alt+c"), "**m0** says *hi*\n\n**m1** says *hi*\n\n**m2** says *hi*", "each message copies its source");

    // Everything, rows not mounted included.
    w.select_all("t").unwrap();
    let all: Vec<String> = (0..200).map(|i| format!("**m{i}** says *hi*")).collect();
    assert_eq!(copy(&mut w, "mod+alt+c"), all.join("\n\n"));
}
