//! Diagnostics regression tests: each variant contains one deliberate mistake and asserts how it
//! surfaces (a diagnostic code, a panic, or correct behaviour when the API rules it out).
//! Mistake ids follow `docs/design/reactive-core.md`.

use octara::prelude::*;
use octara_examples::scenarios::{Check, press};
use octara_kit::button;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct Variant {
    id: &'static str,
    intent: &'static str,
    build: fn() -> Window,
    check: fn(&mut Window) -> Check,
}

thread_local! {
    static PATCHED: Cell<bool> = const { Cell::new(false) };
}

fn expect_count(h: &mut Window, presses: usize, want: &str) -> Check {
    for _ in 0..presses {
        press(h, "inc")?;
    }
    match h.kernel().text_of("count") {
        Some(t) if t == want => Ok(()),
        other => Err(format!("count shows {other:?}, want {want:?}")),
    }
}

fn counter_with(body: fn(Signal<i64>) -> ()) -> View {
    let count = state("count", || 0i64);
    body(count);
    col()
        .child(text(move || format!("Count: {}", count.get())).id("count"))
        .child(button("+1").id("inc").on_press(move || count.update(|n| *n += 1)))
        .into()
}

fn panel(id: &'static str, keyed: bool) -> View {
    let body = move || {
        let open = state("open", || false);
        col()
            .child(button("t").id(format!("{id}-toggle")).on_press(move || open.update(|o| *o = !*o)))
            .child(show(move || open.get(), move || text("body").id(format!("{id}-body")).into()))
            .into()
    };
    if keyed { component_keyed("Panel", id, body) } else { component("Panel", body) }
}

fn panels(keyed: bool) -> View {
    let mut kids = Vec::new();
    if PATCHED.with(|p| p.get()) {
        kids.push(panel("new", keyed));
    }
    kids.push(panel("account", keyed));
    col().children(kids).into()
}

fn reload_keeps_panel(h: &mut Window) -> Check {
    press(h, "account-toggle")?;
    PATCHED.with(|p| p.set(true));
    h.app().hot_reload();
    let (a, n) = (h.kernel().exists("account-body"), h.kernel().exists("new-body"));
    if a && !n { Ok(()) } else { Err(format!("after the patch: account open = {a}, new open = {n}")) }
}

