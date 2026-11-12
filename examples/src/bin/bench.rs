//! Benchmarks for the example apps. Run with `cargo run --release -p octara-examples --bin bench`
//! (`-- --frames`, `-- --stream` or `-- --joins` for one section).
//! Counts (closures, property changes, allocations) are exact; times are the minimum of 3 runs.

use octara::app::Window;
use octara::prelude::*;
use octara_examples::apps::{self, Apps};
use octara_examples::scenarios::{Factory, bench_steps, press, toggle};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Counting = Counting;

fn steps<F: Factory>() {
    println!("## {} per-op cost (mean, layout off)\n", F::NAME);
    println!("| step | closures | prop changes | struct ops | allocs | µs (min of 3) |");
    println!("| --- | ---: | ---: | ---: | ---: | ---: |");
    for step in bench_steps() {
        let mut best = f64::MAX;
        let mut row = (0.0, 0.0, 0.0, 0.0);
        for _ in 0..3 {
            let mut h = step.app.build::<F>();
            h.kernel_mut().set_layout_enabled(false);
            (step.prepare)(&mut h).unwrap();
            h.app().reset_work();
            h.kernel_mut().reset_counters();
            let a0 = ALLOCS.load(Ordering::Relaxed);
            let t0 = Instant::now();
            for i in 0..step.repeat {
                (step.op)(&mut h, i).unwrap();
            }
            let n = step.repeat as f64;
            let us = t0.elapsed().as_secs_f64() * 1e6 / n;
            let c = h.kernel().counters();
            row = (
                h.app().work().closures_run as f64 / n,
                c.prop_changes as f64 / n,
                c.structural_ops as f64 / n,
                (ALLOCS.load(Ordering::Relaxed) - a0) as f64 / n,
            );
            best = best.min(us);
        }
        println!("| {} | {:.1} | {:.1} | {:.1} | {:.0} | {:.1} |", step.name, row.0, row.1, row.2, row.3, best);
    }
    println!();
}

fn scaling() {
    println!("## scaling: µs per op, layout off, min of 3 runs (allocs per op)\n");
    println!("| variant | n | toggle #500 | add one | remove #500 |");
    println!("| --- | ---: | ---: | ---: | ---: |");
    fn one<F: Factory>(n: usize, op: &dyn Fn(&mut Window, usize)) -> (f64, f64) {
        let mut best = (f64::MAX, 0.0);
        for _ in 0..3 {
            let mut h = F::todo_with(n);
            h.kernel_mut().set_layout_enabled(false);
            let a0 = ALLOCS.load(Ordering::Relaxed);
            let t0 = Instant::now();
            for i in 0..20 {
                op(&mut h, i);
            }
            let us = t0.elapsed().as_secs_f64() * 1e6 / 20.0;
            if us < best.0 {
                best = (us, (ALLOCS.load(Ordering::Relaxed) - a0) as f64 / 20.0);
            }
        }
        best
    }
    fn run<F: Factory>(label: &str, n: usize) {
        let t = one::<F>(n, &|h, _| toggle(h, "toggle-500").unwrap());
        let a = one::<F>(n, &|h, _| {
            octara_examples::scenarios::input(h, "new-todo", "new").unwrap();
            press(h, "add").unwrap();
        });
        let r = one::<F>(n, &|h, i| press(h, &format!("remove-{}", 500 + i)).unwrap());
        println!("| {label} | {n} | {:.1} ({:.0}) | {:.1} ({:.0}) | {:.1} ({:.0}) |", t.0, t.1, a.0, a.1, r.0, r.1);
    }
    for n in [1_000usize, 10_000, 100_000] {
        run::<Apps>("octara", n);
    }
    println!();
}

