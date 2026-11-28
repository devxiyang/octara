//! Browser regression for the Web module host, using web_modules.html after wasm-bindgen.

fn main() {}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod browser {
    use js_sys::{Function, Promise};
    use octara::module::{
        AirspacePolicy, CallTicket, HostLifecycleEvent, HostSurface, HostSurfaceKind, ModuleCallCx, ModuleDispatcher, ModuleError,
        ModuleId, ModuleInstallCx, ModuleLifecycleCx, NativeViewEmitter, NativeViewFactory, NativeViewInstance, OctaraModule,
        TypedNativeProps,
    };
    use octara::prelude::*;
    use octara_host_web::WebApp;
    use octara_model::identity::NativeId;
    use octara_model::native::{NativePlacement, NativeSlot, NativeSnapshotRequest};
    use std::any::Any;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{HtmlElement, HtmlInputElement, PopStateEvent};

    const KIND: &str = "test.browser-module";

    #[derive(Default)]
    struct Observed {
        lifecycle: RefCell<Vec<HostLifecycleEvent>>,
        dropped: Cell<u32>,
        shutdown: Cell<bool>,
    }

    struct TestModule(Rc<Observed>);

    impl OctaraModule for TestModule {
        fn id(&self) -> ModuleId {
            ModuleId(KIND)
        }

        fn install(&mut self, cx: &mut ModuleInstallCx<'_>) -> Result<(), ModuleError> {
            assert_eq!(cx.host_kind(), HostSurfaceKind::Web);
            cx.native_views().register(Factory(self.0.clone()))
        }

        fn call(&mut self, _: CallTicket, _: Box<dyn Any>, _: &mut ModuleCallCx<'_>) {
            unreachable!()
        }

        fn on_lifecycle(&mut self, event: &HostLifecycleEvent, cx: &mut ModuleLifecycleCx<'_>) {
            assert_eq!(cx.host_kind(), HostSurfaceKind::Web);
            self.0.lifecycle.borrow_mut().push(event.clone());
        }

        fn shutdown(&mut self) {
            assert!(!self.0.shutdown.replace(true));
        }
    }

    struct Factory(Rc<Observed>);

    impl NativeViewFactory for Factory {
        fn kind(&self) -> &'static str {
            KIND
        }

        fn airspace_policy(&self) -> AirspacePolicy {
            AirspacePolicy::SnapshotOnOcclude
        }

        fn create(
            &mut self,
            surface: HostSurface<'_>,
            _: NativeId,
            slot: NativeSlot,
            props: Box<dyn Any>,
            emitter: NativeViewEmitter,
        ) -> Result<Box<dyn NativeViewInstance>, ModuleError> {
            assert_eq!(surface.kind(), HostSurfaceKind::Web);
            assert!(surface.window().is_some());
            assert!(surface.raw_native().is_none());
            assert_eq!(slot, NativeSlot::Overlay);
            let host = surface.downcast_ref::<HtmlElement>().expect("DOM slot");
            assert!(host.is_connected());
            assert_eq!(host.get_attribute("data-octara-id").as_deref(), Some("typed-slot"));
            let element = host.owner_document().unwrap().create_element("input").unwrap().dyn_into::<HtmlInputElement>().unwrap();
            element.set_type("checkbox");
            element.set_attribute("style", "position:absolute;left:12px;top:12px;margin:0").unwrap();
            host.append_child(&element).unwrap();
            emitter.emit("value", 7_u32, None)?;
            let mut view = Instance { element, emitter, observed: self.0.clone() };
            view.update(props);
            Ok(Box::new(view))
        }
    }

    struct Instance {
        element: HtmlInputElement,
        emitter: NativeViewEmitter,
        observed: Rc<Observed>,
    }

    impl NativeViewInstance for Instance {
        fn update(&mut self, props: Box<dyn Any>) {
            self.element.set_attribute("data-props", &props.downcast::<String>().unwrap()).unwrap();
        }

        fn place(&mut self, placement: &NativePlacement) {
            assert!(placement.visible);
            self.element.set_attribute("data-width", &placement.rect.w.to_string()).unwrap();
        }

        fn call(&mut self, _: CallTicket, command: Box<dyn Any>, _: ModuleDispatcher) {
            self.emitter.emit("value", *command.downcast::<u32>().unwrap(), None).unwrap();
        }

        fn snapshot(&mut self, _: NativeSnapshotRequest, _: ModuleDispatcher) {
            panic!("Browser overlays use the DOM paint order");
        }

        fn set_occluded_by_snapshot(&mut self, _: bool) {
            panic!("The browser keeps this DOM element mounted and visible");
        }
    }

    impl Drop for Instance {
        fn drop(&mut self) {
            self.element.remove();
            self.observed.dropped.set(self.observed.dropped.get() + 1);
        }
    }

    #[wasm_bindgen]
    pub async fn run_web_modules(page_transition: &Function) -> Result<(), JsValue> {
        use HostLifecycleEvent::*;

        let browser = web_sys::window().unwrap();
        let document = browser.document().unwrap();
        let body = document.body().unwrap();
        std::panic::set_hook(Box::new(move |panic| {
            body.set_attribute("data-panic", &panic.to_string()).unwrap();
        }));
        let mount = document.create_element("div")?.dyn_into::<HtmlElement>()?;
        mount.set_attribute("style", "width:300px;height:200px")?;
        document.body().unwrap().append_child(&mount)?;
        let observed = Rc::new(Observed::default());
        let handles = Rc::new(Cell::new(None));
        let listener = Rc::new(RefCell::new(None::<Function>));
        let next_event = || {
            Promise::new(&mut |resolve, _| {
                listener.replace(Some(resolve));
            })
        };
        let created = next_event();
        let definition =
            AppDefinition::new().with_module(TestModule(observed.clone())).window(WindowDefinition::new(WindowOptions::default(), {
                let handles = handles.clone();
                let listener = listener.clone();
                move |b| {
                    let props = b.signal("first".to_owned());
                    let shown = b.signal(true);
                    let covered = b.signal(false);
                    handles.set(Some((props, shown, covered)));
                    col()
                        .child(show(
                            move |r| r.get(shown),
                            move |_| {
                                let listener = listener.clone();
                                typed_native_view(KIND, move |r: &mut ReadCx<'_>| TypedNativeProps::new(r.get(props), None))
                                    .w(100)
                                    .h(40)
                                    .test_id("typed-slot")
                                    .on_typed_native::<u32>(move |_, name, &value| {
                                        assert_eq!(name, "value");
                                        let resolve = listener.take().expect("waiting for a native event");
                                        resolve.call1(&JsValue::UNDEFINED, &JsValue::from(value)).unwrap();
                                    })
                            },
                        ))
                        .portal_part(
                            move |r| r.get(covered),
                            |_| el().position(Position::Absolute).top(0).left(0).w(40).h(32).bg(rgb(0x3264a8)).test_id("cover"),
                        )
                }
            }));
        let app = WebApp::launch(mount.clone(), definition, None)?;
        let window = app.window_id();
        let url = browser.location().href()?;
        assert_eq!(
            *observed.lifecycle.borrow(),
            [AppLaunched { urls: vec![url.clone()], files: Vec::new() }, WindowAttached { window }, AppForeground]
        );
        let input = mount.query_selector("[data-octara-id='typed-slot'] input")?.unwrap().dyn_into::<HtmlInputElement>()?;
        assert_eq!(input.get_attribute("data-props").as_deref(), Some("first"));
        assert_eq!(input.get_attribute("data-width").as_deref(), Some("100"));
        assert_eq!(JsFuture::from(created).await?.as_f64(), Some(7.0));

        let (props, shown, covered) = handles.get().unwrap();
        input.focus()?;
        input.click();
        assert!(input.checked());
        let rect = input.get_bounding_client_rect();
        let hit = || document.element_from_point((rect.x() + 2.0) as f32, (rect.y() + 2.0) as f32).unwrap();
        assert!(hit().is_same_node(Some(&input)));
        app.update(|w| w.set(covered, true)).unwrap();
        assert_eq!(hit().get_attribute("data-octara-id").as_deref(), Some("cover"));
        assert!(input.is_connected() && input.checked());
        app.update(|w| w.set(covered, false)).unwrap();
        assert!(hit().is_same_node(Some(&input)));
        app.update(|w| w.set(props, "second".to_owned())).unwrap();
        assert_eq!(input.get_attribute("data-props").as_deref(), Some("second"));
        assert_eq!(document.active_element().as_ref(), Some(input.as_ref()));

        // Let ResizeObserver finish any frames scheduled by the initial mount.
        for _ in 0..2 {
            let frame = Promise::new(&mut |resolve, reject| {
                if let Err(error) = browser.request_animation_frame(&resolve) {
                    reject.call1(&JsValue::UNDEFINED, &error).unwrap();
                }
            });
            JsFuture::from(frame).await?;
        }
        let reply = next_event();
        assert!(app.update(|w| w.send_typed_native_command("typed-slot", 42_u32, None)).unwrap());
        assert_eq!(JsFuture::from(reply).await?.as_f64(), Some(42.0));
        app.update(|w| w.set(shown, false)).unwrap();
        assert!(!input.is_connected());
        assert_eq!(observed.dropped.get(), 1);

        let transition = |name: &str, persisted: bool| page_transition.call2(&JsValue::UNDEFINED, &name.into(), &persisted.into());
        transition("pagehide", true)?;
        transition("pagehide", true)?;
        assert!(app.with_app(|_| ()).is_some());
        transition("pageshow", true)?;
        browser.dispatch_event(PopStateEvent::new("popstate")?.as_ref())?;
        transition("pagehide", false)?;
        assert!(app.with_app(|_| ()).is_none());
        drop(app);
        assert_eq!(
            observed.lifecycle.borrow()[3..],
            [AppBackground, AppForeground, OpenUrls(vec![url]), AppBackground, WindowDetached { window }, WillTerminate]
        );
        assert!(observed.shutdown.get());
        mount.remove();
        Ok(())
    }
}
