use super::{driver, options, target};
use octara::module::{
    AirspacePolicy, CallTicket, HostSurface, ModuleCallCx, ModuleDispatcher, ModuleError, ModuleId, ModuleInstallCx, NativeId,
    NativePlacement, NativeSlot, NativeSnapshot, NativeSnapshotRequest, NativeViewEmitter, NativeViewFactory, NativeViewInstance,
    OctaraModule, TypedNativeProps,
};
use octara::prelude::*;
use octara::{NodeId, Picture};
use octara_model::paint::{DisplayList, Item};
use octara_render::CpuRenderer;
use std::{any::Any, cell::RefCell, rc::Rc};

const KIND: &str = "test.airspace";
const RED: [u8; 4] = [212, 32, 48, 255];

#[derive(Default)]
struct Probe {
    captures: Vec<NativeSnapshotRequest>,
    visibility: Vec<bool>,
}

#[derive(Clone)]
struct Factory(Rc<RefCell<Probe>>);

impl OctaraModule for Factory {
    fn id(&self) -> ModuleId {
        ModuleId(KIND)
    }

    fn install(&mut self, cx: &mut ModuleInstallCx<'_>) -> Result<(), ModuleError> {
        cx.native_views().register(self.clone())
    }

    fn call(&mut self, _: CallTicket, _: Box<dyn Any>, _: &mut ModuleCallCx<'_>) {}
}

impl NativeViewFactory for Factory {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn airspace_policy(&self) -> AirspacePolicy {
        AirspacePolicy::SnapshotOnOcclude
    }

    fn create(
        &mut self,
        _: HostSurface<'_>,
        _: NativeId,
        _: NativeSlot,
        _: Box<dyn Any>,
        _: NativeViewEmitter,
    ) -> Result<Box<dyn NativeViewInstance>, ModuleError> {
        Ok(Box::new(self.clone()))
    }
}

impl NativeViewInstance for Factory {
    fn update(&mut self, _: Box<dyn Any>) {}
    fn place(&mut self, _: &NativePlacement) {}
    fn call(&mut self, _: CallTicket, _: Box<dyn Any>, _: ModuleDispatcher) {}

    fn snapshot(&mut self, request: NativeSnapshotRequest, _: ModuleDispatcher) {
        self.0.borrow_mut().captures.push(request);
    }

