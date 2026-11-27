use super::{driver, observe, options};
use octara::{
    module::{
        CallTicket, HostSurface, HostSurfaceKind, ModuleCallCx, ModuleDispatcher, ModuleError, ModuleId, ModuleInstallCx,
        ModuleObservationKind, NativeViewEmitter, NativeViewFactory, NativeViewInstance, OctaraModule,
    },
    prelude::*,
};
use octara_inspect::{
    Inspector,
    replay::{Event, Replay},
};
use octara_model::{
    identity::NativeId,
    native::{NativePlacement, NativeSnapshotRequest, TypedNativeProps},
};
use serde_json::json;
use std::{any::Any, cell::RefCell, rc::Rc, time::Duration};

const KIND: &str = "test.gauge";

#[derive(Clone)]
struct GaugeProps(u32);

struct Add(u32);
struct GaugeChanged(u32);

#[derive(Default)]
struct GaugeState {
    emitter: Option<NativeViewEmitter>,
    placements: Vec<NativePlacement>,
    updates: Vec<u32>,
    measured: bool,
    destroyed: bool,
}

struct GaugeModule(Rc<RefCell<GaugeState>>);

impl OctaraModule for GaugeModule {
    fn id(&self) -> ModuleId {
        ModuleId(KIND)
    }

    fn install(&mut self, cx: &mut ModuleInstallCx<'_>) -> Result<(), ModuleError> {
        cx.native_views().register(GaugeFactory(self.0.clone()))
    }

    fn call(&mut self, _: CallTicket, _: Box<dyn Any>, _: &mut ModuleCallCx<'_>) {
        unreachable!("gauge commands arrive at the mounted view");
    }
}

struct GaugeFactory(Rc<RefCell<GaugeState>>);

impl NativeViewFactory for GaugeFactory {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn supports_slot(&self, surface: HostSurfaceKind, slot: NativeSlot) -> bool {
        surface == HostSurfaceKind::Headless && slot == NativeSlot::Underlay
    }

    fn create(
        &mut self,
        surface: HostSurface<'_>,
        _: NativeId,
        _: NativeSlot,
        props: Box<dyn Any>,
        emitter: NativeViewEmitter,
    ) -> Result<Box<dyn NativeViewInstance>, ModuleError> {
        assert_eq!(surface.kind(), HostSurfaceKind::Headless);
        assert!(surface.window().is_some());
        let value = props.downcast::<GaugeProps>().unwrap().0;
        self.0.borrow_mut().emitter = Some(emitter.clone());
        emitter.emit("mounted", GaugeChanged(value), None)?;
        if value == 0 {
            return Err(ModuleError::Unavailable);
        }
        Ok(Box::new(GaugeView { value, emitter, state: self.0.clone() }))
    }
}

struct GaugeView {
    value: u32,
    emitter: NativeViewEmitter,
    state: Rc<RefCell<GaugeState>>,
}

impl NativeViewInstance for GaugeView {
    fn update(&mut self, props: Box<dyn Any>) {
        self.value = props.downcast::<GaugeProps>().unwrap().0;
        self.state.borrow_mut().updates.push(self.value);
    }

    fn place(&mut self, placement: &NativePlacement) {
        self.state.borrow_mut().placements.push(placement.clone());
    }

    fn call(&mut self, _: CallTicket, command: Box<dyn Any>, _: ModuleDispatcher) {
        self.value += command.downcast::<Add>().unwrap().0;
        self.emitter.emit("changed", GaugeChanged(self.value), Some(json!(self.value))).unwrap();
    }

    fn measure(&mut self, _: WindowViewport) -> Option<Size> {
        self.state.borrow_mut().measured = true;
        Some(Size::new(75.0, 35.0))
    }

    fn snapshot(&mut self, _: NativeSnapshotRequest, _: ModuleDispatcher) {}
}

impl Drop for GaugeView {
    fn drop(&mut self) {
        self.state.borrow_mut().destroyed = true;
    }
}

