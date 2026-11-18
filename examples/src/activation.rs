//! Real-window fixture for retained activation and focus-retirement rules.
use octara::accesskit::Action;
use octara::kernel::a11y::a11y_id;
use octara::prelude::*;
use octara_kit::button;

#[ui]
pub fn view() -> View {
    let mode = state("mode", || 0u8);
    let hits = state("hits", || 0u32);
    let blurs = state("blurs", || 0u32);
    let dismissals = state("dismissals", || 0u32);
    let accepted = state("accepted", || false);
    let value = state("value", || "中文 input".to_owned());
    for (command, next) in [("enable", 0), ("disable", 1), ("hide", 2), ("modal", 3)] {
        on_command(command, move || mode.set(next));
    }
    // Explicit probes exercise the public semantic/AccessKit adapters in a real frame. The
    // Inspector invokes these commands; this fixture does not synthesize OS accessibility input.
    on_command("probe.semantic", move || {
        let mut w = App::current().window(current_window()).unwrap();
        accepted.set(w.dispatch(Event::Press("activate".into())).is_ok());
    });
    on_command("probe.accessibility", move || {
        let mut w = App::current().window(current_window()).unwrap();
        let target = a11y_id(w.kernel().find("activate").unwrap());
        accepted.set(w.event(InputEvent::Accessibility { target, action: Action::Click, data: None }));
    });
    col()
        .size_full()
        .p_6()
        .gap_4()
        .bg(Token::Surface)
        .child(text("Activation and focus lifecycle").text_xl())
        .child(text(move || format!("Hits: {}", hits.get())).id("hits"))
        .child(text(move || format!("Blurs: {}", blurs.get())).id("blurs"))
        .child(text(move || format!("Dismissals: {}", dismissals.get())).id("dismissals"))
        .child(text(move || format!("Accepted: {}", accepted.get())).id("accepted"))
        .child(
            col()
                .gap_4()
                .style_with(move |s| if mode.get() == 2 { s.hidden() } else { s.flex() })
                .child(
                    button("Activate")
                        .id("activate")
                        .self_start()
                        .disabled(move || mode.get() == 1)
                        .on_press(move || hits.update(|n| *n += 1)),
                )
                .child(
                    text_input()
                        .id("field")
                        .label("Input")
                        .h(44)
                        .w_full()
                        .value(value)
                        .disabled(move || mode.get() == 1)
                        .on_input(move |next| value.set(next))
                        .on_blur(move || blurs.update(|n| *n += 1)),
                ),
        )
        .child(show(
            move || mode.get() == 3,
            move || {
                el().id("modal")
                    .role(Role::Dialog)
                    .label("Dismissible disabled container")
                    .absolute()
                    .top(360)
                    .left(24)
                    .w(300)
                    .h(100)
                    .p_4()
                    .bg(Token::SurfaceHover)
                    .disabled(true)
                    .modal(true, move || {
                        dismissals.update(|n| *n += 1);
                        mode.set(0);
                    })
                    .child(text("Escape closes this dialog"))
            },
        ))
        .into()
}
