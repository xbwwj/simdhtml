//! Shared benchmark support: data discovery, corpus streaming and criterion
//! registration for the `bench/` package.  Nothing here is part of the
//! published parser or selector crates.
//!
//! Inputs are the real corpora read straight from their compressed sources
//! (Common Crawl `.warc.gz`, SWDE `.7z`); see `simdhtml-corpus`.

pub use simdhtml_corpus as corpus;
pub use simdhtml_corpus::env;

pub use corpus::{Corpus, Dataset, Doc, Subset};
pub use criterion_helpers::{bench_parse, bytes_group, count_group};

pub mod criterion_helpers;
#[cfg(feature = "python")]
pub mod python;