fn virtualization() {
    println!("## virtualization: chat, layout ON, min of 3 (µs; nodes in kernel)\n");
    println!("| variant | n | mount ms | live kernel nodes | token, off-screen µs | token, visible µs | scroll µs |");
    println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: |");
    fn run(label: &str, n: usize, build: fn(usize) -> View, scrolls: bool) {
        let (mut mount, mut token, mut scroll, mut nodes, mut visible) = (f64::MAX, f64::MAX, f64::MAX, 0, f64::MAX);
        for _ in 0..3 {
            let t0 = Instant::now();
            let mut h = Window::headless(move || build(n));
            mount = mount.min(t0.elapsed().as_secs_f64() * 1e3);
            nodes = h.kernel().live_nodes();
            octara_examples::scenarios::external(&mut h, "user", "go").unwrap();
            let t0 = Instant::now();
            for _ in 0..20 {
                octara_examples::scenarios::external(&mut h, "token", "tok ").unwrap();
            }
            token = token.min(t0.elapsed().as_secs_f64() * 1e6 / 20.0);
            if scrolls {
                h.dispatch(Event::ScrollTo("messages".into(), 0.0, 1e9)).unwrap();
                let t0 = Instant::now();
                for _ in 0..20 {
                    octara_examples::scenarios::external(&mut h, "token", "tok ").unwrap();
                }
                visible = visible.min(t0.elapsed().as_secs_f64() * 1e6 / 20.0);
                let t0 = Instant::now();
                for i in 0..20 {
                    h.dispatch(Event::ScrollTo("messages".into(), 0.0, (i * 997 % n) as f32 * apps::MSG_ROW_HEIGHT)).unwrap();
                }
                scroll = scroll.min(t0.elapsed().as_secs_f64() * 1e6 / 20.0);
            }
        }
        let (s, v) = if scrolls { (format!("{scroll:.1}"), format!("{visible:.1}")) } else { ("—".into(), format!("{token:.1}")) };
        println!("| {label} | {n} | {mount:.1} | {nodes} | {token:.1} | {v} | {s} |");
    }
    for n in [1_000usize, 10_000, 100_000] {
        run("each", n, apps::chat, false);
        run("virtual_each", n, apps::chat_virtual, true);
    }
    println!();
}

fn operators() {
    println!("## sort_by + group_by: contacts, layout off, min of 3 (µs per op; allocs)\n");
    println!("| n | rename (move in group) | change team (move across groups) | star (no move) |");
    println!("| ---: | ---: | ---: | ---: |");
    for n in [1_000usize, 10_000, 100_000] {
        let time = |op: &dyn Fn(&mut Window, usize)| {
            let mut best = (f64::MAX, 0.0);
            for _ in 0..3 {
                let mut h = Window::headless(move || apps::contacts(n));
                h.kernel_mut().set_layout_enabled(false);
                let a0 = ALLOCS.load(Ordering::Relaxed);
                let t0 = Instant::now();
                for i in 0..20 {
                    op(&mut h, i);
                }
                let us = t0.elapsed().as_secs_f64() * 1e6 / 20.0;
                if us < best.0 {
                    best = (us, (ALLOCS.load(Ordering::Relaxed) - a0) as f64 / 20.0);
                }
            }
            best
        };
        let r = time(&|h, i| octara_examples::scenarios::external(h, "rename", &format!("{}:a{i:03}", 3 + 3 * i)).unwrap());
        let t = time(&|h, i| octara_examples::scenarios::external(h, "team", &format!("{}:{}", 3 + 3 * i, apps::TEAMS[i % 2])).unwrap());
        let s = time(&|h, i| octara_examples::scenarios::toggle(h, &format!("star-{}", 3 + 3 * i)).unwrap());
        println!("| {n} | {:.1} ({:.0}) | {:.1} ({:.0}) | {:.1} ({:.0}) |", r.0, r.1, t.0, t.1, s.0, s.1);
    }
    println!();
}

fn grouped_virtual() {
    println!("## grouped contacts, layout ON, min of 3: each vs flatten + virtual_each\n");
    println!("| variant | n | mount ms | kernel nodes | rename µs | change team µs |");
    println!("| --- | ---: | ---: | ---: | ---: | ---: |");
    fn run(label: &str, n: usize, build: fn(usize) -> View) {
        let (mut mount, mut nodes, mut rename, mut team) = (f64::MAX, 0, f64::MAX, f64::MAX);
        for _ in 0..3 {
            let t0 = Instant::now();
            let mut h = Window::headless(move || build(n));
            mount = mount.min(t0.elapsed().as_secs_f64() * 1e3);
            nodes = h.kernel().live_nodes();
            let t0 = Instant::now();
            for i in 0..20 {
                octara_examples::scenarios::external(&mut h, "rename", &format!("{}:a{i:03}", 3 + 3 * i)).unwrap();
            }
            rename = rename.min(t0.elapsed().as_secs_f64() * 1e6 / 20.0);
            let t0 = Instant::now();
            for i in 0..20 {
                octara_examples::scenarios::external(&mut h, "team", &format!("{}:{}", 3 + 3 * i, apps::TEAMS[i % 2])).unwrap();
            }
            team = team.min(t0.elapsed().as_secs_f64() * 1e6 / 20.0);
        }
        println!("| {label} | {n} | {mount:.1} | {nodes} | {rename:.1} | {team:.1} |");
    }
    for n in [1_000usize, 10_000, 100_000] {
        run("each (grouped)", n, apps::contacts);
        run("flatten + virtual_each", n, apps::contacts_virtual);
    }
    println!();
}

