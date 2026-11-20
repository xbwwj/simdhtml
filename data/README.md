# Benchmark / conformance data

The only authoritative benchmark inputs are these real corpora.  There are no
synthetic benchmark documents.

| Dataset | Default path | Role | How to get it |
|---|---|---|---|
| WPT | `data/wpt` | HTML tree-construction conformance | git submodule (`git submodule update --init --recursive`) |
| Common Crawl | `data/commoncrawl/html` | parse/markup-scan/select throughput, size buckets | `./x data-cc` |
| SWDE | `data/prepared/swde` | structured extraction, per-site selectors | `./x data-swde` |
| manifests | `data/prepared/manifests/*.jsonl` | the benches read the same documents | `./x data-swde` |

`data/commoncrawl/`, `data/swde/`, and `data/prepared/` are local artifacts and
are gitignored; WPT is the only submodule.

The Common Crawl WARC is decoded by the Rust `warc` crate
(`bench/src/bin/cc-export.rs`), not by a Python extractor.

## Environment overrides

The benches and tests never require a path argument.  Discovery defaults to
walking up from the crate to `data/`; override when the data lives elsewhere:

| Variable | Meaning |
|---|---|
| `SIMDHTML_DATA_DIR` | root containing `wpt/`, `commoncrawl/`, `swde/`, `prepared/` |
| `SIMDHTML_WPT_DIR` | WPT checkout |
| `SIMDHTML_CC_DIR` | exported Common Crawl HTML directory |
| `SIMDHTML_SWDE_DIR` | prepared SWDE documents |
| `SIMDHTML_PREPARED_DIR` | `data/prepared` |
| `SIMDHTML_BENCH_MANIFEST` | one explicit JSONL manifest |
| `SIMDHTML_BENCH_LIMIT` | documents per corpus (default 500; quick bench 60) |
| `SIMDHTML_BENCH_BYTES` | byte budget per corpus |
| `SIMDHTML_BENCH_MAX_DOC_BYTES` | drop larger documents (default 4 MiB) |
| `SIMDHTML_BENCH_ALL=1` | disable the sample bounds |
| `SIMDHTML_REQUIRE_TESTDATA=1` | missing data is an error, not a skip |
| `SIMDHTML_RUN_CORPUS_TESTS=1` | run the Common Crawl preorder-invariant test |

## Commands

```bash
./x data          # WARC download/extract + SWDE preparation + manifests
./x data-cc       # only the Common Crawl half
./x data-swde     # only the SWDE half

./x test          # Rust tests + PyO3 binding smoke test
./x test-wpt      # WPT tree construction
./x test-corpus   # Common Crawl preorder invariants

./x bench         # full Criterion run
./x bench-quick   # smaller/faster sample for iteration
X_BENCH_TARGETS='markup parse' ./x bench-quick   # only selected targets
```

`scripts/` keeps only the data/selector helpers that are called by `./x data`
and the XPath differential tool; the old shell entry points and the Python
benchmark harness are gone.

For a portable offline benchmarking bundle:

```bash
./x bundle                        # sample data
./x bundle --data full --with-warc
```

The bundle includes `data/prepared/manifests/*.jsonl`, the referenced documents,
the selector workloads, the prebuilt binaries, and a portable CPython runtime.
