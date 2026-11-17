//! Focus callbacks share the bounded window-preparation budget with deferred commands.

use octara::prelude::*;
use octara::task::WorkKind;
use std::cell::Cell;
use std::rc::Rc;

#[ui]
fn redirect_target(id: &'static str, next: &'static str, enabled: Rc<Cell<bool>>, calls: Rc<Cell<usize>>, limit: Option<usize>) -> El {
    el().id(id)
        .role(Role::Button)
        .label(id)
        .size_8()
        .focusable()
        .bg(Token::Surface)
        .focus(|p| p.bg(Token::Accent))
        .on_press(|| {})
        .on_focus(move || {
            let count = calls.get() + 1;
            calls.set(count);
            if enabled.get() && limit.is_none_or(|limit| count < limit) {
                focus(next);
            }
        })
}

fn focus_chain(limit: Option<usize>) -> (Window, Rc<Cell<usize>>) {
    let enabled = Rc::new(Cell::new(false));
    let calls = Rc::new(Cell::new(0));
    let observed = calls.clone();
    let window = Window::headless(move || {
        let (start, stop) = (enabled.clone(), enabled.clone());
        col()
            .size_full()
            .child(redirect_target("a", "b", enabled.clone(), observed.clone(), limit))
            .child(redirect_target("b", "a", enabled.clone(), observed.clone(), limit))
            .child(el().id("start").role(Role::Button).label("Start").size_8().on_press(move || {
                start.set(true);
                focus("a");
            }))
            .child(el().id("stop").role(Role::Button).label("Stop").size_8().on_press(move || {
                stop.set(false);
                focus("a");
            }))
            .into()
    });
    (window, calls)
}

#[test]
fn runaway_focus_feedback_stays_pending_without_presenting_and_can_stop() {
    let (mut window, calls) = focus_chain(None);
    let app = window.app();
    let frames = window.frames_presented();

    window.dispatch(Event::Press("start".into())).unwrap();

    assert_eq!(calls.get(), 16, "one operation has a bounded focus allowance");
    assert_eq!(window.frames_presented(), frames, "unfinished focus must not reach the host");
    assert!(app.pending().iter().any(|work| work.kind == WorkKind::Update));
    assert!(app.diagnostics().iter().any(|diagnostic| diagnostic.code == "LAYOUT_LOOP"));
    let diagnostics = app.diagnostic_counts();
    app.reset_work();
    for _ in 0..3 {
        assert!(!window.unsettled().is_empty());
        assert!(app.pending().iter().any(|work| work.kind == WorkKind::Update));
        assert_eq!(app.next_deadline(), None, "a suspended focus chain must not schedule a busy retry");
    }
    assert_eq!(calls.get(), 16, "pending queries do not resume callbacks");
    assert_eq!(app.work().closures_run, 0);
    assert_eq!(app.diagnostic_counts(), diagnostics);
    assert_eq!(window.frames_presented(), frames);

    window.dispatch(Event::Press("stop".into())).unwrap();

    assert_eq!(window.inspect().focus.as_deref(), Some("a"));
    assert!(app.pending().is_empty(), "{:?}", app.pending());
    assert!(window.frames_presented() > frames);
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    app.shutdown();
}

#[test]
fn a_finite_focus_chain_continues_at_a_later_explicit_poll() {
    let (mut window, calls) = focus_chain(Some(20));
    let app = window.app();
    let frames = window.frames_presented();

    window.dispatch(Event::Press("start".into())).unwrap();

    assert_eq!(calls.get(), 16);
    assert!(!window.unsettled().is_empty());
    assert_eq!(window.frames_presented(), frames);
    assert_eq!(app.next_deadline(), None);

    app.poll();

    assert_eq!(calls.get(), 20, "the cutoff retains the next focus request");
    assert_eq!(window.inspect().focus.as_deref(), Some("b"));
    assert!(window.unsettled().is_empty(), "{:?}", window.unsettled());
    assert!(window.frames_presented() > frames);
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    app.shutdown();
}

