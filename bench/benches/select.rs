//! Selector throughput on the real corpora.
//!
//! ```text
//! cargo bench -p simdhtml-bench --bench select
//! cargo bench -p simdhtml-bench --bench select -- --quick
//! ```
//!
//! Two shapes:
//! * `cold` — parse the document and run the selector(s) in the timed loop;
//! * `warm` — parse once in setup, then reuse one `Selector`/`QueryCache` per
//!   document across the query list.
//!
//! The query sets are the checked-in workload files (`data/prepared/workload`),
//! not synthetic documents.  SWDE uses the per-site selectors carried by the
//! generated manifest.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion};
use simdhtml::Document;
use simdhtml_bench::corpus::{Corpus, Dataset};
use simdhtml_bench::{bytes_group, env};
use simdsel::Selector;

fn read_queries(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

fn workload_sets() -> Vec<(String, Vec<String>)> {
    let Some(dir) = env::workload_dir() else {
        return Vec::new();
    };
    ["cc10.sel", "complex8.sel"]
        .into_iter()
        .filter_map(|name| {
            let queries = read_queries(&dir.join(name));
            (!queries.is_empty()).then(|| (name.trim_end_matches(".sel").to_string(), queries))
        })
        .collect()
}

/// Per-document selectors from the SWDE manifest, when present.
fn per_doc_queries(corpus: &Corpus) -> Option<Vec<Vec<String>>> {
    let mut out = Vec::with_capacity(corpus.docs.len());
    let mut any = false;
    for doc in &corpus.docs {
        let queries = doc
            .queries
            .as_ref()
            .map(|path| read_queries(path))
            .unwrap_or_default();
        any |= !queries.is_empty();
        out.push(queries);
    }
    any.then_some(out)
}

fn bench_cold(c: &mut Criterion, corpus: &Corpus, sets: &[(String, Vec<String>)]) {
    let mut group = bytes_group(
        c,
        &format!("select/{}/cold", corpus.name),
        corpus.total_bytes(),
    );

    // One document + all queries, the crawl-loop shape.
    group.bench_function("all-queries", |b| {
        b.iter(|| {
            let mut matches = 0usize;
            for doc in &corpus.docs {
                let selector =
                    Selector::new(Document::parse(black_box(&doc.bytes)).expect("parse"));
                for (_, queries) in sets {
                    for query in queries {
                        matches = matches
                            .wrapping_add(selector.css_count(black_box(query)).expect("select"));
                    }
                }
            }
            black_box(matches)
        });
    });

    // One query at a time, so a regression in one selector is visible.
    for (set, queries) in sets {
        for query in queries {
            group.bench_function(BenchmarkId::new(set, query), |b| {
                b.iter(|| {
                    let mut matches = 0usize;
                    for doc in &corpus.docs {
                        let selector =
                            Selector::new(Document::parse(black_box(&doc.bytes)).expect("parse"));
                        matches = matches
                            .wrapping_add(selector.css_count(black_box(query)).expect("select"));
                    }
                    black_box(matches)
                });
            });
        }
    }
    group.finish();
}

fn bench_warm(c: &mut Criterion, corpus: &Corpus, sets: &[(String, Vec<String>)]) {
    let parsed: Vec<_> = corpus
        .docs
        .iter()
        .map(|doc| Selector::new(Document::parse(&doc.bytes).expect("parse")))
        .collect();

    let mut group = bytes_group(
        c,
        &format!("select/{}/warm", corpus.name),
        corpus.total_bytes(),
    );
    group.bench_function("all-queries", |b| {
        b.iter(|| {
            let mut matches = 0usize;
            for selector in &parsed {
                for (_, queries) in sets {
                    for query in queries {
                        matches = matches
                            .wrapping_add(selector.css_count(black_box(query)).expect("select"));
                    }
                }
            }
            black_box(matches)
        });
    });
    group.finish();
}

fn bench_swde(c: &mut Criterion, corpus: &Corpus) {
    let Some(queries) = per_doc_queries(corpus) else {
        eprintln!("select/swde: manifest has no per-site selectors; skipped");
        return;
    };

    let mut group = bytes_group(
        c,
        &format!("select/{}/cold", corpus.name),
        corpus.total_bytes(),
    );
    group.bench_function("all-queries", |b| {
        b.iter(|| {
            let mut matches = 0usize;
            for (doc, queries) in corpus.docs.iter().zip(&queries) {
                let selector =
                    Selector::new(Document::parse(black_box(&doc.bytes)).expect("parse"));
                for query in queries {
                    matches =
                        matches.wrapping_add(selector.css_count(black_box(query)).expect("select"));
                }
            }
            black_box(matches)
        });
    });
    group.finish();

    let parsed: Vec<_> = corpus
        .docs
        .iter()
        .map(|doc| Selector::new(Document::parse(&doc.bytes).expect("parse")))
        .collect();
    let mut group = bytes_group(
        c,
        &format!("select/{}/warm", corpus.name),
        corpus.total_bytes(),
    );
    group.bench_function("all-queries", |b| {
        b.iter(|| {
            let mut matches = 0usize;
            for (selector, queries) in parsed.iter().zip(&queries) {
                for query in queries {
                    matches =
                        matches.wrapping_add(selector.css_count(black_box(query)).expect("select"));
                }
            }
            black_box(matches)
        });
    });
    group.finish();
}

fn main() {
    let mut c = Criterion::default().configure_from_args();
    let sets = workload_sets();
    let mut found = false;

    if let Some(cc) = Corpus::load(Dataset::CommonCrawl) {
        found = true;
        println!("{}: {} docs, {:.1} MiB", cc.name, cc.len(), cc.mib());
        if sets.is_empty() {
            eprintln!("select/cc: no workload selectors under data/prepared/workload; skipped");
        } else {
            bench_cold(&mut c, &cc, &sets);
            bench_warm(&mut c, &cc, &sets);
        }
    }
    if let Some(swde) = Corpus::load(Dataset::Swde) {
        found = true;
        println!("{}: {} docs, {:.1} MiB", swde.name, swde.len(), swde.mib());
        bench_swde(&mut c, &swde);
    }
    if !found {
        env::skip("select corpus", None);
    }
    c.final_summary();
}
