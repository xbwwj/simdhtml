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
//! The query sets are the checked-in selector files (`bench/selectors/cc`),
//! not synthetic documents.  SWDE uses the per-site selectors carried by the
//! corpus (checked-in `bench/selectors/swde/<site>.sel`).

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion};
use simdhtml::Document;
use simdhtml_bench::corpus::{Corpus, Dataset};
use simdhtml_bench::{bytes_group, env};
use simdsel::Selector;

/// Per-document selectors, when the corpus carries them (SWDE).
fn per_doc_queries(corpus: &Corpus) -> Option<Vec<Vec<String>>> {
    let any = corpus.docs.iter().any(|doc| !doc.queries.is_empty());
    any.then(|| corpus.docs.iter().map(|doc| doc.queries.clone()).collect())
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
        eprintln!("select/swde: corpus has no per-site selectors; skipped");
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
    let mut found = false;

    if let Some(cc) = Corpus::load(Dataset::CommonCrawl) {
        found = true;
        let sets = cc.workload_sets();
        if sets.is_empty() {
            eprintln!("select/cc: no selectors under bench/selectors/cc; skipped");
        } else {
            bench_cold(&mut c, &cc, &sets);
            bench_warm(&mut c, &cc, &sets);
        }
    }
    if let Some(swde) = Corpus::load(Dataset::Swde) {
        found = true;
        bench_swde(&mut c, &swde);
    }
    if !found {
        env::skip("select corpus", None);
    }
    c.final_summary();
}
