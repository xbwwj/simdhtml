//! Shared benchmark support: data discovery, corpus loading, and criterion
//! registration for the `bench/` package.  Nothing here is part of the
//! published parser or selector crates.
//!
//! The only authoritative benchmark inputs are the three real corpora:
//! `data/wpt`, `data/commoncrawl`, and `data/swde` (plus the derived
//! `data/prepared` manifests).  There is deliberately no synthetic document
//! generator here.

pub mod corpus;
pub mod criterion_helpers;
pub mod env;
#[cfg(feature = "python")]
pub mod python;

pub use corpus::{Corpus, Dataset, Doc};
pub use criterion_helpers::{bench_parse, bytes_group, count_group};
pub use env::{data_dir, require_testdata};
