//! The few criterion shapes every benchmark needs, so the register/throughput
//! boilerplate is not copied into every bench file.

use criterion::{BenchmarkGroup, BenchmarkId, Criterion, Throughput, measurement::WallTime};
use std::fs;
use std::hint::black_box;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

use crate::Subset;
use crate::corpus::SamplePolicy;

/// Criterion's result directory, resolved the same way criterion does.
fn criterion_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CRITERION_HOME") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        return PathBuf::from(dir).join("criterion");
    }
    crate::env::data_dir()
        .parent()
        .expect("workspace root")
        .join("target/criterion")
}

/// Record what a benchmark group measured, so `xtask plot` can turn criterion
/// medians into per-page throughput.  Keyed by the criterion group name and
/// merged, because each bench binary writes only its own groups.
pub fn record_input(group: &str, docs: usize, bytes: u64, bucket: Option<(usize, &str)>) {
    let path = criterion_dir().join("corpora.json");
    let mut entries: Map<String, Value> = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let policy = SamplePolicy::active();
    entries.insert(
        group.to_string(),
        json!({
            "docs": docs,
            "bytes": bytes,
            "bucket": bucket.map(|(_, name)| name),
            "bucket_index": bucket.map(|(index, _)| index),
            "sample": policy.describe(),
            "quick": cfg!(feature = "quick"),
        }),
    );
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("criterion directory");
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(&Value::Object(entries)).expect("json"),
    )
    .expect("write corpora.json");
}

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
