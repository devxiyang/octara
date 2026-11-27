//! Exercises the real AppKit window and Metal-backed view from the main thread.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() -> Result<(), octara_host_native::ShellError> {
    macos::run()
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSAppearanceCustomization, NSAppearanceNameDarkAqua, NSApplication, NSScreen, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
        NSVisualEffectView, NSWindow, NSWindowButton, NSWindowStyleMask, NSWindowTitleVisibility,
    };
    use objc2_core_graphics::{CGDisplayIsActive, CGMainDisplayID, CGSessionCopyCurrentDictionary};
    use objc2_foundation::NSPoint;
    use objc2_quartz_core::CAMetalLayer;
    use octara::model::capture::FrameReceipt;
    use octara::prelude::*;
    use octara_host_native::{ShellError, ShellHook, run_app_with_hook};
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    const CUSTOM: &str = "Octara AppKit chrome test";
    const NEW_TITLE: &str = "Octara AppKit updated title";
    const STANDARD: &str = "Octara AppKit geometry test";
    const OFFSET: Point = Point::new(24.0, 7.0);
    const POSITION: Point = Point::new(100.0, 100.0);
    const SIZE: Size = Size::new(440.0, 310.0);
    const LIMITS: [Size; 2] = [Size::new(340.0, 260.0), Size::new(540.0, 410.0)];
    const STANDARD_POSITION: Point = Point::new(175.0, 210.0);
    const STANDARD_SIZE: Size = Size::new(600.0, 450.0);
    type Metrics = (ReadSignal<WindowGeometry>, ReadSignal<WindowChrome>);

    pub(super) fn run() -> Result<(), ShellError> {
        let display = CGMainDisplayID();
        if CGSessionCopyCurrentDictionary().is_none() || display == 0 || !CGDisplayIsActive(display) {
            eprintln!("Skipped AppKit chrome and geometry test because no desktop display is available");
            return Ok(());
        }
        let metrics = Rc::new(Cell::new(None));
        let windows = AppDefinition::new()
            .window(WindowDefinition::new(
                WindowOptions {
                    presentation: WindowPresentation {
                        title: CUSTOM.into(),
                        material: WindowMaterial::Vibrancy(VibrancyMaterial::Sidebar),
                        ..Default::default()
                    },
                    titlebar: TitlebarStyle::HiddenInset { traffic_light_offset: Some(OFFSET) },
                    min_size: Some(LIMITS[0]),
                    max_size: Some(LIMITS[1]),
                    viewport: WindowViewport { size: SIZE, scale: 1.0, safe_area: None, keyboard_inset: 0.0 },
                    position: Some(POSITION),
                    ..Default::default()
                },
                {
                    let metrics = metrics.clone();
                    move |build| {
                        metrics.set(Some((build.window_geometry(), build.window_chrome())));
                        row().window_drag_region(true).h(32).child(text("Title"))
                    }
                },
            ))
            .window(WindowDefinition::new(
                WindowOptions {
                    presentation: WindowPresentation { title: STANDARD.into(), ..Default::default() },
                    maximized: true,
                    position: Some(STANDARD_POSITION),
                    viewport: WindowViewport { size: STANDARD_SIZE, scale: 1.0, safe_area: None, keyboard_inset: 0.0 },
                    ..Default::default()
                },
                |_| text("Observer"),
            ));
        run_app_with_hook(windows, Test { started: Instant::now(), phase: Phase::FirstFrame, metrics, effect: None })?;
        println!("AppKit chrome, vibrancy, and live multi-window geometry verified");
        Ok(())
    }

    #[derive(Debug)]
    enum Phase {
        FirstFrame,
        MoveResize,
        Opaque,
        StartupUnzoom,
        Zoom,
        Unzoom,
        EnterFullscreen,
        ExitFullscreen,
    }

    struct Test {
        started: Instant,
        phase: Phase,
        metrics: Rc<Cell<Option<Metrics>>>,
        effect: Option<Retained<NSVisualEffectView>>,
    }

    impl ShellHook for Test {
        fn tick(&mut self, app: &mut App, presented: &dyn Fn(WindowId) -> Option<FrameReceipt>) -> Result<Option<Duration>, ShellError> {
            assert!(self.started.elapsed() < Duration::from_secs(20), "stalled in {:?}", self.phase);
            let Some(custom) =
                app.runtime().windows().find(|(_, w)| [CUSTOM, NEW_TITLE].contains(&w.presentation().title.as_ref())).map(|(&id, _)| id)
            else {
                return Ok(Some(Duration::from_millis(16)));
            };
            let standard = app.runtime().windows().find(|(_, w)| w.presentation().title.as_ref() == STANDARD).map(|(&id, _)| id).unwrap();
            let (Some(native), Some(other)) = (appkit_window(CUSTOM).or_else(|| appkit_window(NEW_TITLE)), appkit_window(STANDARD)) else {
                return Ok(Some(Duration::from_millis(16)));
            };
            let metrics = self.metrics.get().expect("author observed AppKit metrics");
            let (geometry, chrome) = app.update_window(custom, |write| (write.get(metrics.0), write.get(metrics.1))).unwrap();
            let standard_geometry = app.runtime().window(standard).unwrap().window_geometry();
            match self.phase {
                Phase::FirstFrame if [custom, standard].into_iter().all(|id| presented(id).is_some_and(|frame| frame.gpu_complete)) => {
                    verify_chrome(&native, chrome);
                    self.effect = Some(verify_vibrancy(&native));
                    assert!(other.isOpaque());
                    assert!(standard_geometry.maximized && other.isZoomed());
                    let list = app.runtime().window(custom).unwrap().last_commit().unwrap().draw.as_ref().unwrap();
                    assert_eq!(list.background.components[3], 0.0);
                    verify_geometry(&native, geometry);
                    assert_eq!(geometry.position, Some(POSITION));
                    assert_eq!(geometry.size, SIZE);
                    assert_eq!(native.contentMinSize().width, f64::from(LIMITS[0].width));
                    assert_eq!(native.contentMaxSize().height, f64::from(LIMITS[1].height));
                    app.update_window(custom, |write| {
                        write.set_window_title(NEW_TITLE);
                        write.set_window_theme(Theme::dark());
                        write.set_window_material(WindowMaterial::Vibrancy(VibrancyMaterial::Hud));
                        write.set_window_geometry(WindowGeometryRequest {
                            position: Some(Point::new(180.0, 140.0)),
                            size: Some(Size::new(460.0, 330.0)),
                            ..Default::default()
                        });
                        write.focus_window(custom);
                    })
                    .unwrap();
                    self.phase = Phase::MoveResize;
                }
                Phase::MoveResize
                    if native.title().to_string() == NEW_TITLE && geometry.size == Size::new(460.0, 330.0) && native.isKeyWindow() =>
                {
                    verify_chrome(&native, chrome);
                    verify_geometry(&native, geometry);
                    assert_eq!(geometry.position, Some(Point::new(180.0, 140.0)));
                    let effect = visual_effect(&native).expect("live window visual effect");
                    assert!(std::ptr::eq(&*effect, self.effect.as_deref().unwrap()));
                    assert_eq!(effect.material(), NSVisualEffectMaterial::HUDWindow);
                    // SAFETY: This is AppKit's static name for Dark Aqua.
                    assert_eq!(&*native.effectiveAppearance().name(), unsafe { NSAppearanceNameDarkAqua });
                    app.update_window(custom, |write| write.set_window_material(WindowMaterial::Opaque)).unwrap();
                    self.phase = Phase::Opaque;
                }
                Phase::Opaque if native.isOpaque() => {
                    assert!(visual_effect(&native).is_none());
                    let runtime = app.runtime().window(custom).unwrap();
                    assert_eq!(
                        runtime.last_commit().unwrap().draw.as_ref().unwrap().background,
                        runtime.tree().theme().get(ColorToken::Background),
                    );
                    app.update_window(standard, |write| write.toggle_maximize_window()).unwrap();
                    self.phase = Phase::StartupUnzoom;
                }
                Phase::StartupUnzoom if !standard_geometry.maximized => {
                    verify_geometry(&other, standard_geometry);
                    assert_eq!(standard_geometry.position, Some(STANDARD_POSITION));
                    assert_eq!(standard_geometry.size, STANDARD_SIZE);
                    app.update_window(standard, |write| write.toggle_maximize_window()).unwrap();
                    self.phase = Phase::Zoom;
                }
                Phase::Zoom if standard_geometry.maximized => {
                    assert!(other.isZoomed());
                    verify_geometry(&other, standard_geometry);
                    app.update_window(standard, |write| write.toggle_maximize_window()).unwrap();
                    self.phase = Phase::Unzoom;
                }
                Phase::Unzoom if !standard_geometry.maximized => {
                    app.update_window(standard, |write| {
                        write.set_window_geometry(WindowGeometryRequest { fullscreen: Some(true), ..Default::default() })
                    })
                    .unwrap();
                    self.phase = Phase::EnterFullscreen;
                }
                Phase::EnterFullscreen if standard_geometry.fullscreen => {
                    assert!(other.styleMask().contains(NSWindowStyleMask::FullScreen));
                    app.update_window(standard, |write| {
                        write.set_window_geometry(WindowGeometryRequest {
                            fullscreen: Some(false),
                            position: Some(Point::new(250.0, 160.0)),
                            size: Some(Size::new(470.0, 320.0)),
                            ..Default::default()
                        });
                        write.set_window_geometry(WindowGeometryRequest { size: Some(Size::new(490.0, 350.0)), ..Default::default() });
                    })
                    .unwrap();
                    self.phase = Phase::ExitFullscreen;
                }
                Phase::ExitFullscreen if !standard_geometry.fullscreen => {
                    assert!(!other.styleMask().contains(NSWindowStyleMask::FullScreen));
                    assert_eq!(standard_geometry.position, Some(Point::new(250.0, 160.0)));
                    assert_eq!(standard_geometry.size, Size::new(490.0, 350.0));
                    app.close_window(custom).unwrap();
                    app.close_window(standard).unwrap();
                    return Ok(None);
                }
                _ => {}
            }
            Ok(Some(Duration::from_millis(16)))
        }
    }

    fn appkit_window(title: &str) -> Option<Retained<NSWindow>> {
        let mtm = MainThreadMarker::new().expect("native harness must run on its main thread");
        NSApplication::sharedApplication(mtm).windows().iter().find(|window| window.title().to_string() == title)
    }

    fn verify_chrome(window: &NSWindow, chrome: WindowChrome) {
        assert!(window.styleMask().contains(NSWindowStyleMask::FullSizeContentView));
        assert_eq!(window.titleVisibility(), NSWindowTitleVisibility::Hidden);
        assert!(window.titlebarAppearsTransparent());
        let content = window.contentView().unwrap();
        for kind in [NSWindowButton::CloseButton, NSWindowButton::MiniaturizeButton, NSWindowButton::ZoomButton] {
            let button = window.standardWindowButton(kind).expect("system traffic light");
            // SAFETY: These are live AppKit controls on their owning thread.
            let parent = unsafe { button.superview() }.unwrap();
            let frame = content.convertRect_fromView(button.frame(), Some(&parent));
            let x = frame.origin.x as f32;
            let y = if content.isFlipped() { frame.origin.y } else { content.bounds().size.height - frame.max().y } as f32;
            let traffic_lights = chrome.traffic_light_rect.expect("AppKit advertised traffic lights");
            assert!(traffic_lights.contains(Point::new(x + 1.0, y + 1.0)));
            if kind == NSWindowButton::CloseButton {
                assert!((x - OFFSET.x).abs() < 1.0 && (y - OFFSET.y).abs() < 1.0, "traffic light origin {x}, {y}");
            }
            assert!(chrome.titlebar_height >= traffic_lights.bottom());
            assert_eq!(chrome.leading_inset, traffic_lights.right());
            assert_eq!(chrome.trailing_inset, 0.0);
        }
    }

    fn visual_effect(window: &NSWindow) -> Option<Retained<NSVisualEffectView>> {
        window.contentView()?.subviews().iter().find_map(|view| view.downcast::<NSVisualEffectView>().ok())
    }

    fn verify_vibrancy(window: &NSWindow) -> Retained<NSVisualEffectView> {
        assert!(!window.isOpaque());
        let content = window.contentView().unwrap();
        let effect = visual_effect(window).expect("window visual effect");
        assert_eq!(effect.frame(), content.bounds());
        assert_eq!(effect.material(), NSVisualEffectMaterial::Sidebar);
        assert_eq!(effect.blendingMode(), NSVisualEffectBlendingMode::BehindWindow);
        assert!(effect.hitTest(NSPoint::new(35.0, 35.0)).is_none());
        let backing_layer = effect.layer().unwrap();
        let root = content.layer().unwrap();
        let metal =
            unsafe { root.sublayers() }.unwrap().iter().find_map(|layer| layer.downcast::<CAMetalLayer>().ok()).expect("Metal layer");
        assert!(metal.zPosition() > backing_layer.zPosition());
        assert!(!metal.isOpaque());
        effect
    }

    fn verify_geometry(window: &NSWindow, geometry: WindowGeometry) {
        let mtm = MainThreadMarker::new().unwrap();
        let frame = window.frame();
        let content = window.contentView().unwrap().bounds();
        let screen = NSScreen::screens(mtm).firstObject().unwrap();
        let origin = Point::new(frame.origin.x as f32, (screen.frame().max().y - frame.max().y) as f32);
        let position = geometry.position.expect("AppKit window has screen coordinates");
        assert!((position.x - origin.x).abs() < 1.0 && (position.y - origin.y).abs() < 1.0, "{geometry:?} versus frame {frame:?}");
        assert_eq!(geometry.size, Size::new(content.size.width as f32, content.size.height as f32));
        assert_eq!(geometry.scale, window.backingScaleFactor() as f32);
    }
}
