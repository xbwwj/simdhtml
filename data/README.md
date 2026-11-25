# Benchmark / conformance data

No synthetic documents, and nothing is duplicated on disk: the benchmarks read
the real corpora in the shape they are shipped in.

Two layers, matching the `Makefile`:

| Layer | Command | What it produces |
|---|---|---|
| raw inputs | `make data` | Common Crawl `.warc.gz` (download), SWDE `.7z` (submodule) |
| working set | `make extract-data` | `data/swde-html/**` (unpacked) + `data/swde-index.tsv` |

| Dataset | Raw form | Working set | Role |
|---|---|---|---|
| Common Crawl | `data/commoncrawl/warc/*.warc.gz` | — (benches stream the WARC) | parse/markup-scan/select throughput, size buckets |
| SWDE | `data/swde/swde/sourceCode/sourceCode/*.7z` | `data/swde-html` + `data/swde-index.tsv` | structured extraction, per-site selectors |
| WPT | `data/wpt` (submodule) | — | `html/syntax/parsing/resources/*.dat`, tree-construction conformance |
| selectors | `bench/selectors` (checked in) | — | CSS workload definition: `cc/*.sel` sets, `swde/<site>.sel` per site |

`data/commoncrawl/`, `data/swde-html/` and `data/swde-index.tsv` are local
artifacts and are gitignored.  `data/swde` and `data/wpt` are submodules; the
derived SWDE tree lives outside the submodule so `git status` stays clean.

The tree-construction `.dat` files are the html5lib suite as maintained in WPT
(`html/syntax/parsing/resources`) — the standalone `html5lib-tests` repository
no longer carries them — so they come from the `wpt` submodule rather than a
copy in this repository.

## Why SWDE is unpacked but Common Crawl is not

Common Crawl is a single `.warc.gz` that the benches stream record by record
(`warc` + `flate2`); sampling stops as soon as the size-bucket quotas are met,
so a quick run touches a few MiB.

SWDE ships as eight solid `.7z` archives (124,291 documents, 8 GiB unpacked).
Decoding those inside the benchmark is far too slow (pure-Rust LZMA is ~3 MB/s
versus ~700 MB/s for `7z`), so `make extract-data` unpacks them once with the
`7z` CLI and writes a flat index:

```
data/swde-index.tsv      <size>\t<vertical>/<site>/<page>.htm   (sorted, ~5 MB)
```

The benches read that index sequentially, sample per vertical and per site, and
open only the selected documents.  A quick run therefore loads ~64 documents
(~5 MiB) without touching the other 124k files, and a full run is plain file
I/O with no decompression.

`make extract-data` needs the `7z` binary (`p7zip-full`) and ~8 GiB of disk.

## Environment overrides

The benches and tests never take a path argument; discovery walks up from the
crate to `data/`.  Override when the data lives elsewhere:

| Variable | Meaning |
|---|---|
| `SIMDHTML_DATA_DIR` | root containing `commoncrawl/`, `swde/`, `wpt/` |
| `SIMDHTML_WARC_DIR` | Common Crawl WARC directory |
| `SIMDHTML_SWDE_DIR` | SWDE submodule checkout |
| `SIMDHTML_SWDE_HTML_DIR` | extracted SWDE tree |
| `SIMDHTML_SWDE_INDEX` | SWDE document index |
| `SIMDHTML_SELECTORS_DIR` | selector files (default `bench/selectors`) |
| `SIMDHTML_WPT_DIR` | WPT submodule checkout |
| `SIMDHTML_BENCH_LIMIT` | documents kept per corpus (default 500; `bench-quick`/`bench-compare` use 60) |
| `SIMDHTML_BENCH_BYTES` | byte budget per corpus (default 64 MiB; quick/compare 16 MiB) |
| `SIMDHTML_BENCH_MAX_DOC_BYTES` | drop larger documents (default 4 MiB) |
| `SIMDHTML_BENCH_ALL=1` | keep every document (needs RAM for the whole corpus) |
| `SIMDHTML_REQUIRE_TESTDATA=1` | missing data is an error, not a skip |
| `SIMDHTML_RUN_CORPUS_TESTS=1` | run the Common Crawl preorder-invariant test |

## Commands

```bash
make data           # WARC download + SWDE submodule          (once, network)
make data-wpt       # WPT submodule (only for `make test-wpt`)
make extract-data   # 7z x + index.tsv                        (once, ~1 min, 8 GiB)
make bench-quick    # sampled criterion run for iteration     (no plots)
make bench-build    # build the bench executables (cargo, once)
make bench-compare  # scoreboard: extract + python_compare    (60 docs, figures/*.svg)
make bench          # full criterion run (runs the prebuilt binaries)
make venv           # .venv with parsel/selectolax/bs4 via pip (venv-cn/venv-ali: mirrors)
make info           # data, venv, bench binaries and the targets that will run
```

Benchmarks never download or unpack anything implicitly: a missing input fails
with the command to run.

A quick run can be narrowed; the sampling knobs also apply per run:

```bash
make bench-parse
make bench-quick SIMDHTML_BENCH_LIMIT=200 SIMDHTML_BENCH_BYTES=$((64*1024*1024))
make bench-compare SIMDHTML_BENCH_LIMIT=200
SIMDHTML_BENCH_ALL=1 make bench-parse
```

## Running the benchmarks on another machine

`make bench-build` stages the executables under `target/bench/bin`; `make
bench` only runs them, so a copied working tree needs neither cargo nor the
registry cache.  Copy `Makefile`, `bench/`, `crates/`, `Cargo.toml`,
`Cargo.lock`, `scripts/`, `target/` (the staged binaries plus the `deps/` they
point into), `.venv/` and the working set (`data/commoncrawl/warc/*.warc.gz`,
`data/swde-html`, `data/swde-index.tsv`).  The raw inputs (`data/swde/*.7z`,
`data/wpt`) are only needed by `make extract-data`/`make test-wpt`.

```bash
# build machine
make data extract-data bench-build
rsync -a --exclude .git ./ server:simdhtml/

# server (only make + bash are required)
cd simdhtml && make bench-compare
```

`data/swde-html` is 8 GiB and is what makes the copy large; the alternative is
to ship `data/swde` (0.6 GiB) and run `make extract-data` on the server, which
needs the `7z` binary.  The copied `.venv` only works if the machine has the
same Python version and `libpython`; when it does not, `make bench` hides
`python_compare` and `make bench-compare` fails on it.  Rebuild the venv there
with `make venv` (`venv-cn` / `venv-ali` for a mirror) — the embedded-CPython
bench needs that Python version to match the prebuilt binary, otherwise
`python_compare` has to be rebuilt too.

## Differential harness

`make diff` compares the selector engines against parsel/lxml on the corpus
(`scripts/css_diff.py`, `scripts/parsel_api_diff.py`, `scripts/xpath_diff.py`).
It is the only target that needs the Python venv (`uv sync --group bench`) and
it is not part of `make bench`.
