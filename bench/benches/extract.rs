//! Extraction throughput: parse + fixed CSS selectors on real workloads.
//!
//! ```text
//! cargo xtask bench extract
//! cargo xtask bench --quick extract
//! ```
//!
//! Engines: simdsel in both repair modes, scraper (html5ever + servo
//! selectors), and dom_query (html5ever + its own selector engine).
//!
//! Documents are streamed from the real corpora by `simdhtml-data`; CC uses
//! the checked-in selector sets (`bench/selectors/cc`), SWDE the per-site
//! selectors (`bench/selectors/swde`).  Common Crawl is measured both as one
//! mixed corpus and per document-size bucket, which is what shows the
//! fixed-per-document cost of the DOM engines.

use std::hint::black_box;

use criterion::{BenchmarkGroup, Criterion, measurement::WallTime};
use simdhtml::document::Document;
use simdhtml::parse::{ParseOptions, RepairMode};
use simdhtml_bench::corpus::{Corpus, Dataset, Doc, Subset};
use simdhtml_bench::{bytes_group, env, record_input};
use simdsel::prelude::Selector;

/// Per-document selectors, when the corpus carries them (SWDE).
fn per_doc_queries(corpus: &Corpus) -> Option<Vec<Vec<String>>> {
    let any = corpus.docs.iter().any(|doc| !doc.queries.is_empty());
    any.then(|| corpus.docs.iter().map(|doc| doc.queries.clone()).collect())
}

fn bench_simdsel_common(
    group: &mut BenchmarkGroup<'_, WallTime>,
    docs: &[&Doc],
    sets: &[(String, Vec<String>)],
    mode: RepairMode,
) {
    let id = format!("simdhtml-{mode:?}");
    group.bench_function(&id, |b| {
        b.iter(|| {
            let mut matches = 0usize;
            for doc in docs {
                let parsed = Document::parse_with(
                    black_box(&doc.bytes),
                    ParseOptions {
                        repair: mode,
                        ..Default::default()
                    },
                )
                .expect("parse");
                let selector = Selector::new(parsed);
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
}

fn bench_simdsel_per_doc(
    group: &mut BenchmarkGroup<'_, WallTime>,
    corpus: &Corpus,
    queries: &[Vec<String>],
    mode: RepairMode,
) {
    let id = format!("simdhtml-{mode:?}");
    group.bench_function(&id, |b| {
        b.iter(|| {
            let mut matches = 0usize;
            for (doc, queries) in corpus.docs.iter().zip(queries) {
                let parsed = Document::parse_with(
                    black_box(&doc.bytes),
                    ParseOptions {
                        repair: mode,
                        ..Default::default()
                    },
                )
                .expect("parse");
                let selector = Selector::new(parsed);
                for query in queries {
                    matches =
                        matches.wrapping_add(selector.css_count(black_box(query)).expect("select"));
                }
            }
            black_box(matches)
        });
    });
}

fn bench_common(
    c: &mut Criterion,
    name: &str,
    subset: &Subset<'_>,
    sets: &[(String, Vec<String>)],
    bucket: Option<(usize, &str)>,
) {
    let strings: Vec<String> = subset
        .docs
        .iter()
        .map(|doc| String::from_utf8_lossy(&doc.bytes).into_owned())
        .collect();
    let mut group = bytes_group(c, name, subset.total_bytes());

    for mode in [RepairMode::None, RepairMode::Spec] {
        bench_simdsel_common(&mut group, &subset.docs, sets, mode);
    }

    group.bench_function("scraper", |b| {
        b.iter(|| {
            let mut matches = 0usize;
            for html in &strings {
                let doc = scraper::Html::parse_document(black_box(html));
                for (_, queries) in sets {
                    for query in queries {
                        let selector = scraper::Selector::parse(black_box(query)).expect("scraper");
                        matches = matches.wrapping_add(doc.select(&selector).count());
                    }
                }
            }
            black_box(matches)
        });
    });

    group.bench_function("dom_query", |b| {
        b.iter(|| {
            let mut matches = 0usize;
            for html in &strings {
                let doc = dom_query::Document::from(black_box(html.as_str()));
                for (_, queries) in sets {
                    for query in queries {
                        matches = matches.wrapping_add(doc.select(black_box(query)).length());
                    }
                }
            }
            black_box(matches)
        });
    });
    group.finish();

    record_input(name, subset.len(), subset.total_bytes(), bucket);
}

fn bench_per_doc(c: &mut Criterion, corpus: &Corpus, queries: &[Vec<String>]) {
    let strings: Vec<String> = corpus
        .docs
        .iter()
        .map(|doc| String::from_utf8_lossy(&doc.bytes).into_owned())
        .collect();
    let mut group = bytes_group(c, &format!("extract/{}", corpus.name), corpus.total_bytes());

    for mode in [RepairMode::None, RepairMode::Spec] {
        bench_simdsel_per_doc(&mut group, corpus, queries, mode);
    }

    group.bench_function("scraper", |b| {
        b.iter(|| {
            let mut matches = 0usize;
            for (html, queries) in strings.iter().zip(queries) {
                let doc = scraper::Html::parse_document(black_box(html));
                for query in queries {
                    let selector = scraper::Selector::parse(black_box(query)).expect("scraper");
                    matches = matches.wrapping_add(doc.select(&selector).count());
                }
            }
            black_box(matches)
        });
    });

    group.bench_function("dom_query", |b| {
        b.iter(|| {
            let mut matches = 0usize;
            for (html, queries) in strings.iter().zip(queries) {
                let doc = dom_query::Document::from(black_box(html.as_str()));
                for query in queries {
                    matches = matches.wrapping_add(doc.select(black_box(query)).length());
                }
            }
            black_box(matches)
        });
    });
    group.finish();

    record_input(
        &format!("extract/{}", corpus.name),
        corpus.len(),
        corpus.total_bytes(),
        None,
    );
}

fn main() {
    let mut c = Criterion::default().configure_from_args();
    let mut found = false;

    if let Some(cc) = Corpus::load(Dataset::CommonCrawl) {
        found = true;
        let sets = cc.workload_sets();
        if sets.is_empty() {
            eprintln!("extract/cc: no selectors under bench/selectors/cc; skipped");
        } else {
            let whole = Subset {
                name: cc.name.clone(),
                docs: cc.docs.iter().collect(),
            };
            bench_common(&mut c, "extract/commoncrawl", &whole, &sets, None);
        }
    }

    // Size buckets come from a separate sample so they do not skew the
    // whole-corpus numbers above.
    if let Some(buckets) = Corpus::load_stratified(Dataset::CommonCrawl) {
        found = true;
        let sets = buckets.workload_sets();
        for (index, (bucket, ids)) in buckets.buckets_by_size().into_iter().enumerate() {
            let subset = buckets.subset(&bucket, &ids);
            if subset.is_empty() {
                continue;
            }
            let name = format!("extract/commoncrawl/{bucket}");
            bench_common(&mut c, &name, &subset, &sets, Some((index, &bucket)));
        }
    }

    if let Some(swde) = Corpus::load(Dataset::Swde) {
        found = true;
        if let Some(queries) = per_doc_queries(&swde) {
            bench_per_doc(&mut c, &swde, &queries);
        } else {
            eprintln!("extract/swde: corpus has no per-site selectors; skipped");
        }
    }

    if !found {
        env::skip("extract corpus", None);
    }
    c.final_summary();
}
