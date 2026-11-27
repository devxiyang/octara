use octara_runtime::ReactiveWorld;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct BufferAllocator;

static BUFFER: AtomicUsize = AtomicUsize::new(0);
static STORAGE_CHANGES: AtomicUsize = AtomicUsize::new(0);

// This integration test has one allocator probe. Every operation forwards the
// original allocation and layout to the system allocator.
unsafe impl GlobalAlloc for BufferAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr.addr() == BUFFER.load(Relaxed) {
            STORAGE_CHANGES.fetch_add(1, Relaxed);
        }
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ptr.addr() == BUFFER.load(Relaxed) {
            STORAGE_CHANGES.fetch_add(1, Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: BufferAllocator = BufferAllocator;

#[test]
fn memo_with_reuses_the_initial_allocation_during_recomputation() {
    struct Buffer(Vec<u32>);

    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let mut build = world.build(scope, None);
    let length = build.signal(1_u32);
    let buffer = Vec::<u32>::with_capacity(4096);
    let address = buffer.as_ptr().addr();
    let capacity = buffer.capacity();
    let memo = build.memo_with(Buffer(buffer), move |read, buffer| {
        buffer.0.clear();
        buffer.0.extend(0..read.get(length));
        true
    });
    BUFFER.store(address, Relaxed);
    let mut write = world.write(scope, None);
    for next in 1..=1024 {
        write.set(length, next);
        write.with(memo, |buffer| {
            assert_eq!((buffer.0.as_ptr().addr(), buffer.0.capacity()), (address, capacity));
            assert_eq!(buffer.0.last(), Some(&(next - 1)));
        });
    }
    BUFFER.store(0, Relaxed);
    assert_eq!(STORAGE_CHANGES.load(Relaxed), 0);
}
