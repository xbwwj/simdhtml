#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
mkdir -p target/bench

echo "== simdhtml vs html5ever =="
cargo bench -p simdhtml --bench html5ever_compare --features bench-external

echo "== simdsel vs scraper/lol_html =="
cargo bench -p simdsel --bench external_compare --features bench-external

echo "== uv sync =="
uv sync --group bench

echo "== python synthetic corpora =="
uv run --group bench python crates/simdsel-py/benchmarks/synthetic_corpora.py \
    --json target/bench/synthetic_corpora.json

if [[ -d tests/data/commoncrawl-one ]]; then
    echo "== common crawl =="
    uv run --group bench python crates/simdsel-py/benchmarks/common_crawl.py \
        --reps "${REPS:-3}" --json target/bench/common_crawl.json
else
    echo "skip Common Crawl: tests/data/commoncrawl-one is not present"
fi
