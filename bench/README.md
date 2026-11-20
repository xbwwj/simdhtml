# Benchmarks

All benchmarks live here, outside `crates/`, in the `simdhtml-bench` package.
Criterion owns measurement and reporting for every engine, including the Python
ones.

```text
bench/
  src/                 # data discovery, corpus loading, criterion helpers
  src/bin/cc-export.rs # WARC -> data/commoncrawl/html (uses the `warc` crate)
  benches/markup.rs    # simdhtml markup scan
  benches/parse.rs     # simdhtml parse, all repair modes
  benches/classify.rs  # SIMD classify / prefix_xor
  benches/select.rs    # simdsel cold/warm select
  benches/parse_compare.rs  # simdhtml vs html5ever        (feature external)
  benches/extract.rs        # simdhtml vs scraper/dom_query (feature external)
  benches/python_compare.rs # parsel/selectolax/beautifulsoup4 via PyO3 (feature python)
  tests/python_binding.rs   # PyO3 binding smoke test
```

Inputs are only the real corpora (`data/wpt`, `data/commoncrawl`, `data/swde`);
see `data/README.md` for discovery and environment variables.

Use the root entry point:

```bash
./x bench          # full run, all targets/engines
./x bench-quick    # 60 docs / 16 MiB sample + Criterion --quick
X_BENCH_TARGETS='markup parse' ./x bench-quick
./x bench 'markup/swde/camera'   # Criterion filter
```

Direct Cargo invocations work too:

```bash
cargo bench -p simdhtml-bench
cargo bench -p simdhtml-bench --features external
PYO3_PYTHON="$PWD/.venv/bin/python" \
  cargo bench -p simdhtml-bench --features external,python --bench python_compare
```

`./x` sets `PYO3_PYTHON` and `PYTHONPATH` from the uv bench environment; Python
packages are installed with `uv sync --group bench`.
