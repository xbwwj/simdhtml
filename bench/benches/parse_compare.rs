//! Parse-only comparison against html5ever + rcdom on the real corpora.
//!
//! ```text
//! cargo bench -p simdhtml-bench --bench parse_compare --features external
//! cargo bench -p simdhtml-bench --bench parse_compare --features external -- --quick
//! ```

use std::hint::black_box;

use criterion::Criterion;
use html5ever_039::driver::parse_document;
use html5ever_039::tendril::TendrilSink;
use markup5ever_rcdom::RcDom;
use simdhtml::{Document, ParseOptions, RepairMode};
use simdhtml_bench::corpus::{Corpus, Dataset};
use simdhtml_bench::{bytes_group, env};

fn bench_corpus(c: &mut Criterion, corpus: &Corpus) {
    for (bucket, ids) in corpus.buckets() {
        let subset = corpus.subset(&bucket, &ids);
        let strings: Vec<String> = subset
            .docs
            .iter()
            .map(|doc| String::from_utf8_lossy(&doc.bytes).into_owned())
            .collect();
        let name = format!("parse_compare/{}/{bucket}", corpus.name);
        let mut group = bytes_group(c, &name, subset.total_bytes());

        group.bench_function("simdhtml-none", |b| {
            b.iter(|| {
                let mut nodes = 0usize;
                for doc in &subset.docs {
                    nodes = nodes.wrapping_add(
                        Document::parse_with(
                            black_box(&doc.bytes),
                            ParseOptions::new().repair(RepairMode::None),
                        )
                        .expect("parse")
                        .node_tape
                        .len(),
                    );
                }
                black_box(nodes)
            });
        });

        group.bench_function("simdhtml-spec", |b| {
            b.iter(|| {
                let mut nodes = 0usize;
                for doc in &subset.docs {
                    nodes = nodes.wrapping_add(
                        Document::parse_with(
                            black_box(&doc.bytes),
                            ParseOptions::new().repair(RepairMode::Spec),
                        )
                        .expect("parse")
                        .node_tape
                        .len(),
                    );
                }
                black_box(nodes)
            });
        });

        group.bench_function("html5ever", |b| {
            b.iter(|| {
                let mut nodes = 0usize;
                for html in &strings {
                    let dom = parse_document(RcDom::default(), Default::default())
                        .one(black_box(html.clone()));
                    nodes = nodes.wrapping_add(dom.document.children.borrow().len());
                    black_box(&dom);
                }
                black_box(nodes)
            });
        });

        group.finish();
    }
}

fn main() {
    let mut c = Criterion::default().configure_from_args();
    let mut found = false;
    for dataset in [Dataset::CommonCrawl, Dataset::Swde] {
        if let Some(corpus) = Corpus::load(dataset) {
            found = true;
            bench_corpus(&mut c, &corpus);
        }
    }
    if !found {
        env::skip("parse_compare corpus", None);
    }
    c.final_summary();
}
