# Benchmark and conformance data

This directory is also the `simdhtml-data` crate (`Cargo.toml`, `src/`).

Everything here is read in the shape it is shipped in; no synthetic documents.

| Path | Role |
|---|---|
| `data/commoncrawl/warc/*.warc.gz` | Common Crawl stream for parse/markup/select benchmarks |
| `data/swde/source` | SWDE submodule (`.7z` archives) |
| `data/swde/html` | bounded SWDE working set used by benchmarks |
| `data/wpt` | WPT submodule, tree-construction `.dat` files |
| `bench/selectors` | CSS workload definition, checked in |

`data/commoncrawl/` and `data/swde/html/` are local and gitignored.

`cargo xtask data` fetches the submodules and WARC, then extracts about 500
SWDE documents (under 40 MiB) from the solid `.7z` archives. The full SWDE
tree is 8 GiB and pure-Rust LZMA is too slow, so the benchmark reads the
bounded working set instead of decoding archives. `cargo xtask data --clear`
removes the downloaded WARC and the working set.
