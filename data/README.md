# Data

Benchmark and conformance corpora. This directory is also the `simdhtml-data`
crate (`src/`).

| Path | Source |
|---|---|
| `commoncrawl/warc/*.warc.gz` | Common Crawl WARC |
| `swde/source` | SWDE submodule (`.7z` archives) |
| `swde/html` | SWDE working set, extracted from `swde/source` |
| `wpt` | WPT submodule (html5lib tree-construction `.dat` files) |

`commoncrawl/` and `swde/html/` are local and gitignored.

`cargo xtask data` fetches the submodules and the WARC, then extracts the SWDE
working set. `cargo xtask data --clear` removes them.
