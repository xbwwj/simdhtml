#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

echo "== simdhtml classify =="
cargo bench -p simdhtml --bench classify

echo "== simdhtml raw scan =="
cargo bench -p simdhtml --bench raw --features bench-internal

if [[ -d tests/data/commoncrawl-one ]]; then
    echo "== simdhtml raw scan (common crawl) =="
    cargo bench -p simdhtml --bench raw --features bench-internal -- "$PWD/tests/data/commoncrawl-one"
fi

echo "== simdhtml stage1 =="
cargo bench -p simdhtml --bench stage1

echo "== simdsel cold parse+select =="
cargo bench -p simdsel --bench cold_parse_select
