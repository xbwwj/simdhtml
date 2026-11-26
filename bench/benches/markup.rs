//! Markup scan (`markup::scan`) throughput on the real corpora.
//!
//! ```text
//! cargo xtask bench markup
//! cargo xtask bench --quick markup
//! ```
//!
//! Input is streamed from the Common Crawl `.warc.gz` and the SWDE `.7z`
//! archives (`simdhtml-data`, see `data/README.md`), sampled to a bounded
//! number of documents.  There is no synthetic filler pattern: the point is the
//! tag/attribute/region distribution real pages have.

use std::hint::black_box;

use criterion::Criterion;
use simdhtml::markup::scan;
use simdhtml_bench::corpus::{Corpus, Dataset};
use simdhtml_bench::{bytes_group, env};

fn bench_corpus(c: &mut Criterion, corpus: &Corpus) {
    for (bucket, ids) in corpus.buckets() {
        let subset = corpus.subset(&bucket, &ids);
        let name = format!("markup/{}/{}", corpus.name, bucket);
        let mut group = bytes_group(c, &name, subset.total_bytes());
        group.bench_function("scan", |b| {
            b.iter(|| {
                let mut tags = 0usize;
                for doc in &subset.docs {
                    let index = scan(black_box(&doc.bytes)).expect("scan");
                    tags = tags.wrapping_add(index.tag_open.len());
                }
                black_box(tags)
            });
        });
        group.finish();
    }
}

fn main() {
    let mut c = Criterion::default().configure_from_args();
    println!("features: {:?}", simdhtml::simd::instruction_sets());
    let mut found = false;
    for dataset in [Dataset::CommonCrawl, Dataset::Swde] {
        if let Some(corpus) = Corpus::load(dataset) {
            found = true;
            bench_corpus(&mut c, &corpus);
        }
    }
    if !found {
        env::skip("markup corpus", None);
    }
    c.final_summary();
}
