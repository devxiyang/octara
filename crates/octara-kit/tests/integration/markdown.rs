//! Markdown ([text core](../../../../docs/design/text.md) step 4): blocks as elements with stable
//! keys; a stream re-parses its end and keeps every earlier block's node.

use octara::kernel::{Kernel, NodeId};
use octara::platform::{HeadlessPlatform, WindowOptions};
use octara::prelude::*;
use octara_kit::markdown::markdown;

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
    assert_eq!(w.app().platform().read_text().as_deref(), Some("let a = 1;\nlet b = 2;"));
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
    let live = app.run(|| LiveText::new(""));
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
    let live = app.run(|| LiveText::new(""));
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
    w.app().platform().read_text().unwrap_or_default()
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
    let live = app.run(|| LiveText::new(""));
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
    let live = app.run(|| LiveText::new("live one"));
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

/// A selectable transcript of selectable messages: one document.
fn transcript() -> View {
    let messages = list("messages", || (0..200u32).collect(), |i: &u32| *i);
    col()
        .child(virtual_each(move || messages.all(), |_| 40.0, |i, _| markdown(format!("**m{i}** says *hi*")).id(format!("m{i}")).into()))
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
