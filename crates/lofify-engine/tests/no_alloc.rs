//! Proves the real-time paths never allocate: a counting allocator watches the test thread while
//! the capture writer and output renderer run through every state (prefill, running, underrun,
//! idle, overrun trimming, resampling, format conversion, parameter and bypass changes).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use lofify_dsp::params::Params;
use lofify_dsp::shared::SharedParams;
use lofify_engine::capture::CaptureWriter;
use lofify_engine::metrics::Shared;
use lofify_engine::output::{OutputRenderer, RendererConfig};
use rtrb::RingBuffer;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn note() {
    if COUNTING.with(Cell::get) {
        ALLOCATIONS.with(|a| a.set(a.get() + 1));
    }
}

struct Counting;

// SAFETY: forwards every call unchanged to the system allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn run(capture_rate: u32, output_rate: u32) -> usize {
    let (producer, consumer) = RingBuffer::<f32>::new(2 * 48_000);
    let shared = Arc::new(Shared::default());
    let params = Arc::new(SharedParams::new(&Params::default()));
    let mut writer = CaptureWriter::new(producer, shared.clone());
    let mut renderer = OutputRenderer::new(
        consumer,
        params.clone(),
        shared.clone(),
        RendererConfig {
            capture_rate,
            output_rate,
            ..RendererConfig::default()
        },
    )
    .unwrap();

    // Everything that may allocate is prepared before counting starts.
    let input_i16: Vec<i16> = (0..441 * 6).map(|i| ((i * 97) % 20_000) as i16).collect();
    let input_f32: Vec<f32> = (0..441 * 2)
        .map(|i| ((i * 31) % 100) as f32 / 200.0)
        .collect();
    let mut out_f32 = vec![0.0f32; 480 * 2];
    let mut out_i16 = vec![0i16; 480 * 6];
    let mut out_u16 = vec![0u16; 480];
    let stormy = [
        Params::default(),
        lofify_dsp::presets::builtin_preset("Classic Lofi")
            .unwrap()
            .params,
        lofify_dsp::presets::builtin_preset("Cassette")
            .unwrap()
            .params,
    ];
    let huge = vec![0.25f32; 2 * 30_000]; // forces the overrun-trim path

    COUNTING.with(|c| c.set(true));
    for tick in 0..600usize {
        params.store(&stormy[tick % 3]);
        params.set_bypass(tick % 11 == 0);
        // Mostly steady, with an outage (underrun/idle path) and a flood (overrun path).
        let outage = (200..260).contains(&tick);
        if !outage {
            if tick % 2 == 0 {
                writer.write(&input_i16, 6);
            } else {
                writer.write(&input_f32, 2);
            }
        }
        if tick == 400 {
            writer.write(&huge, 2);
        }
        match tick % 3 {
            0 => renderer.render(&mut out_f32, 2),
            1 => renderer.render(&mut out_i16, 6),
            _ => renderer.render(&mut out_u16, 1),
        }
    }
    COUNTING.with(|c| c.set(false));
    assert!(out_f32.iter().all(|v| v.is_finite()));
    let m = shared.snapshot();
    assert!(
        m.overruns >= 1,
        "the overrun path must have been exercised: {m:?}"
    );
    assert!(m.output_frames > 0);
    ALLOCATIONS.with(Cell::get)
}

#[test]
fn same_rate_path_does_not_allocate() {
    assert_eq!(run(48_000, 48_000), 0, "capture/render path allocated");
}

#[test]
fn resampling_path_does_not_allocate() {
    assert_eq!(run(44_100, 48_000), 0, "resampling path allocated");
}

#[test]
fn counter_actually_detects_allocations() {
    ALLOCATIONS.with(|a| a.set(0));
    COUNTING.with(|c| c.set(true));
    let v: Vec<u8> = Vec::with_capacity(4096);
    COUNTING.with(|c| c.set(false));
    assert!(v.capacity() >= 4096);
    assert!(
        ALLOCATIONS.with(Cell::get) >= 1,
        "allocation counter is broken"
    );
}
