//! Parse throughput on the real corpora, one group per repair mode.
//!
//! ```text
//! cargo bench -p simdhtml-bench --bench parse
//! cargo bench -p simdhtml-bench --bench parse -- --quick
//! ```
//!
//! `RepairMode::None` is the default and therefore the headline number; `Spec`
//! is measured on the same bytes so the cost of the specification's tree is
//! visible.  Inputs are streamed from the real corpora by `simdhtml-corpus`
//! (Common Crawl `.warc.gz` and SWDE `.7z`), never synthesised.

use criterion::Criterion;
use simdhtml::document::Document;
use simdhtml::parse::{ParseOptions, RepairMode};
use simdhtml_bench::corpus::{Corpus, Dataset};
use simdhtml_bench::{bench_parse, bytes_group, env};

const MODES: [RepairMode; 2] = [RepairMode::None, RepairMode::Spec];

fn bench_corpus(c: &mut Criterion, corpus: &Corpus) {
    for (bucket, ids) in corpus.buckets() {
        let subset = corpus.subset(&bucket, &ids);
        let name = format!("parse/{}/{bucket}", corpus.name);
        let mut group = bytes_group(c, &name, subset.total_bytes());
        for mode in MODES {
            bench_parse(&mut group, &subset, &format!("{mode:?}"), |bytes| {
                Document::parse_with(
                    bytes,
                    ParseOptions {
                        repair: mode,
                        ..Default::default()
                    },
                )
                .expect("parse")
                .nodes
                .len()
            });
        }
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
        env::skip("parse corpus", None);
    }
    c.final_summary();
}