fn variants() -> Vec<Variant> {
    vec![
        Variant {
            id: "I1",
            intent: "display state without subscribing",
            build: || {
                Window::headless(|| {
                    let count = state("count", || 0i64);
                    col()
                        .child(text(format!("Count: {}", count.get())).id("count"))
                        .child(button("+1").id("inc").on_press(move || count.update(|n| *n += 1)))
                        .into()
                })
            },
            check: |h| expect_count(h, 2, "Count: 2"),
        },
        Variant {
            id: "I2",
            intent: "write state while building",
            build: || Window::headless(|| counter_with(|c| c.set(10))),
            check: |h| expect_count(h, 1, "Count: 11"),
        },
        Variant {
            id: "I3",
            intent: "duplicate list keys",
            build: || {
                Window::headless(|| {
                    let items = state_list("items", || vec![1u32, 2, 3], |_| 0u32);
                    col().child(each(move || items.all(), |k, _| text(format!("{k}")).into())).id("list").into()
                })
            },
            check: |h| {
                let n = h.kernel().find("list").map(|l| h.kernel().children(l).len());
                if n == Some(3) { Ok(()) } else { Err(format!("list shows {n:?} rows, want 3")) }
            },
        },
        Variant {
            id: "I4-watch",
            intent: "clamp written with `watch`",
            build: || {
                Window::headless(|| {
                    counter_with(|c| {
                        watch(
                            move || c.get(),
                            move |v, _| {
                                if *v > 3 {
                                    c.set(3)
                                }
                            },
                        )
                    })
                })
            },
            check: |h| expect_count(h, 5, "Count: 3"),
        },
        Variant {
            id: "I4b",
            intent: "two watches feeding each other",
            build: || {
                Window::headless(|| {
                    let a = state("a", || 0i64);
                    let b = state("b", || 0i64);
                    watch(move || a.get(), move |v, _| b.set(v + 1));
                    watch(move || b.get(), move |v, _| a.set(v + 1));
                    col()
                        .child(text(move || format!("Count: {}", a.get())).id("count"))
                        .child(button("+").id("inc").on_press(move || a.update(|n| *n += 1)))
                        .into()
                })
            },
            check: |h| {
                press(h, "inc")?;
                Err("a loop has no correct value; reaching here means it terminated".into())
            },
        },
        Variant {
            id: "I5",
            intent: "use the item of a removed row",
            build: || {
                Window::headless(|| {
                    let items = state_list("items", || vec![1u32, 2], |x| *x);
                    let kept: Rc<RefCell<Vec<ReadSignal<u32>>>> = Rc::default();
                    let k2 = kept.clone();
                    col()
                        .child(col().child(each(
                            move || items.all(),
                            move |k, item| {
                                k2.borrow_mut().push(item);
                                text(format!("row {k}")).into()
                            },
                        )))
                        .child(button("drop").id("drop").on_press(move || items.remove(&1)))
                        .child(button("+").id("inc").on_press(move || {
                            let first = kept.borrow()[0];
                            let _ = first.get_untracked();
                        }))
                        .into()
                })
            },
            check: |h| {
                press(h, "drop")?;
                press(h, "inc")
            },
        },
        Variant {
            id: "I6",
            intent: "handler captures a stale value",
            build: || {
                Window::headless(|| {
                    let count = state("count", || 0i64);
                    let n = count.get_untracked();
                    col()
                        .child(text(move || format!("Count: {}", count.get())).id("count"))
                        .child(button("+1").id("inc").on_press(move || count.set(n + 1)))
                        .into()
                })
            },
            check: |h| expect_count(h, 2, "Count: 2"),
        },
        Variant {
            id: "I7",
            intent: "patch inserts a same-named component first",
            build: || Window::headless(|| panels(false)),
            check: reload_keeps_panel,
        },
        Variant {
            id: "I7-keyed",
            intent: "same patch, `component_keyed`",
            build: || Window::headless(|| panels(true)),
            check: reload_keeps_panel,
        },
        Variant {
            id: "I8-watch",
            intent: "helper reading other state inside a `watch` callback",
            build: || {
                Window::headless(|| {
                    let count = state("count", || 0i64);
                    let theme = state("theme", || "light");
                    let log = state("log", Vec::<String>::new);
                    let label = move || format!("Count: {}", count.get());
                    watch(
                        move || theme.get(),
                        move |t, _| {
                            let line = format!("{t} {}", label());
                            log.update(|l| l.push(line));
                        },
                    );
                    col()
                        .child(text(move || format!("{} log lines", log.with(|l| l.len()))).id("log"))
                        .child(text(label).id("count"))
                        .child(button("+1").id("inc").on_press(move || count.update(|n| *n += 1)))
                        .into()
                })
            },
            check: |h| {
                press(h, "inc")?;
                press(h, "inc")?;
                match h.kernel().text_of("log") {
                    Some(t) if t == "1 log lines" => Ok(()),
                    other => Err(format!("log shows {other:?}")),
                }
            },
        },
        Variant {
            id: "I9",
            intent: "list row reads its item once",
            build: || {
                Window::headless(|| {
                    let items = state_list("items", || vec![(1u32, false)], |t| t.0);
                    col()
                        .child(col().child(each(move || items.all(), |_, t| el().role(Role::CheckBox).id("box").checked(t.get().1).into())))
                        .child(button("t").id("inc").on_press(move || items.update(&1, |t| t.1 = !t.1)))
                        .into()
                })
            },
            check: |h| {
                press(h, "inc")?;
                match h.kernel().checked_of("box") {
                    Some(true) => Ok(()),
                    other => Err(format!("checkbox is {other:?}")),
                }
            },
        },
        Variant {
            id: "I10",
            intent: "component state without a key",
            build: || {
                Window::headless(|| {
                    let count = Signal::new(0i64);
                    col()
                        .child(text(move || format!("Count: {}", count.get())).id("count"))
                        .child(button("+1").id("inc").on_press(move || count.update(|n| *n += 1)))
                        .into()
                })
            },
            check: |h| {
                expect_count(h, 2, "Count: 2")?;
                h.app().hot_reload();
                match h.kernel().text_of("count") {
                    Some(t) if t == "Count: 2" => Ok(()),
                    other => Err(format!("after reload count shows {other:?}")),
                }
            },
        },
        Variant {
            id: "M1",
            intent: "memo writes state",
            build: || {
                Window::headless(|| {
                    let count = state("count", || 0i64);
                    let doubled = memo(move || {
                        count.set(99);
                        count.get() * 2
                    });
                    col()
                        .child(text(move || format!("Count: {}", doubled.get() / 2)).id("count"))
                        .child(button("+").id("inc").on_press(move || count.update(|n| *n += 1)))
                        .into()
                })
            },
            check: |h| expect_count(h, 1, "Count: 1"),
        },
    ]
}

