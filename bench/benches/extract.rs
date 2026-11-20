//! Extraction throughput: parse + fixed CSS selectors on real workloads.
//!
//! ```text
//! cargo bench -p simdhtml-bench --bench extract --features external
//! cargo bench -p simdhtml-bench --bench extract --features external -- --quick
//! ```
//!
//! Engines: simdsel in all three repair modes, scraper (html5ever + servo
//! selectors), and dom_query (html5ever + its own selector engine).
//!
//! Documents come from `simdhtml-bench`; CC uses the checked-in workload selector
//! sets, SWDE uses the per-site selectors from the generated manifest.

use std::hint::black_box;

use criterion::{BenchmarkGroup, Criterion, measurement::WallTime};
use simdhtml::{Document, ParseOptions, RepairMode};
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

fn bench_simdsel_common(
    group: &mut BenchmarkGroup<'_, WallTime>,
    corpus: &Corpus,
    sets: &[(String, Vec<String>)],
    mode: RepairMode,
) {
    let id = format!("simdhtml-{mode}");
    group.bench_function(&id, |b| {
        b.iter(|| {
            let mut matches = 0usize;
            for doc in &corpus.docs {
                let parsed =
                    Document::parse_with(black_box(&doc.bytes), ParseOptions::new().repair(mode))
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
    let id = format!("simdhtml-{mode}");
    group.bench_function(&id, |b| {
        b.iter(|| {
            let mut matches = 0usize;
            for (doc, queries) in corpus.docs.iter().zip(queries) {
                let parsed =
                    Document::parse_with(black_box(&doc.bytes), ParseOptions::new().repair(mode))
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

fn bench_common(c: &mut Criterion, corpus: &Corpus, sets: &[(String, Vec<String>)]) {
    let strings: Vec<String> = corpus
        .docs
        .iter()
        .map(|doc| String::from_utf8_lossy(&doc.bytes).into_owned())
        .collect();
    let mut group = bytes_group(c, &format!("extract/{}", corpus.name), corpus.total_bytes());

    for mode in [RepairMode::None, RepairMode::Practical, RepairMode::Spec] {
        bench_simdsel_common(&mut group, corpus, sets, mode);
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
}

fn bench_per_doc(c: &mut Criterion, corpus: &Corpus, queries: &[Vec<String>]) {
    let strings: Vec<String> = corpus
        .docs
        .iter()
        .map(|doc| String::from_utf8_lossy(&doc.bytes).into_owned())
        .collect();
    let mut group = bytes_group(c, &format!("extract/{}", corpus.name), corpus.total_bytes());

    for mode in [RepairMode::None, RepairMode::Practical, RepairMode::Spec] {
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
}

fn main() {
    let mut c = Criterion::default().configure_from_args();
    let sets = workload_sets();
    let mut found = false;

    if let Some(cc) = Corpus::load(Dataset::CommonCrawl) {
        found = true;
        println!("{}: {} docs, {:.1} MiB", cc.name, cc.len(), cc.mib());
        if sets.is_empty() {
            eprintln!("extract/cc: no workload selectors; skipped");
        } else {
            bench_common(&mut c, &cc, &sets);
        }
    }

    if let Some(swde) = Corpus::load(Dataset::Swde) {
        found = true;
        println!("{}: {} docs, {:.1} MiB", swde.name, swde.len(), swde.mib());
        if let Some(queries) = per_doc_queries(&swde) {
            bench_per_doc(&mut c, &swde, &queries);
        } else {
            eprintln!("extract/swde: manifest has no per-site selectors; skipped");
        }
    }

    if !found {
        env::skip("extract corpus", None);
    }
    c.final_summary();
}
