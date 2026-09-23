//! Instrumentation used only in a separate, untimed benchmark build.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
};

struct CountingAllocator;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
static ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static DEALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static REALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

// SAFETY: Every operation forwards the original pointer and layout to System.
// Instrumentation only updates atomics and cannot allocate or unwind.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ENABLED.load(Relaxed) {
            ALLOCATIONS.fetch_add(1, Relaxed);
            BYTES.fetch_add(layout.size() as u64, Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if ENABLED.load(Relaxed) {
            ALLOCATIONS.fetch_add(1, Relaxed);
            BYTES.fetch_add(layout.size() as u64, Relaxed);
        }
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ENABLED.load(Relaxed) {
            DEALLOCATIONS.fetch_add(1, Relaxed);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if ENABLED.load(Relaxed) {
            REALLOCATIONS.fetch_add(1, Relaxed);
            BYTES.fetch_add(size as u64, Relaxed);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}

pub struct Counts {
    pub allocations: u64,
    pub deallocations: u64,
    pub reallocations: u64,
    pub bytes: u64,
}

pub fn measure(f: impl FnOnce()) -> Counts {
    ALLOCATIONS.store(0, Relaxed);
    DEALLOCATIONS.store(0, Relaxed);
    REALLOCATIONS.store(0, Relaxed);
    BYTES.store(0, Relaxed);
    ENABLED.store(true, Relaxed);
    f();
    ENABLED.store(false, Relaxed);
    Counts {
        allocations: ALLOCATIONS.load(Relaxed),
        deallocations: DEALLOCATIONS.load(Relaxed),
        reallocations: REALLOCATIONS.load(Relaxed),
        bytes: BYTES.load(Relaxed),
    }
}
