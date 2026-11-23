# simdhtml

A SIMD-accelerated HTML parser and CSS selector.

- Rust crates: [`simdhtml`](./crates/simdhtml) (parser) and
  [`simdsel`](./crates/simdsel) (CSS/XPath selection).
- Python binding: [`simdsel-py`](./crates/simdsel-py).

## Performance results

The following figures show how fast simdhtml extracts data from HTML. The task is to extract main content from most webpages and extract structured data from specified sites. The bench currently runs on an AMD Ryzen 5600 processor with single core. You can reproduce the result on your device with `./x bench`.

[![Main content and link extraction: Common Crawl, parse + 18 CSS selectors](./figures/main-content-and-link-extraction.svg)](./figures/main-content-and-link-extraction.svg)

[![Structured data extraction: SWDE, per-site field selectors](./figures/structured-data-extraction.svg)](./figures/structured-data-extraction.svg)

We also reached 98% HTML compliance in `RepairMode::Spec`, which is the
`simdsel (spec mode)` row: the WHATWG tree, about 40% of the no-repair speed.

The tree is the browser's, not libxml2's: `mode="spec"` inserts `<tbody>`,
foster-parents table content and runs the adoption agency, so selectors copied
from devtools (`table > tbody > tr`) work. `parsel` compatibility is API-level
(`css()`/`xpath()`/`get()`/`re()`/`attrib`, `::text`/`::attr()`); see
[`../与parsel不一致.md`](../与parsel不一致.md) for the deliberate differences.

## Development

```bash
./x data          # download/extract Common Crawl WARC, prepare SWDE + manifests
./x test          # Rust tests + PyO3 binding smoke test
./x test-wpt      # WPT tree-construction conformance
./x test-corpus   # Common Crawl preorder-invariant test
./x bench         # full Criterion run: simdhtml/simdsel + html5ever/scraper/dom_query + Python
./x bench-quick   # smaller/faster run for iteration
./x check         # cargo fmt --check + cargo clippy
./x bundle        # portable tarball: binaries + data + Python runtime
```

`./x bundle` builds a `.tar.zst` (falls back to `.tar.gz`) with the prebuilt
bench binaries, a sampled data tree, the uv site-packages, and a portable
CPython runtime.  Unpack it on the target machine and run `./x bench`; no build
or network is needed.

```bash
./x bundle                              # sample data (500 CC + 500 SWDE docs)
./x bundle --data full --with-warc      # full corpora
./x bundle --data none                  # binaries/Python only
./x bundle --cc-docs 100 --swde-docs 100
./x bundle --format gz
```

```bash
rm BUNDLE.env
X_FORCE_SOURCE=1 RUSTFLAGS='-C target-cpu=native' ./x bench
```

The Common Crawl WARC is decoded by the Rust [`warc`](https://crates.io/crates/warc)
crate (`bench/src/bin/cc-export.rs`); Python engines (parsel, selectolax, lxml)
are called from Rust through PyO3, so one Criterion run reports every engine.
