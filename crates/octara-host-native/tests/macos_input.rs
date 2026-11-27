//! Exercises the AppKit responder on a desktop session with OCTARA_TEST_MACOS_INPUT=1.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() -> Result<(), octara_host_native::ShellError> {
    if std::env::var_os("OCTARA_TEST_MACOS_INPUT").is_some() {
        macos::run()?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod macos {
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{MainThreadMarker, msg_send};
    use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSView, NSWindow};
    use objc2_core_graphics::{
        CGDisplayIsActive, CGEvent, CGEventField, CGMainDisplayID, CGMomentumScrollPhase, CGScrollEventUnit, CGScrollPhase,
        CGSessionCopyCurrentDictionary,
    };
    use objc2_foundation::{NSArray, NSPoint, NSString, NSTimer};
    use objc2_quartz_core::CAMetalLayer;
    use octara::model::capture::FrameReceipt;
    use octara::model::input::{InteractionEvent, Key, Modifiers, WheelPhase, WheelStage};
    use octara::model::semantics::Role;
    use octara::prelude::*;
    use octara::runtime::Input;
    use octara_host_native::{ShellError, ShellHook, run_app_with_hook};
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    const TITLE: &str = "Octara AppKit input test";
    const BUTTON_LABEL: &str = "Confirm native accessibility";
    const WHEEL_SAMPLES: [(WheelPhase, f32); 2] =
        [(WheelPhase::Direct(WheelStage::Start), -7.0), (WheelPhase::Momentum(WheelStage::End), -3.0)];

    #[derive(Default)]
    struct Observations {
        shortcut: Cell<bool>,
        text_entered: Cell<bool>,
        pressed: Cell<bool>,
    }

    pub(super) fn run() -> Result<(), ShellError> {
        let display = CGMainDisplayID();
        if CGSessionCopyCurrentDictionary().is_none() || display == 0 || !CGDisplayIsActive(display) {
            eprintln!("Skipped AppKit input test because no desktop display is available");
            return Ok(());
        }

        let observations = Rc::new(Observations::default());
        let fixture = AppDefinition::new().window(WindowDefinition::new(
            WindowOptions { presentation: WindowPresentation { title: TITLE.into(), ..Default::default() }, ..Default::default() },
            {
                let state = observations.clone();
                move |cx| {
                    let field = cx.signal(String::new());
                    let shortcut = state.clone();
                    let editor = state.clone();
                    col()
                        .p(24)
                        .gap(8)
                        .child(
                            input()
                                .value(field)
                                .autofocus()
                                .on_input(move |write, value| {
                                    write.set(field, value.into());
                                    editor.text_entered.set(value == "a");
                                })
                                .on_key(move |_, key| {
                                    let handled =
                                        key.modifiers == Modifiers::META && key.code == Some('j') && matches!(key.key, Key::Char(_));
                                    shortcut.shortcut.set(shortcut.shortcut.get() || handled);
                                    handled
                                }),
                        )
                        .child(
                            el().role(Role::Button)
                                .label(BUTTON_LABEL)
                                .child(text(BUTTON_LABEL))
                                .on_press(move |_| state.pressed.set(true)),
                        )
                }
            },
        ));

        run_app_with_hook(
            fixture,
            Test {
                started: Instant::now(),
                observations,
                direct_and_momentum: Vec::new(),
                input_scheduled: false,
                ax_press_sent: false,
                done: false,
            },
        )?;
        println!("AppKit precise wheel, key equivalent, editor insertion and AccessKit action verified");
        Ok(())
    }

    struct Test {
        started: Instant,
        observations: Rc<Observations>,
        direct_and_momentum: Vec<(WheelPhase, f32)>,
        input_scheduled: bool,
        ax_press_sent: bool,
        done: bool,
    }

    impl ShellHook for Test {
        fn before_input(&mut self, _: &App, _: WindowId, input: &Input) {
            if let Input::Interaction(InteractionEvent::Wheel { delta, phase, .. }) = input
                && WHEEL_SAMPLES.iter().any(|sample| sample.0 == *phase && sample.1 == delta.y)
            {
                self.direct_and_momentum.push((*phase, delta.y));
            }
        }

        fn tick(&mut self, app: &mut App, presented: &dyn Fn(WindowId) -> Option<FrameReceipt>) -> Result<Option<Duration>, ShellError> {
            assert!(
                self.started.elapsed() < Duration::from_secs(15),
                "AppKit input stalled (scheduled {}, wheel {:?}, shortcut {}, text {}, AX press sent {}, pressed {})",
                self.input_scheduled,
                self.direct_and_momentum,
                self.observations.shortcut.get(),
                self.observations.text_entered.get(),
                self.ax_press_sent,
                self.observations.pressed.get(),
            );

            let delay = Ok(Some(Duration::from_millis(16)));
            let Some((&window, _)) = app.runtime().windows().next() else { return delay };
            if !presented(window).is_some_and(|frame| frame.gpu_complete) {
                return delay;
            }
            let Some(native) = appkit_window() else { return delay };
            let view = render_view(&native);
            if !self.input_scheduled {
                // SAFETY: The block captures no data. The selector calls run from
                // this main-thread timer after the ShellHook releases the driver.
                unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.0, false, &RcBlock::new(|_| synthesize_input())) };
                let _ = children(&view);
                self.input_scheduled = true;
            }
            if !self.ax_press_sent
                && let Some(button) = named_button(&view)
            {
                // SAFETY: The object was found by the AX button role and implements the press action.
                let handled: bool = unsafe { msg_send![&*button, accessibilityPerformPress] };
                assert!(handled, "AccessKit button rejected a native press");
                self.ax_press_sent = true;
            }
            if self.direct_and_momentum == WHEEL_SAMPLES
                && self.observations.shortcut.get()
                && self.observations.text_entered.get()
                && self.observations.pressed.get()
            {
                self.done = true;
                return Ok(None);
            }
            delay
        }

        fn shutdown_requested(&self) -> bool {
            self.done
        }
    }

    fn appkit_window() -> Option<Retained<NSWindow>> {
        let mtm = MainThreadMarker::new().expect("AppKit test runs on the main thread");
        NSApplication::sharedApplication(mtm).windows().iter().find(|window| window.title().to_string() == TITLE)
    }

    fn render_view(window: &NSWindow) -> Retained<NSView> {
        let view = window
            .contentView()
            .expect("window has a content container")
            .subviews()
            .iter()
            .find(|view| view.class().name() == c"OctaraView")
            .expect("window contains OctaraView");
        assert!(view.layer().is_some_and(|layer| layer.downcast::<CAMetalLayer>().is_ok()));
        view
    }

    fn synthesize_input() {
        let window = appkit_window().expect("visible fixture window");
        let view = render_view(&window);
        let context = view.inputContext().expect("focused Octara editor has an AppKit input context");
        assert_eq!(
            Retained::as_ptr(&context.client()).cast::<NSView>(),
            Retained::as_ptr(&view),
            "OctaraView is the AppKit text input client",
        );
        let direct = scroll_sample(7, CGScrollPhase::Began.0, CGMomentumScrollPhase::None);
        assert!(direct.hasPreciseScrollingDeltas());
        view.scrollWheel(&direct);

        let momentum = scroll_sample(3, 0, CGMomentumScrollPhase::End);
        assert!(momentum.hasPreciseScrollingDeltas());
        view.scrollWheel(&momentum);

        assert!(view.performKeyEquivalent(&key_sample(&window, "j", 38, NSEventModifierFlags::Command)));
        view.keyDown(&key_sample(&window, "a", 0, NSEventModifierFlags::empty()));
    }

    fn scroll_sample(vertical: i32, direct: u32, momentum: CGMomentumScrollPhase) -> Retained<NSEvent> {
        let event = CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Pixel, 1, vertical, 0, 0).expect("scroll event");
        CGEvent::set_integer_value_field(Some(&event), CGEventField::ScrollWheelEventScrollPhase, i64::from(direct));
        CGEvent::set_integer_value_field(Some(&event), CGEventField::ScrollWheelEventMomentumPhase, i64::from(momentum.0));
        NSEvent::eventWithCGEvent(&event).expect("AppKit scroll event")
    }

    fn key_sample(window: &NSWindow, character: &str, code: u16, modifiers: NSEventModifierFlags) -> Retained<NSEvent> {
        let character = NSString::from_str(character);
        NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown,
            NSPoint::ZERO,
            modifiers,
            0.0,
            window.windowNumber(),
            None,
            &character,
            &character,
            false,
            code,
        )
        .expect("AppKit key event")
    }

    fn children(object: &AnyObject) -> Vec<Retained<AnyObject>> {
        // SAFETY: Only walks the AppKit accessibility tree and uses its documented selector.
        let children: Option<Retained<NSArray<AnyObject>>> = unsafe { msg_send![object, accessibilityChildren] };
        children.map_or_else(Vec::new, |children| children.to_vec())
    }

    fn named_button(object: &AnyObject) -> Option<Retained<AnyObject>> {
        for child in children(object) {
            // SAFETY: AX children support the role and title queries.
            let role: Option<Retained<NSString>> = unsafe { msg_send![&*child, accessibilityRole] };
            let title: Option<Retained<NSString>> = unsafe { msg_send![&*child, accessibilityTitle] };
            if role.is_some_and(|role| role.to_string() == "AXButton") && title.is_some_and(|title| title.to_string() == BUTTON_LABEL) {
                return Some(child);
            }
            if let Some(button) = named_button(&child) {
                return Some(button);
            }
        }
        None
    }
}