/// What a variant must produce: these diagnostic codes, a panic containing this text, and
/// whether its behaviour check passes.
struct Expect {
    codes: &'static [&'static str],
    panic: Option<&'static str>,
    passes: bool,
}

fn expected(id: &str) -> Expect {
    let e = |codes, panic, passes| Expect { codes, panic, passes };
    match id {
        "I1" => e(&["STRICT_READ_UNTRACKED"], None, false),
        "I2" => e(&["WRITE_IN_BUILD"], None, false),
        "I3" => e(&["DUPLICATE_KEY"], None, false),
        "I4-watch" => e(&[], None, true),
        "I4b" => e(&["FLUSH_RUNAWAY"], None, false),
        "I5" => e(&[], Some("READ_DISPOSED"), false),
        "I6" => e(&[], None, false), // known silent: an explicit `get_untracked` capture
        "I7" => e(&["AMBIGUOUS_COMPONENT"], None, false),
        "I7-keyed" => e(&[], None, true),
        "I8-watch" => e(&[], None, true),
        "I9" => e(&["STRICT_READ_UNTRACKED"], None, false),
        "I10" => e(&["UNKEYED_STATE"], None, false),
        "M1" => e(&["WRITE_IN_DERIVE"], None, true),
        other => panic!("no expectation for {other}"),
    }
}

#[test]
fn every_mistake_surfaces_as_expected() {
    std::panic::set_hook(Box::new(|_| {}));
    let mut failures = Vec::new();
    for v in variants() {
        let (build, check, id) = (v.build, v.check, v.id);
        let (codes, panic, passed) = std::thread::spawn(move || {
            let out: Rc<RefCell<Vec<&'static str>>> = Rc::default();
            let o2 = out.clone();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let mut app = build();
                let res = check(&mut app);
                *o2.borrow_mut() = app.app().diagnostics().iter().map(|d| d.code.id).collect();
                res.is_ok()
            }));
            let codes = out.take();
            match r {
                Ok(passed) => (codes, None, passed),
                Err(p) => (codes, p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())), false),
            }
        })
        .join()
        .unwrap();
        let want = expected(id);
        let codes_ok = want.codes.iter().all(|c| codes.contains(c)) && codes.iter().all(|c| want.codes.contains(c));
        let panic_ok = match (want.panic, &panic) {
            (None, None) => true,
            (Some(p), Some(msg)) => msg.contains(p),
            _ => false,
        };
        if !(codes_ok && panic_ok && passed == want.passes) {
            failures.push(format!("{id} ({}): codes {codes:?}, panic {panic:?}, passes {passed}", v.intent));
        }
    }
    let _ = std::panic::take_hook();
    assert!(failures.is_empty(), "unexpected outcomes:\n{}", failures.join("\n"));
}