#[test]
fn registered_view_mounts_updates_calls_and_emits_on_the_virtual_clock() {
    let recording = Rc::new(RefCell::new(Replay::new([11, 29])));
    let mut driver = driver().with_inspector(Inspector::default().with_trace_recorder(recording.clone()));
    let state = Rc::new(RefCell::new(GaugeState::default()));
    let (value, events) = driver.app_mut().build(|cx| (cx.signal(5_u32), cx.signal(Vec::new()))).unwrap();
    let definition = AppDefinition::new().with_module(GaugeModule(state.clone())).window(WindowDefinition::new(options(), move |_| {
        typed_native_view(KIND, move |read: &mut ReadCx<'_>| {
            let value = read.get(value);
            TypedNativeProps::new(GaugeProps(value), Some(json!({ "value": value })))
        })
        .native_slot(NativeSlot::Underlay)
        .test_id("gauge")
        .w(80)
        .h(40)
        .on_typed_native(move |write, name, GaugeChanged(value)| {
            let mut log = write.get(events);
            log.push((name, *value));
            write.set(events, log);
        })
    }));
    let [window] = driver.launch(definition).unwrap()[..] else { panic!("expected one headless window") };
    assert_eq!(state.borrow().placements.last().unwrap().rect.w, 80.0);
    assert!(state.borrow().measured);
    let log = |driver: &mut octara_inspect::HeadlessDriver| driver.app_mut().update(|write| write.get(events)).unwrap();
    assert_eq!(log(&mut driver), [("mounted", 5)]);

    driver.app_mut().update(|write| write.set(value, 12)).unwrap();
    let snapshot = observe(&mut driver, window);
    let gauge = snapshot.nodes.iter().find(|node| node.test_id.as_deref() == Some("gauge")).unwrap();
    assert_eq!(gauge.native_props, Some(json!({ "value": 12 })));
    assert_eq!(state.borrow().updates, [12]);
    let queued = driver
        .app_mut()
        .update_window(window, |write| write.send_typed_native_command("gauge", Add(7), Some(json!({ "amount": 7 }))))
        .unwrap();
    assert!(queued);
    driver.advance_to(Duration::from_millis(5)).unwrap();
    assert_eq!(log(&mut driver), [("mounted", 5), ("changed", 19)]);

    let emitter = state.borrow().emitter.clone().unwrap();
    driver.app_mut().close_window(window).unwrap();
    driver.sync_native_views().unwrap();
    assert!(state.borrow().destroyed);
    assert_eq!(emitter.emit("late", GaugeChanged(0), None), Err(ModuleError::Cancelled));
    driver.advance_to(Duration::from_millis(6)).unwrap();
    assert_eq!(log(&mut driver), [("mounted", 5), ("changed", 19)]);

    let recording = recording.borrow();
    let observed: Vec<_> = recording
        .entries()
        .iter()
        .filter_map(|entry| match &entry.event {
            Event::Module(event) if event.module == KIND => Some((event.kind, event.payload.as_ref())),
            _ => None,
        })
        .collect();
    assert_eq!(
        observed,
        [
            (ModuleObservationKind::NativeEvent, None),
            (ModuleObservationKind::NativeCommand, Some(&json!({ "amount": 7 }))),
            (ModuleObservationKind::NativeEvent, Some(&json!(19))),
        ]
    );
}

#[test]
fn callbacks_from_a_failed_mount_are_discarded() {
    let mut driver = driver();
    let state = Rc::new(RefCell::new(GaugeState::default()));
    let seen = driver.app_mut().build(|cx| cx.signal(false)).unwrap();
    let definition = AppDefinition::new().with_module(GaugeModule(state)).window(WindowDefinition::new(options(), move |_| {
        typed_native_view(KIND, TypedNativeProps::new(GaugeProps(0), None))
            .native_slot(NativeSlot::Underlay)
            .on_typed_native(move |write, _, _: &GaugeChanged| write.set(seen, true))
    }));
    driver.launch(definition).unwrap();
    driver.advance_to(Duration::from_millis(1)).unwrap();
    assert!(!driver.app_mut().update(|write| write.get(seen)).unwrap());
}
