use octara_model::geometry::{Point, Size, WindowViewport};
use octara_model::identity::TreeId;
use octara_model::style::{Position, StyleProp};
use octara_model::theme::{Color, Fill};
use octara_pipeline::{NativeEngine, NativeEngineConfig};
use octara_runtime::HostEngine;
use octara_tree::tables::AuthoredStyle;
use octara_tree::{NodeChange, NodeContent, NodeSpec, Tree, TreeDelta};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::time::Instant;

struct CountingAllocator;

thread_local! {
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

fn record() {
    let _ = ALLOCATIONS.try_with(|count| {
        if let Some(n) = count.get() {
            count.set(Some(n + 1));
        }
    });
}

// The wrapper forwards each operation to the system allocator with its original layout.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record();
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn warm_frame_paint_and_hit_testing_do_not_allocate() {
    let mut tree = Tree::new(TreeId::new([3, 5], 1), Default::default());
    let mut tx = tree.transaction();
    let root = tree.node_ref(tree.root());
    for index in 0..4096 {
        let node = tx.create(NodeSpec::new(NodeContent::Box));
        let props = [
            StyleProp::Position(Position::Absolute),
            StyleProp::Left(((index % 64) * 24).into()),
            StyleProp::Top(((index / 64) * 24).into()),
            StyleProp::Width(20.into()),
            StyleProp::Height(20.into()),
            StyleProp::Background(Some(Fill::Solid(Color::BLACK.into()))),
        ];
        tx.change(node, NodeChange::Style(Box::new(AuthoredStyle { base: props.into_iter().collect(), ..Default::default() })));
        tx.place(root, index as usize, node);
    }
    let (_, mounted) = tree.apply(tx.finish()).unwrap();
    let mut initial = tree.initial_delta();
    initial.merge(mounted);
    let mut engine = NativeEngine::new(NativeEngineConfig::default());
    let viewport = WindowViewport { size: Size::new(1536.0, 1536.0), scale: 1.0, safe_area: None };
    engine.project(&tree, &initial, viewport, 0.0).unwrap();
    engine.commit();
    let delta = TreeDelta::default();
    for frame in 1..=4 {
        engine.project(&tree, &delta, viewport, frame as f64).unwrap();
        engine.commit();
    }

    let point = Point::new(12.0, 12.0);
    assert!(engine.hit_test(&tree, point).and_then(|target| target.hit.node()).is_some_and(|node| node != tree.root()));
    let mut frame_allocations = 0;
    for frame in 5..=20 {
        ALLOCATIONS.set(Some(0));
        let result = engine.project(&tree, &delta, viewport, frame as f64);
        frame_allocations += ALLOCATIONS.replace(None).unwrap();
        result.unwrap();
        engine.commit();
    }
    let iterations = 512;
    let start = Instant::now();
    ALLOCATIONS.set(Some(0));
    for _ in 0..iterations {
        black_box(engine.hit_test(&tree, black_box(point)));
    }
    let hit_allocations = ALLOCATIONS.replace(None).unwrap();
    eprintln!(
        "4096 nodes, 16 warm projections: {frame_allocations} allocations; {iterations} hit tests: {hit_allocations} allocations, {:.0} ns/hit",
        start.elapsed().as_nanos() as f64 / f64::from(iterations)
    );
    assert_eq!(frame_allocations, 0);
    assert_eq!(hit_allocations, 0);
}
