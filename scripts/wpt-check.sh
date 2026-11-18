#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

cargo test -p simdhtml --test wpt_tree_construction -- --ignored --nocapture
