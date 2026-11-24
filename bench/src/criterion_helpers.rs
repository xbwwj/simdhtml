//! The few criterion shapes every benchmark needs, so the register/throughput
//! boilerplate is not copied into every bench file.

use criterion::{BenchmarkGroup, BenchmarkId, Criterion, Throughput, measurement::WallTime};
use std::hint::black_box;

use crate::Subset;

/// A group whose throughput is `bytes`.
pub fn bytes_group<'a>(
    c: &'a mut Criterion,
    name: &str,
    bytes: u64,
) -> BenchmarkGroup<'a, WallTime> {
    let mut group = c.benchmark_group(name);
    group.throughput(Throughput::Bytes(bytes));
    group
}

/// A group whose throughput is `elements`.
pub fn count_group<'a>(
    c: &'a mut Criterion,
    name: &str,
    elements: u64,
) -> BenchmarkGroup<'a, WallTime> {
    let mut group = c.benchmark_group(name);
    group.throughput(Throughput::Elements(elements));
    group
}

/// Parse every document once per iteration.  `f` returns the per-document
/// observation (normally the node count), which is black-boxed so the loop is
/// not optimised away.
pub fn bench_parse<F>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    subset: &Subset<'_>,
    mode: &str,
    mut f: F,
) where
    F: FnMut(&[u8]) -> usize,
{
    group.bench_function(BenchmarkId::new("parse", mode), |b| {
        b.iter(|| {
            let mut nodes = 0usize;
            for doc in &subset.docs {
                nodes = nodes.wrapping_add(f(black_box(&doc.bytes)));
            }
            black_box(nodes)
        });
    });
}
