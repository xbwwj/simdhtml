//! Throughput probe for stage-1 (`MarkupIndex`).
//!
//! Run with:
//!
//! ```text
//! cargo bench -p simdhtml --bench stage1
//! ```
//!
//! The input mixes ordinary tags, quoted attributes, comments and rawtext so
//! the fixed point and raw mask are exercised, not just the classifier.

use std::hint::black_box;
use std::time::Instant;

use simdhtml::build_markup_index;
use simdhtml::simd::features;

const MIB: usize = 1024 * 1024;

const PATTERN: &[u8] = br#"<div class="item" data-id="42"><a href="/x?a=1&amp;b=2" title='a"b'>hello</a><!-- note <b> --><span>world</span><script>if (a < b) { x = 1; }</script></div>"#;

fn make_input(target: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(target + PATTERN.len());
    while data.len() < target {
        data.extend_from_slice(PATTERN);
    }
    data.truncate(target);
    data
}

fn best_of(reps: usize, mut f: impl FnMut() -> u64) -> (u64, f64) {
    let mut best = f64::MAX;
    let mut out = 0;
    for _ in 0..reps {
        let t0 = Instant::now();
        out = f();
        let dt = t0.elapsed().as_secs_f64();
        if dt < best {
            best = dt;
        }
    }
    (out, best)
}

fn main() {
    let data = make_input(64 * MIB);
    let bytes = data.len();
    println!("features: {:?}", features());
    println!("input: {:.2} MiB", bytes as f64 / MIB as f64);

    let (tags, dt) = best_of(7, || {
        let index = build_markup_index(black_box(&data)).unwrap();
        black_box(index.tag_count() as u64)
    });
    let gbs = bytes as f64 / dt / 1e9;
    println!(
        "stage1: {:8.2} GB/s  {:8.2} ms  (tags={tags})",
        gbs,
        dt * 1e3
    );
}
