use octara::app::{InputEvent, Phase, PointerEvent, PointerKind};
use octara::prelude::*;
use octara_examples::panel_scroll::{self, LIST};
use std::time::Duration;

fn contact(window: &mut Window, phase: Phase, (x, y): (f32, f32), up: f32) {
    window.event(InputEvent::Pointer(PointerEvent::touch(1, phase, x, y - up)));
}

fn geometry(window: &Window) -> (f32, f32, f32) {
    let k = window.kernel();
    let list = k.find(LIST).unwrap();
    let height = k.client_size(list).1;
    (height, k.scroll_offset(list).1, k.scroll_size(list).1 - height)
}

fn report(window: &Window) -> [f32; 6] {
    let report = window.inspect().touch_scroll.unwrap();
    let vectors = [report.input, report.before, report.layout_correction, report.viewport, report.after, report.remaining];
    assert!(vectors.iter().all(|[dx, _]| *dx == 0.0));
    vectors.map(|[_, dy]| dy)
}

#[test]
fn the_same_touch_distributes_forward_and_reverse_then_ignores_a_late_release() {
    let mut window = Window::headless(panel_scroll::view);
    let (_, at) = window.pointer_target("pan-place-0").unwrap();
    contact(&mut window, Phase::Down, at, 0.0);
    assert!(window.kernel().state_of("pan-place-0").unwrap().contains(State::PRESSED));
    assert_eq!(window.kernel().text_of("pan-activations").as_deref(), Some("Activations 0"));

    window.app().advance(Duration::from_millis(16));
    let frames = window.frames_presented();
    contact(&mut window, Phase::Move, at, 30.0);
    assert_eq!(geometry(&window), (112.0, 18.0, 88.0));
    assert_eq!(report(&window), [30.0, 12.0, 0.0, 18.0, 0.0, 0.0]);
    assert_eq!(window.frames_presented(), frames + 1, "expansion and scrolling are published together");
    assert!(!window.kernel().state_of("pan-place-0").unwrap().contains(State::PRESSED));

    window.app().advance(Duration::from_millis(16));
    contact(&mut window, Phase::Move, at, 5.0);
    assert_eq!(geometry(&window), (105.0, 0.0, 95.0));
    assert_eq!(report(&window), [-25.0, 0.0, 0.0, -18.0, -7.0, 0.0]);
    assert_eq!(window.held_pointer(), Some((1, PointerKind::Touch)));

    contact(&mut window, Phase::Cancel, at, 5.0);
    assert!(window.held_pointer().is_none());
    contact(&mut window, Phase::Move, at, 55.0);
    contact(&mut window, Phase::Up, at, 5.0);
    window.app().advance(Duration::from_millis(200));
    assert_eq!(geometry(&window), (105.0, 0.0, 95.0));
    assert_eq!(report(&window), [-25.0, 0.0, 0.0, -18.0, -7.0, 0.0]);
    assert_eq!(window.kernel().text_of("pan-activations").as_deref(), Some("Activations 0"));
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

#[test]
fn viewport_clamping_is_separate_from_displacement_and_reaches_the_same_hit_map() {
    let mut window = Window::headless(panel_scroll::view);
    let (_, (x, y)) = window.pointer_target("pan-place-0").unwrap();
    window.wheel(x, y, 0.0, 95.0);
    assert!(window.inspect().touch_scroll.is_none());
    assert_eq!(geometry(&window), (100.0, 95.0, 100.0));
    let (_, at) = window.pointer_target("pan-place-2").unwrap();
    contact(&mut window, Phase::Down, at, 0.0);
    let frames = window.frames_presented();
    contact(&mut window, Phase::Move, at, 30.0);
    assert_eq!(report(&window), [30.0, 12.0, -7.0, 0.0, 0.0, 18.0]);
    assert_eq!(geometry(&window), (112.0, 88.0, 88.0));
    assert_eq!(window.frames_presented(), frames + 1);

    let k = window.kernel();
    let list = k.find(LIST).unwrap();
    let (x, y, _, height) = k.rect(list);
    let top = k.hit_test(x + 10.0, y + 1.0).unwrap();
    let end = k.hit_test(x + 10.0, y + height - 1.0).unwrap();
    assert!(k.is_within(top, k.find("pan-place-2").unwrap()));
    assert!(k.is_within(end, k.find("pan-place-4").unwrap()));
    assert!(!k.hit_test(x + 10.0, y - 1.0).is_some_and(|hit| k.is_within(hit, list)));
}

#[test]
fn a_rebuilt_viewport_cannot_adopt_the_old_touch() {
    let mut window = Window::headless(panel_scroll::view);
    let (_, at) = window.pointer_target("pan-place-0").unwrap();
    contact(&mut window, Phase::Down, at, 0.0);
    contact(&mut window, Phase::Move, at, 30.0);
    let target = window.inspect().find(LIST).unwrap().target.clone();
    window.invoke("pan.replace").unwrap();
    assert_ne!(window.inspect().find(LIST).unwrap().target, target);
    assert!(window.held_pointer().is_none());
    contact(&mut window, Phase::Move, at, 55.0);
    contact(&mut window, Phase::Up, at, 55.0);
    window.app().advance(Duration::from_millis(200));
    assert_eq!(geometry(&window), (112.0, 0.0, 88.0));
    assert_eq!(window.inspect().touch_scroll.unwrap().target, target);
    assert_eq!(report(&window), [30.0, 12.0, 0.0, 18.0, 0.0, 0.0]);
    assert_eq!(window.kernel().text_of("pan-activations").as_deref(), Some("Activations 0"));
}

#[test]
fn losing_app_activity_retires_a_scroll_contact_before_resume() {
    let mut window = Window::headless(panel_scroll::view);
    let (_, at) = window.pointer_target("pan-place-0").unwrap();
    contact(&mut window, Phase::Down, at, 0.0);
    contact(&mut window, Phase::Move, at, 30.0);
    assert_eq!(geometry(&window), (112.0, 18.0, 88.0));

    window.event(InputEvent::Active(false));
    assert!(window.held_pointer().is_none());
    window.event(InputEvent::Active(true));
    contact(&mut window, Phase::Move, at, 58.0);
    contact(&mut window, Phase::Up, at, 58.0);
    window.app().advance(Duration::from_millis(200));
    assert_eq!(geometry(&window), (112.0, 18.0, 88.0));
    assert_eq!(window.kernel().text_of("pan-activations").as_deref(), Some("Activations 0"));
}
