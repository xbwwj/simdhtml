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
//! and `./x bench` points `PYO3_PYTHON` at that environment.

use std::hint::black_box;

use criterion::{BenchmarkGroup, Criterion, measurement::WallTime};
use pyo3_embed::prelude::*;
use pyo3_embed::types::PyAny;
use simdhtml_bench::corpus::{Corpus, Dataset};
use simdhtml_bench::python::add_venv_sys_path;
use simdhtml_bench::{bytes_group, env};

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
    corpus: &Corpus,
    sets: &[(String, Vec<String>)],
    strings: &[String],
) {
    let mut group = bytes_group(
        c,
        &format!("python/{}/common", corpus.name),
        corpus.total_bytes(),
    );
    for engine in PyEngine::ALL {
        group.bench_function(engine.name(), |b| {
            b.iter(|| {
                Python::attach(|py| {
                    let mut matches = 0usize;
                    for html in strings {
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
}

fn bench_per_doc(c: &mut Criterion, corpus: &Corpus, queries: &[Vec<String>], strings: &[String]) {
    let mut group = bytes_group(
        c,
        &format!("python/{}/per-site", corpus.name),
        corpus.total_bytes(),
    );
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
                    "{} is not importable: {err}\ninstall the bench group with: uv sync --group bench",
                    engine.name()
                );
            }
        }
    });

    let sets = workload_sets();
    let mut c = Criterion::default().configure_from_args();
    let mut found = false;

    if let Some(cc) = Corpus::load(Dataset::CommonCrawl) {
        found = true;
        let strings: Vec<String> = cc
            .docs
            .iter()
            .map(|doc| String::from_utf8_lossy(&doc.bytes).into_owned())
            .collect();
        if sets.is_empty() {
            eprintln!("python/cc: no workload selectors; skipped");
        } else {
            bench_common(&mut c, &cc, &sets, &strings);
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
            eprintln!("python/swde: manifest has no per-site selectors; skipped");
        }
    }

    if !found {
        env::skip("python_compare corpus", None);
    }
    c.final_summary();
}
