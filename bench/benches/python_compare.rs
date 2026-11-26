//! Python-engine comparison through an embedded CPython interpreter.
//!
//! ```text
//! PYO3_PYTHON=.venv/bin/python \
//!   cargo bench -p simdhtml-bench --features external,python --bench python_compare
//! ```
//!
//! Criterion owns the measurement; the Python libraries are called through
//! PyO3.  The engines are the ones a Scrapy user would compare against:
//! parsel (lxml behind it), selectolax (Lexbor) and beautifulsoup4 (stdlib
//! `html.parser` + soupsieve).  Rust `simdsel` is measured by
//! the other bench targets; this file is only the Python side.
//!
//! The Python packages are installed by:
//!     uv sync --group bench
//! and `cargo xtask bench --compare` sets the venv for the embedding.

use std::hint::black_box;

use criterion::{BenchmarkGroup, Criterion, measurement::WallTime};
use pyo3_embed::prelude::*;
use pyo3_embed::types::PyAny;
use simdhtml_bench::corpus::{Corpus, Dataset, Subset};
use simdhtml_bench::python::add_venv_sys_path;
use simdhtml_bench::{bytes_group, env, record_input};

fn per_doc_queries(corpus: &Corpus) -> Option<Vec<Vec<String>>> {
    let any = corpus.docs.iter().any(|doc| !doc.queries.is_empty());
    any.then(|| corpus.docs.iter().map(|doc| doc.queries.clone()).collect())
}

#[derive(Clone, Copy)]
enum PyEngine {
    Parsel,
    Selectolax,
    BeautifulSoup,
}

impl PyEngine {
    const ALL: [PyEngine; 3] = [
        PyEngine::Parsel,
        PyEngine::Selectolax,
        PyEngine::BeautifulSoup,
    ];

    fn name(self) -> &'static str {
        match self {
            PyEngine::Parsel => "python-parsel",
            PyEngine::Selectolax => "python-selectolax",
            PyEngine::BeautifulSoup => "python-beautifulsoup4",
        }
    }

    fn parse<'py>(self, py: Python<'py>, html: &str) -> PyResult<Bound<'py, PyAny>> {
        match self {
            PyEngine::Parsel => py.import("parsel")?.getattr("Selector")?.call1((html,)),
            PyEngine::Selectolax => py
                .import("selectolax.lexbor")?
                .getattr("LexborHTMLParser")?
                .call1((html,)),
            // `html.parser` is bs4's own tree builder; the lxml backend would
            // just measure lxml again.
            PyEngine::BeautifulSoup => py
                .import("bs4")?
                .getattr("BeautifulSoup")?
                .call1((html, "html.parser")),
        }
    }

    fn select(self, tree: &Bound<'_, PyAny>, query: &str) -> PyResult<usize> {
        match self {
            // bs4 spells CSS selection `select`; parsel and selectolax use `css`.
            PyEngine::BeautifulSoup => tree.call_method1("select", (query,))?.len(),
            _ => tree.call_method1("css", (query,))?.len(),
        }
    }
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
    for engine in PyEngine::ALL {
        group.bench_function(engine.name(), |b| {
            b.iter(|| {
                Python::attach(|py| {
                    let mut matches = 0usize;
                    for html in &strings {
                        let tree = engine.parse(py, black_box(html)).expect("python parse");
                        for (_, queries) in sets {
                            for query in queries {
                                matches = matches.wrapping_add(
                                    engine
                                        .select(&tree, black_box(query))
                                        .expect("python select"),
                                );
                            }
                        }
                    }
                    black_box(matches)
                })
            });
        });
    }
    group.finish();

    record_input(name, subset.len(), subset.total_bytes(), bucket);
}

fn bench_per_doc(c: &mut Criterion, corpus: &Corpus, queries: &[Vec<String>], strings: &[String]) {
    let name = format!("python/{}/per-site", corpus.name);
    let mut group = bytes_group(c, &name, corpus.total_bytes());
    for engine in PyEngine::ALL {
        group.bench_function(engine.name(), |b| {
            b.iter(|| {
                Python::attach(|py| {
                    let mut matches = 0usize;
                    for (html, queries) in strings.iter().zip(queries) {
                        if queries.is_empty() {
                            continue;
                        }
                        let tree = engine.parse(py, black_box(html)).expect("python parse");
                        for query in queries {
                            matches = matches.wrapping_add(
                                engine
                                    .select(&tree, black_box(query))
                                    .expect("python select"),
                            );
                        }
                    }
                    black_box(matches)
                })
            });
        });
    }
    group.finish();

    record_input(&name, corpus.len(), corpus.total_bytes(), None);
}

/// Keep the type visible to rustdoc without forcing the bench group helper to
/// know about it.
#[allow(dead_code)]
fn _group_type(_: &BenchmarkGroup<'_, WallTime>) {}

fn main() {
    // Import once so a missing package fails before any criterion timing.
    Python::attach(|py| {
        add_venv_sys_path(py);
        for engine in PyEngine::ALL {
            if let Err(err) = engine.parse(py, "<p>x</p>") {
                panic!(
                    "{} is not importable: {err}\nprepare the venv with: uv sync --group bench",
                    engine.name()
                );
            }
        }
    });

    let mut c = Criterion::default().configure_from_args();
    let mut found = false;

    if let Some(cc) = Corpus::load(Dataset::CommonCrawl) {
        found = true;
        let sets = cc.workload_sets();
        if sets.is_empty() {
            eprintln!("python/cc: no selectors under bench/selectors/cc; skipped");
        } else {
            let whole = Subset {
                name: cc.name.clone(),
                docs: cc.docs.iter().collect(),
            };
            bench_common(&mut c, "python/commoncrawl/common", &whole, &sets, None);
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
            let name = format!("python/commoncrawl/{bucket}");
            bench_common(&mut c, &name, &subset, &sets, Some((index, &bucket)));
        }
    }

    if let Some(swde) = Corpus::load(Dataset::Swde) {
        found = true;
        let strings: Vec<String> = swde
            .docs
            .iter()
            .map(|doc| String::from_utf8_lossy(&doc.bytes).into_owned())
            .collect();
        if let Some(queries) = per_doc_queries(&swde) {
            bench_per_doc(&mut c, &swde, &queries, &strings);
        } else {
            eprintln!("python/swde: corpus has no per-site selectors; skipped");
        }
    }

    if !found {
        env::skip("python_compare corpus", None);
    }
    c.final_summary();
}