#[test]
fn the_last_command_cannot_present_before_blur_feedback_is_prepared() {
    for hide in [false, true] {
        let commands = Rc::new(Cell::new(0));
        let blurs = Rc::new(Cell::new(0));
        let (command_calls, blur_calls) = (commands.clone(), blurs.clone());
        let mut window = Window::headless(move || {
            let retire = state("retire", || false);
            let width = state("width", || 40.0);
            let command_calls = command_calls.clone();
            let blur_calls = blur_calls.clone();
            on_command("last", move || {
                let count = command_calls.get() + 1;
                command_calls.set(count);
                if count == 16 {
                    retire.set(true);
                } else {
                    invoke_after_update("last");
                }
            });
            col()
                .size_full()
                .child(
                    el().id("target")
                        .role(Role::Button)
                        .label("Target")
                        .size_8()
                        .focusable()
                        .bg(Token::Surface)
                        .focus(|p| p.bg(Token::Accent))
                        .disabled(move || !hide && retire.get())
                        .style_with(move |p| if hide && retire.get() { p.hidden() } else { p.flex() })
                        .on_press(|| {})
                        .on_blur(move || {
                            blur_calls.set(blur_calls.get() + 1);
                            width.set(140.0);
                        }),
                )
                .child(el().id("result").h(20).style_with(move |p| p.w(width.get())))
                .child(el().id("queue").role(Role::Button).label("Queue").size_8().on_press(|| invoke_after_update("last")))
                .into()
        });
        window.dispatch(Event::Focus("target".into())).unwrap();
        let frames = window.frames_presented();

        window.dispatch(Event::Press("queue".into())).unwrap();

        assert_eq!(commands.get(), 16);
        assert_eq!(blurs.get(), 1, "hidden or disabled controls still finish their focus lifecycle");
        assert_eq!(window.frames_presented(), frames, "hide={hide}: blur feedback is not a completed frame");
        assert!(!window.unsettled().is_empty(), "hide={hide}: the final command left layout feedback");
        assert_eq!(window.app().next_deadline(), None);

        window.app().poll();

        let width = {
            let kernel = window.kernel();
            kernel.rect(kernel.find("result").unwrap()).2
        };
        assert_eq!(width, 140.0);
        assert!(window.inspect().focus.is_none());
        assert!(window.unsettled().is_empty(), "{:?}", window.unsettled());
        assert!(window.frames_presented() > frames);
        assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
        window.app().shutdown();
    }
}

#[test]
fn a_command_only_cutoff_still_discards_the_queue_and_finishes_its_frame() {
    let calls = Rc::new(Cell::new(0));
    let observed = calls.clone();
    let mut window = Window::headless(move || {
        let count = state("count", || 0);
        let observed = observed.clone();
        on_command("loop", move || {
            observed.set(observed.get() + 1);
            count.update(|count| *count += 1);
            invoke_after_update("loop");
        });
        col()
            .size_full()
            .child(text(move || count.get().to_string()).id("count"))
            .child(el().id("queue").role(Role::Button).label("Queue").size_8().on_press(|| invoke_after_update("loop")))
            .into()
    });
    let frames = window.frames_presented();

    window.dispatch(Event::Press("queue".into())).unwrap();

    assert_eq!(calls.get(), 16);
    assert_eq!(window.kernel().text_of("count").as_deref(), Some("16"));
    assert!(window.unsettled().is_empty(), "{:?}", window.unsettled());
    assert!(window.frames_presented() > frames);
    assert!(window.app().diagnostics().iter().any(|diagnostic| diagnostic.code == "COMMAND_QUEUE_LIMIT"));
    assert!(!window.app().diagnostics().iter().any(|diagnostic| diagnostic.code == "LAYOUT_LOOP"));

    window.app().poll();

    assert_eq!(calls.get(), 16, "discarded commands do not resume at the next operation");
    assert!(window.unsettled().is_empty());
    window.app().shutdown();
}