fn streaming_text() {
    println!("## streaming one token into a long body: virtual chat, layout ON, scrolled to it (min of 3)\n");
    println!("Paragraphed: ~500-byte paragraphs, like a model's reply. One paragraph: no line breaks at all.\n");
    println!(
        "| body | String body, paragraphed: µs / token | Text body, paragraphed: µs / token | bytes written | Text body, one paragraph: µs / token |"
    );
    println!("| ---: | ---: | ---: | ---: | ---: |");
    fn run(build: fn(usize) -> View, body: usize, paragraphs: bool) -> (f64, u64) {
        let mut best = (f64::MAX, 0);
        for _ in 0..3 {
            let mut h = Window::headless(move || build(1_000));
            octara_examples::scenarios::external(&mut h, "user", "go").unwrap();
            h.dispatch(Event::ScrollTo("messages".into(), 0.0, 1e9)).unwrap();
            // 500 bytes of words, ending a paragraph or not.
            let words = "lorem ipsum dolor sit amet ".repeat(20);
            let chunk = if paragraphs { format!("{}\n\n", &words[..498]) } else { words[..500].to_string() };
            for _ in 0..body / 500 {
                octara_examples::scenarios::external(&mut h, "token", &chunk).unwrap();
            }
            h.kernel_mut().reset_counters();
            let t0 = Instant::now();
            for _ in 0..100 {
                octara_examples::scenarios::external(&mut h, "token", "tok ").unwrap();
            }
            let us = t0.elapsed().as_secs_f64() * 1e6 / 100.0;
            if us < best.0 {
                best = (us, h.kernel().counters().text_bytes / 100);
            }
        }
        best
    }
    for body in [1_000usize, 10_000, 100_000] {
        let (a, _) = run(apps::chat_virtual, body, true);
        let (b, bb) = run(apps::chat_stream, body, true);
        let (c, _) = run(apps::chat_stream, body, false);
        println!("| {} KB | {a:.1} | {b:.1} | {bb} | {c:.1} |", body / 1_000);
    }
    println!();
}

/// What a frame costs after the view settles: building the display list and the accessibility
/// tree with nothing changed, and after one press (min of 3 runs of 20 frames, µs per frame).
fn frames() {
    println!("## frame: display list and accessibility tree, layout ON (µs per frame, min of 3)\n");
    println!(
        "| app | nodes | items | display list (idle) | press + display list | a11y tree (whole) | a11y update after a press (nodes) |"
    );
    println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: |");
    type Case = (&'static str, fn() -> View, &'static str);
    let cases: [Case; 4] = [
        ("todo 1000", || apps::todo(1000), "toggle-1"),
        ("chat 300", || apps::chat(300), "composer"),
        ("contacts 500", || apps::contacts(500), "header"),
        ("showcase", octara_examples::showcase::showcase, "add"),
    ];
    for (name, view, target) in cases {
        let mut app = Window::headless(view);
        app.resize(1024.0, 768.0);
        let items = app.display_list().items().len();
        let nodes = app.kernel().counters().nodes_created - app.kernel().counters().nodes_destroyed;
        let time = |app: &mut Window, press: bool| {
            let mut best = f64::MAX;
            for _ in 0..3 {
                let t0 = Instant::now();
                for _ in 0..20 {
                    if press {
                        let _ = app.tap(target);
                    }
                    std::hint::black_box(app.display_list());
                }
                best = best.min(t0.elapsed().as_secs_f64() * 1e6 / 20.0);
            }
            best
        };
        let idle = time(&mut app, false);
        let pressed = time(&mut app, true);
        let mut a11y = f64::MAX;
        for _ in 0..3 {
            let t0 = Instant::now();
            for _ in 0..20 {
                std::hint::black_box(app.accessibility_tree());
            }
            a11y = a11y.min(t0.elapsed().as_secs_f64() * 1e6 / 20.0);
        }
        // Incremental: after a press, what the platform is sent (the press itself not timed).
        let _ = app.accessibility_update();
        let (mut update, mut sent) = (f64::MAX, 0);
        for _ in 0..3 {
            let mut total = 0.0;
            for _ in 0..20 {
                let _ = app.tap(target);
                let t0 = Instant::now();
                sent = std::hint::black_box(app.accessibility_update()).nodes.len();
                total += t0.elapsed().as_secs_f64();
            }
            update = update.min(total * 1e6 / 20.0);
        }
        println!("| {name} | {nodes} | {items} | {idle:.0} | {pressed:.0} | {a11y:.0} | {update:.0} ({sent}) |");
    }
    println!();
}

