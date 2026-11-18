//! Throughput probe for the raw-region pre-scan (`raw::scan_raw_spans`).
//!
//! Run with:
//!
//! ```text
//! cargo bench -p simdhtml --bench raw --features bench-internal
//! ```
//!
//! This isolates Pass A from the 64-byte SIMD block scan so raw-scan changes
//! can be compared without stage-1 noise.

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

use simdhtml::bench_scan_raw_spans;

const MIB: usize = 1024 * 1024;
const TARGET: usize = 32 * MIB;

/// Ordinary tags with quoted attributes, no comments/rawtext.
const TAG_HEAVY: &[u8] = br#"<div class="item" data-id="42"><a href="/x?a=1&amp;b=2" title='a"b'>hello</a><span>world</span></div>"#;

/// Comments and rawtext exercises every branch of the region scanner.
const MIXED: &[u8] = br#"<div class="item" data-id="42"><a href="/x?a=1&amp;b=2" title='a"b'>hello</a><!-- note <b> --><span>world</span><script>if (a < b) { x = 1; }</script></div>"#;

/// Mostly rawtext/comment content.
const RAW_HEAVY: &[u8] = br#"<script>if (a < b) { x = "</div>"; }</script><style>a>b{}</style><!-- note <b> --><textarea><p>x</p></textarea>"#;

fn make_input(pattern: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(TARGET + pattern.len());
    while data.len() < TARGET {
        data.extend_from_slice(pattern);
    }
    data.truncate(TARGET);
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
    println!("input: {:.2} MiB", TARGET as f64 / MIB as f64);
    for (name, pattern) in [
        ("tag-heavy", TAG_HEAVY),
        ("mixed", MIXED),
        ("raw-heavy", RAW_HEAVY),
    ] {
        let data = make_input(pattern);
        let bytes = data.len();
        let (spans, dt) = best_of(9, || {
            let (count, flags) = bench_scan_raw_spans(black_box(&data));
            black_box(flags);
            count as u64
        });
        let gbs = bytes as f64 / dt / 1e9;
        println!(
            "{name:10} {:8.2} GB/s  {:8.3} ms  (spans={spans})",
            gbs,
            dt * 1e3
        );
    }

    if let Some(dir) = std::env::args().nth(1) {
        bench_corpus(Path::new(&dir));
    }
}

fn collect_html(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_html(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "html") {
            out.push(path);
        }
    }
}

fn bench_corpus(dir: &Path) {
    let mut paths = Vec::new();
    collect_html(dir, &mut paths);
    paths.sort();
    if paths.is_empty() {
        println!("corpus {dir:?}: no .html files");
        return;
    }

    let docs: Vec<Vec<u8>> = paths.iter().filter_map(|p| std::fs::read(p).ok()).collect();
    let bytes: usize = docs.iter().map(Vec::len).sum();
    let (spans, dt) = best_of(7, || {
        let mut spans = 0u64;
        for doc in &docs {
            let (count, flags) = bench_scan_raw_spans(black_box(doc));
            black_box(flags);
            spans += count as u64;
        }
        spans
    });
    println!(
        "corpus {:10} {:8.2} GB/s  {:8.3} ms  ({} docs, {:.2} MiB, spans={spans})",
        dir.file_name().unwrap_or_default().to_string_lossy(),
        bytes as f64 / dt / 1e9,
        dt * 1e3,
        docs.len(),
        bytes as f64 / MIB as f64
    );
}
