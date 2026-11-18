//! Throughput probe for the stage-1 SIMD primitives.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p simdhtml --bench classify
//! ```
//!
//! This intentionally avoids criterion: M1 only needs a stable, quick
//! order-of-magnitude number to compare AVX2 against scalar.

use std::hint::black_box;
use std::time::Instant;

use simdhtml::simd::classify::classify_stage1;
use simdhtml::simd::prefix_xor::{prefix_xor, prefix_xor_scalar};
use simdhtml::simd::{BlockReader, features};

const MIB: usize = 1024 * 1024;

fn make_input(target: usize) -> Vec<u8> {
    let pattern = br#"<div class="item" data-id="42"><a href="/x?a=1&amp;b=2">hello</a><span>world</span></div>"#;
    let mut data = Vec::with_capacity(target + pattern.len());
    while data.len() < target {
        data.extend_from_slice(pattern);
    }
    data.truncate(target);
    data
}

fn best_of<F: FnMut() -> u64>(mut f: F, reps: usize) -> (u64, f64) {
    let mut best = f64::MAX;
    let mut out = 0u64;
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
    let f = features();
    println!("features: {f:?}");
    println!("input: {:.2} MiB", bytes as f64 / MIB as f64);

    let reps = 7;

    let (sum_simd, dt_simd) = best_of(
        || {
            let mut sum = 0u64;
            let mut reader = BlockReader::new(&data);
            while let Some(block) = reader.next_block() {
                let m = classify_stage1(black_box(block));
                sum = sum
                    .wrapping_add(m.lt.count_ones() as u64)
                    .wrapping_add(m.gt.count_ones() as u64)
                    .wrapping_add(m.dq.count_ones() as u64);
            }
            if let Some((tail, _)) = reader.tail_padded() {
                let m = classify_stage1(black_box(&tail));
                sum = sum.wrapping_add(m.lt.count_ones() as u64);
            }
            black_box(sum)
        },
        reps,
    );
    let simd_gbs = bytes as f64 / dt_simd / 1e9;
    println!(
        "classify_stage1 (dispatch): {:8.2} GB/s  {:8.2} ms  (sum={sum_simd})",
        simd_gbs,
        dt_simd * 1e3
    );

    let (sum_scalar, dt_scalar) = best_of(
        || {
            let mut sum = 0u64;
            let mut reader = BlockReader::new(&data);
            while let Some(block) = reader.next_block() {
                let m = simdhtml::simd::classify::classify_stage1_scalar(black_box(block));
                sum = sum
                    .wrapping_add(m.lt.count_ones() as u64)
                    .wrapping_add(m.gt.count_ones() as u64)
                    .wrapping_add(m.dq.count_ones() as u64);
            }
            if let Some((tail, _)) = reader.tail_padded() {
                let m = simdhtml::simd::classify::classify_stage1_scalar(black_box(&tail));
                sum = sum.wrapping_add(m.lt.count_ones() as u64);
            }
            black_box(sum)
        },
        reps,
    );
    let scalar_gbs = bytes as f64 / dt_scalar / 1e9;
    println!(
        "classify_stage1 (scalar):   {:8.2} GB/s  {:8.2} ms  (sum={sum_scalar})",
        scalar_gbs,
        dt_scalar * 1e3
    );
    if simd_gbs > 0.0 {
        println!("speedup: {:.2}x", simd_gbs / scalar_gbs);
    }

    // prefix_xor over 8-byte words.
    let words: Vec<u64> = data
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| u64::from_le_bytes(*c))
        .collect();
    let word_bytes = words.len() * 8;

    let (px_sum, px_dt) = best_of(
        || {
            let mut sum = 0u64;
            for &w in &words {
                sum = sum.wrapping_add(prefix_xor(black_box(w)));
            }
            black_box(sum)
        },
        reps,
    );
    let px_gbs = word_bytes as f64 / px_dt / 1e9;
    println!(
        "prefix_xor:         {:8.2} GB/s  {:8.2} ms  (sum={px_sum})",
        px_gbs,
        px_dt * 1e3
    );

    let (pxs_sum, pxs_dt) = best_of(
        || {
            let mut sum = 0u64;
            for &w in &words {
                sum = sum.wrapping_add(prefix_xor_scalar(black_box(w)));
            }
            black_box(sum)
        },
        reps,
    );
    let pxs_gbs = word_bytes as f64 / pxs_dt / 1e9;
    println!(
        "prefix_xor scalar:  {:8.2} GB/s  {:8.2} ms  (sum={pxs_sum})",
        pxs_gbs,
        pxs_dt * 1e3
    );
}