/// A lookup join (`inbox`): each person is on 1/20 of the messages. A rename updates their rows;
/// a change the rows do not show updates none.
fn joins() {
    println!("## join: inbox, layout off, min of 3 (µs per op; rows rewritten)\n");
    println!("| messages | rename one person | toggle a field rows do not show |");
    println!("| ---: | ---: | ---: |");
    for n in [1_000usize, 10_000] {
        let time = |event: &str, payload: &str| {
            let mut best = (f64::MAX, 0u64);
            for _ in 0..3 {
                let mut h = Window::headless(move || apps::inbox(n));
                h.kernel_mut().set_layout_enabled(false);
                h.kernel_mut().reset_counters();
                let t0 = Instant::now();
                for i in 0..20 {
                    let p = if event == "rename" { format!("{payload}:n{i}") } else { payload.to_string() };
                    octara_examples::scenarios::external(&mut h, event, &p).unwrap();
                }
                let us = t0.elapsed().as_secs_f64() * 1e6 / 20.0;
                if us < best.0 {
                    best = (us, h.kernel().counters().prop_changes / 20);
                }
            }
            best
        };
        let (r, ru) = time("rename", "4");
        let (o, ou) = time("online", "4");
        println!("| {n} | {r:.1} ({ru}) | {o:.1} ({ou}) |");
    }
    println!();
}

/// A chat of 40 Markdown answers with the last one streaming in (3 characters a token, the view
/// following it), with and without layout motion: per token, the frame's cost and what it built,
/// and the accessibility update's.
fn stream() {
    use octara::platform::{HeadlessPlatform, WindowOptions};
    use octara_kit::markdown::markdown;
    use std::time::Duration;
    println!("## stream: a 41st answer streaming under 40 (per token, after the first second)\n");
    println!("| motion | display list (µs) | parts | drawings built | glyph runs built | a11y update (µs) | a11y compared | a11y sent |");
    println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    let para = "Some **answer** text with `code` and a [link](https://x.dev) that runs over a couple of lines in the window.\n\n";
    let msg: String = (0..6).map(|_| para).collect::<String>() + "- a\n- b\n\n```\nfn f() {}\n```\n";
    for motion in [false, true] {
        let app = App::new(HeadlessPlatform::default());
        let live = app.run(|| LiveText::new(""));
        let stream = live.clone();
        let m = msg.clone();
        let mut w = app
            .open_window(WindowOptions::new("Chat"), move || {
                let m = m.clone();
                let c = col()
                    .id("list")
                    .size_full()
                    .overflow_y_scroll()
                    .gap_4()
                    .p_4()
                    .children((0..40).map(move |_| markdown(m.as_str())))
                    .child(markdown(&stream));
                if motion { c.motion(Spring::SNAP).into() } else { c.into() }
            })
            .unwrap();
        w.resize(1024.0, 768.0);
        let _ = w.accessibility_update();
        let chars: Vec<char> = msg.chars().collect();
        let tokens: Vec<String> = chars.chunks(3).map(|c| c.iter().collect()).collect();
        let (mut paint, mut a11y, mut items, mut sent, mut n) = (0.0, 0.0, 0, 0, 0);
        let mut start = None;
        for (i, t) in tokens.iter().enumerate() {
            app.run(|| live.append(t));
            w.dispatch(Event::ScrollTo("list".into(), 0.0, 1.0e6)).unwrap();
            app.advance(Duration::from_millis(16));
            let t0 = Instant::now();
            let dl = w.display_list();
            let p = t0.elapsed().as_secs_f64();
            let t0 = Instant::now();
            let u = w.accessibility_update();
            let a = t0.elapsed().as_secs_f64();
            // What entered settles in the first second.
            if i < 60 {
                continue;
            }
            let k = w.kernel();
            start.get_or_insert((k.drawings(), k.a11y_compared()));
            paint += p;
            a11y += a;
            items += dl.parts.len();
            sent += u.nodes.len();
            n += 1;
        }
        let k = w.kernel();
        let ((d0, c0), d1, c1) = (start.unwrap(), k.drawings(), k.a11y_compared());
        let f = n as f64;
        println!(
            "| {motion} | {:.0} | {} | {:.1} | {:.1} | {:.0} | {:.0} | {:.1} |",
            paint / f * 1e6,
            items / n,
            (d1.0 - d0.0) as f64 / f,
            (d1.2 - d0.2) as f64 / f,
            a11y / f * 1e6,
            (c1 - c0) as f64 / f,
            sent as f64 / f
        );
    }
    println!();
}

fn main() {
    if std::env::args().any(|a| a == "--frames") {
        return frames();
    }
    if std::env::args().any(|a| a == "--stream") {
        return stream();
    }
    if std::env::args().any(|a| a == "--joins") {
        return joins();
    }
    frames();
    stream();
    joins();
    steps::<Apps>();
    scaling();
    virtualization();
    operators();
    grouped_virtual();
    streaming_text();
}
