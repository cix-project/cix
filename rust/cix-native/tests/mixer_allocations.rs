//! Regression guard for fixed-mix per-symbol scratch allocation.
//!
//! The count includes HashMap growth and arithmetic output allocation.  It is
//! intentionally a generous total budget; the old Vec scratch path alone made
//! tens of thousands of allocations for this fixture.

use cix_native::fixedmix;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Mutex;

struct CountingAllocator;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.with(Cell::get) {
            ALLOCATIONS.with(|count| count.set(count.get() + 1));
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if COUNTING.with(Cell::get) {
            ALLOCATIONS.with(|count| count.set(count.get() + 1));
        }
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if COUNTING.with(Cell::get) {
            ALLOCATIONS.with(|count| count.set(count.get() + 1));
        }
        unsafe { System.realloc(ptr, layout, size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

static SERIAL: Mutex<()> = Mutex::new(());

fn counted<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    COUNTING.with(|active| active.set(false));
    ALLOCATIONS.with(|count| count.set(0));
    COUNTING.with(|active| active.set(true));
    let result = operation();
    COUNTING.with(|active| active.set(false));
    (result, ALLOCATIONS.with(Cell::get))
}

#[test]
fn fixedmix_uses_bounded_scratch_allocations_per_serial_encode() {
    let _serial = SERIAL.lock().unwrap();
    let input = b"fixed mixer allocation fixture: abracadabra 0123456789\n".repeat(32);
    let (encoded, allocations) =
        counted(|| fixedmix::encode_with_history_eta(&input, &[], 6).unwrap());
    assert_eq!(fixedmix::decode(&encoded, input.len()).unwrap(), input);
    assert!(
        allocations < 1_000,
        "fixed-mix scratch allocation regression: {allocations} allocations for {} bytes",
        input.len()
    );
}
