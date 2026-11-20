//! Parse throughput on the real corpora, one group per repair mode.
//!
//! ```text
//! cargo bench -p simdhtml-bench --bench parse
//! cargo bench -p simdhtml-bench --bench parse -- --quick
//! ```
//!
//! `RepairMode::None` is the default and therefore the headline number;
//! `Practical` and `Spec` are measured on the same bytes so the cost of each
//! repair strategy is visible.  Inputs come from `simdhtml-bench` (`commoncrawl` and
//! `swde`), never from a synthetic pattern.

use criterion::Criterion;
use simdhtml::{Document, ParseOptions, RepairMode};
use simdhtml_bench::corpus::{Corpus, Dataset};
use simdhtml_bench::{bench_parse, bytes_group, env};

const MODES: [RepairMode; 3] = [RepairMode::None, RepairMode::Practical, RepairMode::Spec];

fn bench_corpus(c: &mut Criterion, corpus: &Corpus) {
    for (bucket, ids) in corpus.buckets() {
        let subset = corpus.subset(&bucket, &ids);
        let name = format!("parse/{}/{bucket}", corpus.name);
        let mut group = bytes_group(c, &name, subset.total_bytes());
        for mode in MODES {
            bench_parse(&mut group, &subset, &mode.to_string(), |bytes| {
                Document::parse_with(bytes, ParseOptions::new().repair(mode))
                    .expect("parse")
                    .node_tape
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
            println!(
                "{}: {} docs, {:.1} MiB",
                corpus.name,
                corpus.len(),
                corpus.mib()
            );
            bench_corpus(&mut c, &corpus);
        }
    }
    if !found {
        env::skip("parse corpus", None);
    }
    c.final_summary();
}
