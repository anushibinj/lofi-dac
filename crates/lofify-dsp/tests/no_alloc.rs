//! Proves the audio path never allocates: a counting global allocator watches the test thread
//! while the chain processes audio, changes parameters, syncs shared state and toggles bypass.

mod common;

use common::*;
use lofify_dsp::chain::Chain;
use lofify_dsp::params::Params;
use lofify_dsp::shared::SharedParams;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn note_allocation() {
    if COUNTING.with(Cell::get) {
        ALLOCATIONS.with(|a| a.set(a.get() + 1));
    }
}

struct Counting;

// SAFETY: forwards every call unchanged to the system allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_allocation();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[test]
fn audio_path_does_not_allocate() {
    let shared = SharedParams::default();
    let mut chain = Chain::new(SR);
    let mut buf = noise(512, 0.5, 1);
    let params = [classic(), extreme(), Params::default()];
    // Warm up outside the measured region.
    chain.process(&mut buf);

    COUNTING.with(|c| c.set(true));
    for round in 0..400 {
        shared.store(&params[round % params.len()]);
        shared.set_bypass(round % 5 == 0);
        chain.sync_from(&shared);
        chain.process(&mut buf);
    }
    COUNTING.with(|c| c.set(false));

    assert_eq!(ALLOCATIONS.with(Cell::get), 0, "the audio path allocated");
    assert!(buf.iter().all(|v| v.is_finite()));
}

#[test]
fn counter_actually_detects_allocations() {
    ALLOCATIONS.with(|a| a.set(0));
    COUNTING.with(|c| c.set(true));
    let v: Vec<u8> = Vec::with_capacity(1024);
    COUNTING.with(|c| c.set(false));
    assert!(v.capacity() >= 1024);
    assert!(
        ALLOCATIONS.with(Cell::get) >= 1,
        "allocation counter is broken"
    );
}
