# Test data

Rust integration tests live in the crate that owns them, for example
`crates/simdsel/tests/facade.rs`.

This directory holds external data used by compatibility and benchmark
harnesses:

- `wpt/` — WPT checkout/submodule. Only
  `html/syntax/parsing/resources` is used by
  `crates/simdhtml/tests/wpt_tree_construction.rs` (ignored test).
- `data/commoncrawl-one/` — local Common Crawl corpus used by
  `crates/simdsel-py/benchmarks/common_crawl.py`. This directory is gitignored.

Both `wpt/` and `data/` are gitignored so they can be populated independently
of the main repository.