    fn set_occluded_by_snapshot(&mut self, hidden: bool) {
        self.0.borrow_mut().visibility.push(hidden);
    }
}

#[test]
fn freezes_the_mounted_overlay_and_ignores_async_results_from_old_occlusions() {
    let mut driver = driver();
    let probe = Rc::new(RefCell::new(Probe::default()));
    let (cover, props) = driver.app_mut().build(|b| (b.signal(0), b.signal(0))).unwrap();
    let app = AppDefinition::new().with_module(Factory(probe.clone())).window(WindowDefinition::new(options(), move |_| {
        el().size(rel(1.0))
            .child(
                col()
                    .w(180)
                    .h(140)
                    .scroll_y()
                    .test_id("scroll")
                    .child(el().h(20).shrink(0.0))
                    .child(
                        typed_native_view(KIND, move |r: &mut ReadCx<'_>| TypedNativeProps::new(r.get(props), None))
                            .native_slot(NativeSlot::Overlay)
                            .test_id("live")
                            .w(120)
                            .h(110)
                            .shrink(0.0),
                    )
                    .child(el().h(180).shrink(0.0)),
            )
            .child(
                typed_native_view(KIND, TypedNativeProps::new((), None))
                    .native_slot(NativeSlot::Underlay)
                    .position(Position::Absolute)
                    .left(90)
                    .top(50)
                    .size(30),
            )
            .child(
                typed_native_view("test.unmounted", TypedNativeProps::new((), None))
                    .native_slot(NativeSlot::Overlay)
                    .position(Position::Absolute)
                    .left(100)
                    .top(55)
                    .size(20),
            )
            .portal_part(
                move |r| r.get(cover) != 0,
                move |_| {
                    el().position(Position::Absolute)
                        .top(40)
                        .w(70)
                        .h(50)
                        .bg(rgb(0x22c040))
                        .style_with(move |r, style| style.left(if r.get(cover) == 1 { 185 } else { 50 }))
                },
            )
    }));
    let [window] = driver.launch(app).unwrap().try_into().unwrap();
    let live = target(&driver, window, "live").node().unwrap();
    let scroll = target(&driver, window, "scroll").node().unwrap();
    let cover_at = |driver: &mut super::HeadlessDriver, value| {
        driver.app_mut().update(|w| w.set(cover, value)).unwrap();
        driver.sync_native_views().unwrap();
    };
    cover_at(&mut driver, 1);
    assert!(probe.borrow().captures.is_empty());
    cover_at(&mut driver, 2);
    let first = probe.borrow().captures[0];
    assert_eq!(first.id, NativeId::from(live));
    cover_at(&mut driver, 0);
    cover_at(&mut driver, 2);
    let second = probe.borrow().captures[1];
    assert_ne!(first, second);

    let reply = driver.app().runtime().module_dispatcher();
    let complete = |request| {
        let picture = Picture::new(1, 1, RED.to_vec()).unwrap();
        reply.resolve_native_snapshot(request, NativeSnapshot { picture, misses: Vec::new() }).unwrap();
    };
    complete(first);
    driver.advance_to(std::time::Duration::from_millis(1)).unwrap();
    assert!(probe.borrow().visibility.is_empty());
    assert!(snapshot(draw(&driver, window), live).is_none());

    // A change to the view's properties while capture is pending invalidates the old result.
    complete(second);
    driver.app_mut().update(|w| w.set(props, 1)).unwrap();
    driver.sync_native_views().unwrap();
    let third = probe.borrow().captures[2];
    assert!(probe.borrow().visibility.is_empty());
    complete(third);
    driver.advance_to(std::time::Duration::from_millis(2)).unwrap();
    assert_eq!(probe.borrow().visibility, [true]);
    let display = draw(&driver, window);
    let before = snapshot(display, live).unwrap();
    let spatial_id = before.spatial_id;
    assert_ne!(spatial_id, 0);
    let frozen = before.item.clone();
    let mut renderer = CpuRenderer::new();
    let picture = renderer.paint(display, 1.0).unwrap();
    assert_eq!(picture.pixel(10, 35), Some(RED));
    assert_eq!(picture.pixel(65, 45), Some([34, 192, 64, 255]));

    driver.app_mut().update_window(window, |w| w.scroll_to(scroll, Vec2::new(0.0, 10.0))).unwrap();
    driver.sync_native_views().unwrap();
    let after = snapshot(draw(&driver, window), live).unwrap();
    assert_eq!(after.item, &frozen);
    assert_eq!(after.spatial_id, spatial_id);
    assert_eq!(after.spatial_offset.y, -10.0);
    assert_eq!(probe.borrow().captures.len(), 3);

    cover_at(&mut driver, 1);
    assert_eq!(probe.borrow().visibility, [true, false]);
    assert!(snapshot(draw(&driver, window), live).is_none());
    complete(third);
    driver.advance_to(std::time::Duration::from_millis(3)).unwrap();
    assert_eq!(probe.borrow().visibility, [true, false]);
}

fn draw(driver: &super::HeadlessDriver, window: WindowId) -> &DisplayList {
    driver.app().runtime().window(window).unwrap().last_commit().unwrap().draw.as_deref().unwrap()
}

fn snapshot(display: &DisplayList, node: NodeId) -> Option<octara_model::paint::PlacedItem<'_>> {
    display.placed_items().find(|item| matches!(item.item, Item::Image(image) if image.source == node))
}
