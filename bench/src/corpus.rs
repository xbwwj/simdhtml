//! The `simdhtml-corpus` crate owns data discovery and sampling: Common Crawl
//! is streamed from the `.warc.gz` and SWDE is read through the index that
//! `make extract-data` writes.  This module only re-exports it so the benches
//! keep importing `simdhtml_bench::corpus`.

pub use simdhtml_corpus::*;
