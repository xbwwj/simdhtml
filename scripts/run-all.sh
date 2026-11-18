#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

./scripts/check.sh

echo "== uv sync =="
uv sync --group bench

echo "== python tests =="
uv run --group bench python -m unittest discover -s crates/simdsel-py/python/tests -v

echo "== done =="
echo "For full benchmarks, run: scripts/bench-full.sh"
