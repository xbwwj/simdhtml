# simdhtml / simdsel

Rust HTML parsing (`simdhtml`) + CSS selection (`simdsel`) with a
parsel-shaped Python API (`simdsel-py`).

XPath is intentionally not exposed. See `ARCHITECTURE.zh.md` §10 for the
decision and the retained parsel-style CSS extensions (`::text`,
`::attr(name)`, `:contains`, `[attr!=value]`).

- Parser: `crates/simdhtml`
- Selector engine and parsel-shaped Rust API: `crates/simdsel`
- Python binding: `crates/simdsel-py`
- Architecture notes: [`ARCHITECTURE.zh.md`](ARCHITECTURE.zh.md)
- Benchmark harnesses: `crates/simdhtml/benches`, `crates/simdsel/benches`, `crates/simdsel-py/benchmarks`

```bash
# full check: fmt + clippy + Rust tests
scripts/check.sh

# core microbenches and end-to-end parse+select
scripts/bench-core.sh

# external comparisons and Common Crawl benchmarks
scripts/bench-full.sh

# Python extension and smoke tests
uv sync --group bench
uv run --group dev maturin develop --release
uv run --group bench python -m unittest discover -s crates/simdsel-py/python/tests -v
uv run --group bench python crates/simdsel-py/benchmarks/synthetic_corpora.py

# optional WPT tree-construction check (requires tests/wpt)
scripts/wpt-check.sh
```
