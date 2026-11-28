//! Exercises a native panel and AppKit drag-out with OCTARA_TEST_MACOS_DRAG_PANEL=1.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() -> Result<(), String> {
    if std::env::var_os("OCTARA_TEST_MACOS_DRAG_PANEL").is_some() {
        macos::run()?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::rc::Retained;
    use objc2::runtime::NSObjectProtocol;
    use objc2::{ClassType, MainThreadMarker};
    use objc2_app_kit::{
        NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSFloatingWindowLevel, NSNormalWindowLevel, NSPanel, NSPasteboard,
        NSPasteboardNameDrag, NSPasteboardTypeFileURL, NSPasteboardTypeString, NSPasteboardTypeTIFF, NSStatusWindowLevel, NSWindow,
        NSWindowStyleMask,
    };
    use objc2_core_graphics::{CGDisplayIsActive, CGMainDisplayID, CGSessionCopyCurrentDictionary};
    use objc2_foundation::{NSPoint, NSURL};
    use octara::model::input::{Button, DragItem, PointerKind};
    use octara::model::theme::ColorScheme;
    use octara::model::window::WindowLevel;
    use octara::{StatusBarStyle, WindowId, WindowMaterial, WindowOptions, WindowPresentation};
    use octara_host_native::ShellConfig;
    use octara_host_native::os::{LoopState, OsHost, OsWindow, OsWindowContext, PlatformEvent, PlatformSender, PointerPhase, ShellDriver};
    use octara_render::wgpu;
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    const TITLE: &str = "Octara AppKit panel and drag source";
    const DRAG_TEXT: &str = "Octara drag-out smoke text";

    pub(super) fn run() -> Result<(), String> {
        let display = CGMainDisplayID();
        if CGSessionCopyCurrentDictionary().is_none() || display == 0 || !CGDisplayIsActive(display) {
            eprintln!("Skipped AppKit panel and drag-out test because no desktop display is available");
            return Ok(());
        }

        let baseline_active = application().isActive();
        let driver = Test {
            baseline_active,
            deadline: Instant::now() + Duration::from_secs(10),
            sender: None,
            window: None,
            pressed: false,
            dragged: false,
        };
        octara_host_native::os::create_os_host().run(Box::new(driver))?;
        println!("AppKit nonactivating panel, window levels, and drag-out verified");
        Ok(())
    }

    struct Test {
        baseline_active: bool,
        deadline: Instant,
        sender: Option<PlatformSender>,
        window: Option<Box<dyn OsWindow>>,
        pressed: bool,
        dragged: bool,
    }

    impl ShellDriver for Test {
        fn start(&mut self, _: &mut dyn OsHost, sender: PlatformSender) -> Result<(), String> {
            self.sender = Some(sender);
            Ok(())
        }

        fn event(&mut self, os: &mut dyn OsHost, event: PlatformEvent) -> Result<bool, String> {
            match event {
                PlatformEvent::Launched => self.show_panel(os)?,
                PlatformEvent::Pointer { phase: PointerPhase::Started, kind: PointerKind::Mouse, button: Button::Primary, .. } => {
                    self.pressed = true;
                    assert!(!self.window.as_mut().unwrap().start_drag_out(vec![DragItem::Text(DRAG_TEXT.into())]));
                }
                PlatformEvent::Pointer { phase: PointerPhase::Moved, kind: PointerKind::Mouse, button: Button::Primary, .. } => {
                    assert!(self.pressed, "a native press precedes the drag");
                    assert!(!self.dragged, "one synthetic drag was posted");
                    let window = self.window.as_mut().unwrap();
                    assert!(!window.start_drag_out(Vec::new()));
                    assert!(window.start_drag_out(drag_items()), "AppKit did not start a dragging session");
                    assert!(!window.start_drag_out(vec![DragItem::Text(DRAG_TEXT.into())]), "a mouse event can only start one session");
                    verify_pasteboard();
                    self.dragged = true;
                }
                _ => {}
            }
            Ok(false)
        }

        fn idle(&mut self, _: &mut dyn OsHost) -> Result<LoopState, String> {
            if self.dragged {
                assert!(!self.window.as_mut().unwrap().start_drag_out(vec![DragItem::Text(DRAG_TEXT.into())]));
                // AppKit starts tracking on its next run-loop turn. The posted events
                // exercise session creation without taking over the desktop pointer.
                return Ok(LoopState { exit: true, deadline: None });
            }
            if Instant::now() >= self.deadline {
                return Err(format!("AppKit drag-out timed out (pressed {})", self.pressed));
            }
            Ok(LoopState { exit: false, deadline: Some(self.deadline) })
        }

        fn shutdown(&mut self, _: &mut dyn OsHost) -> Result<(), String> {
            if let Some(window) = &mut self.window {
                window.close();
            }
            Ok(())
        }
    }

    impl Test {
        fn show_panel(&mut self, os: &mut dyn OsHost) -> Result<(), String> {
            let options = WindowOptions {
                non_activating_panel: true,
                presentation: WindowPresentation { title: TITLE.into(), level: WindowLevel::Floating, ..Default::default() },
                ..Default::default()
            };
            let mut window = os.create_window(
                WindowId::default(),
                &options,
                OsWindowContext {
                    backend: wgpu::Backends::empty(),
                    config: &ShellConfig::from_env(),
                    sender: self.sender.clone().unwrap(),
                    transparent: false,
                },
            )?;
            window.apply_presentation(TITLE, ColorScheme::Light, WindowMaterial::Opaque, StatusBarStyle::Auto, WindowLevel::Floating);
            window.show();

            let app = application();
            let native = app.windows().iter().find(|window| window.title().to_string() == TITLE).expect("native panel");
            assert!(native.isKindOfClass(NSPanel::class()));
            assert!(native.styleMask().contains(NSWindowStyleMask::NonactivatingPanel));
            assert!(native.isVisible());
            assert!(!native.hidesOnDeactivate());
            assert!(!native.isKeyWindow());
            assert_eq!(app.isActive(), self.baseline_active);
            assert_eq!(native.level(), NSFloatingWindowLevel);
            for (level, expected) in [(WindowLevel::AlwaysOnTop, NSStatusWindowLevel), (WindowLevel::Normal, NSNormalWindowLevel)] {
                window.apply_presentation(TITLE, ColorScheme::Light, WindowMaterial::Opaque, StatusBarStyle::Auto, level);
                assert_eq!(native.level(), expected);
                assert!(native.styleMask().contains(NSWindowStyleMask::NonactivatingPanel));
            }

            post_drag(&app, &native);
            self.window = Some(window);
            Ok(())
        }
    }

    fn application() -> Retained<NSApplication> {
        NSApplication::sharedApplication(MainThreadMarker::new().expect("AppKit smoke runs on the main thread"))
    }

    fn post_drag(app: &NSApplication, window: &NSWindow) {
        for (kind, x, pressure) in
            [(NSEventType::LeftMouseDown, 50.0, 1.0), (NSEventType::LeftMouseDragged, 100.0, 1.0), (NSEventType::LeftMouseUp, 100.0, 0.0)]
        {
            let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                kind,
                NSPoint::new(x, 70.0),
                NSEventModifierFlags::empty(),
                0.0,
                window.windowNumber(),
                None,
                0,
                1,
                pressure,
            )
            .expect("AppKit mouse event");
            app.postEvent_atStart(&event, false);
        }
    }

    fn sample_file() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")
    }

    fn drag_items() -> Vec<DragItem> {
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 3).write_to(&mut png, image::ImageFormat::Png).unwrap();
        vec![
            DragItem::Text(DRAG_TEXT.into()),
            DragItem::File(sample_file()),
            DragItem::Image { media_type: "image/png".into(), bytes: png.into_inner() },
        ]
    }

    fn verify_pasteboard() {
        // SAFETY: AppKit provides these pasteboard names and types for the lifetime of the process.
        let (name, string, file, image) =
            unsafe { (NSPasteboardNameDrag, NSPasteboardTypeString, NSPasteboardTypeFileURL, NSPasteboardTypeTIFF) };
        let pasteboard = NSPasteboard::pasteboardWithName(name);
        let items = pasteboard.pasteboardItems().expect("native drag pasteboard items");
        assert_eq!(items.len(), 3);
        assert_eq!(items.objectAtIndex(0).stringForType(string).unwrap().to_string(), DRAG_TEXT);
        let url = NSURL::URLWithString(&items.objectAtIndex(1).stringForType(file).unwrap()).unwrap();
        assert!(url.isFileURL());
        assert_eq!(url.path().unwrap().to_string(), sample_file().to_str().unwrap());
        assert!(items.objectAtIndex(2).dataForType(image).is_some_and(|data| data.length() > 0));
    }
}
