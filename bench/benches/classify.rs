//! SIMD primitive throughput on real corpus bytes.
//!
//! ```text
//! cargo bench -p simdhtml-bench --bench classify
//! cargo bench -p simdhtml-bench --bench classify -- --quick
//! ```
//!
//! This is a kernel-level bench (dispatch vs scalar `classify`, and
//! `prefix_xor`), but the bytes still come from `data/commoncrawl` and
//! the Common Crawl and SWDE archives via `simdhtml-corpus`; no repeated
//! synthetic pattern.

use std::hint::black_box;

use criterion::Criterion;
use simdhtml::simd::fallback::{classify as classify_scalar, prefix_xor as prefix_xor_scalar};
use simdhtml::simd::{classify, instruction_sets, prefix_xor};
use simdhtml_bench::corpus::{Corpus, Dataset};
use simdhtml_bench::{bytes_group, env};

fn bench_corpus(c: &mut Criterion, corpus: &Corpus) {
    let mut group = bytes_group(
        c,
        &format!("classify/{}", corpus.name),
        corpus.total_bytes(),
    );

    group.bench_function("dispatch", |b| {
        b.iter(|| {
            let mut sum = 0u64;
            for doc in &corpus.docs {
                for block in doc.bytes.as_chunks::<64>().0 {
                    let masks = classify(black_box(block));
                    sum = sum
                        .wrapping_add(masks.lt.count_ones() as u64)
                        .wrapping_add(masks.gt.count_ones() as u64)
                        .wrapping_add(masks.dq.count_ones() as u64);
                }
            }
            black_box(sum)
        });
    });

    group.bench_function("scalar", |b| {
        b.iter(|| {
            let mut sum = 0u64;
            for doc in &corpus.docs {
                for block in doc.bytes.as_chunks::<64>().0 {
                    let masks = classify_scalar(black_box(block));
                    sum = sum
                        .wrapping_add(masks.lt.count_ones() as u64)
                        .wrapping_add(masks.gt.count_ones() as u64)
                        .wrapping_add(masks.dq.count_ones() as u64);
                }
            }
            black_box(sum)
        });
    });

    group.bench_function("prefix_xor", |b| {
        b.iter(|| {
            let mut sum = 0u64;
            for doc in &corpus.docs {
                for word in doc.bytes.as_chunks::<8>().0 {
                    let word = u64::from_le_bytes(*word);
                    sum = sum.wrapping_add(prefix_xor(black_box(word)));
                }
            }
            black_box(sum)
        });
    });

    group.bench_function("prefix_xor_scalar", |b| {
        b.iter(|| {
            let mut sum = 0u64;
            for doc in &corpus.docs {
                for word in doc.bytes.as_chunks::<8>().0 {
                    let word = u64::from_le_bytes(*word);
                    sum = sum.wrapping_add(prefix_xor_scalar(black_box(word)));
                }
            }
            black_box(sum)
        });
    });

    group.finish();
}

fn main() {
    let mut c = Criterion::default().configure_from_args();
    println!("features: {:#?}", instruction_sets());
    let mut found = false;
    for dataset in [Dataset::CommonCrawl, Dataset::Swde] {
        if let Some(corpus) = Corpus::load(dataset) {
            found = true;
            bench_corpus(&mut c, &corpus);
        }
    }
    if !found {
        env::skip("classify corpus", None);
    }
    c.final_summary();
}
