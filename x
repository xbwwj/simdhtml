#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")"

BUNDLE_ROOT=""
if [[ -f "$PWD/BUNDLE.env" ]]; then
    export BUNDLE_ROOT="$PWD"
    # shellcheck disable=SC1091
    source "$PWD/BUNDLE.env"
fi

WARC_URL="https://data.commoncrawl.org/crawl-data/CC-MAIN-2026-43/segments/1791170053823.21/warc/CC-MAIN-20261005105623-20261005135623-00993.warc.gz"
WARC_SIZE=958726164
WARC_DIR=data/commoncrawl/warc
HTML_DIR=data/commoncrawl/html

usage() {
    cat <<'MSG'
usage: ./x <command> [args]

data            download/extract Common Crawl WARC + prepare SWDE + manifests
data-cc         only download and extract the Common Crawl WARC
data-swde       only prepare SWDE documents/labels and the benchmark manifests
test            Rust tests + PyO3 binding smoke test
test-wpt        WPT tree-construction conformance
test-corpus     Common Crawl preorder/invariant test
xpath-diff      XPath differential test against lxml
css-diff        CSS differential test against parsel (scripts/css_diff.py)
parsel-api-diff parsel-shaped API differential test (scripts/parsel_api_diff.py)
check           cargo fmt --check + cargo clippy
fmt             cargo fmt
clippy          cargo clippy
bench           full criterion run (simdhtml/simdsel + external + Python)
bench-quick     smaller/faster criterion run for iteration
bench-targets   list benchmark targets
bundle          build a portable bench bundle (binaries + data + Python)
                  options: --data sample|full|none --cc-docs N --swde-docs N
                           --format zst|gz|none --with-warc --out DIR

Examples:
  ./x data
  ./x test-wpt
  ./x bench
  ./x bench-quick
  X_BENCH_TARGETS='extract python_compare' ./x bench-quick
  ./x bench 'markup/swde/camera'
  ./x bundle --data sample
MSG
}

sync_bench() {
    uv sync --group dev --group bench
    if [[ -x .venv/bin/python ]]; then
        export PYO3_PYTHON="$PWD/.venv/bin/python"
        local site
        site="$(uv run --group dev --group bench python -c 'import sysconfig; print(sysconfig.get_paths()["purelib"])')"
        export PYTHONPATH="$site${PYTHONPATH:+:$PYTHONPATH}"
    fi
}

download_warc() {
    mkdir -p "$WARC_DIR"
    local name dest
    name="${WARC_URL##*/}"
    dest="$WARC_DIR/$name"
    if [[ -f "$dest" ]]; then
        local size
        size="$(stat -c%s "$dest")"
        if [[ "$size" == "$WARC_SIZE" ]]; then
            echo "[skip] WARC already downloaded: $dest"
            return
        fi
        echo "[warn] $dest is $size bytes, expected $WARC_SIZE; resuming"
    fi
    local part="$dest.part"
    if [[ -f "$part" && "$(stat -c%s "$part")" -gt "$WARC_SIZE" ]]; then
        rm -f "$part"
    fi
    echo "[download] $WARC_URL"
    curl -fL -C - --retry 6 --retry-delay 2 -o "$part" "$WARC_URL"
    local size
    size="$(stat -c%s "$part")"
    if [[ "$size" != "$WARC_SIZE" ]]; then
        echo "[error] downloaded $size bytes, expected $WARC_SIZE" >&2
        return 1
    fi
    mv "$part" "$dest"
}

extract_warc() {
    local force="${1:-}"
    local warc
    warc="$(find "$WARC_DIR" -maxdepth 1 -name '*.warc.gz' | sort | head -n1)"
    if [[ -z "$warc" ]]; then
        echo "[error] no .warc.gz under $WARC_DIR; run ./x data-cc" >&2
        return 1
    fi
    local args=(--warc "$warc" --out "$HTML_DIR")
    [[ "$force" == "--force" ]] && args+=(--force)
    cargo run --release -p simdhtml-bench --bin cc-export -- "${args[@]}"
}

cmd_data_cc() {
    download_warc
    extract_warc "${1:-}"
}

cmd_data_swde() {
    uv run python scripts/prepare_data.py
    uv sync --group bench
    uv run --group bench python scripts/make_swde_selectors.py
    uv run python scripts/prepare_data.py --manifests-only
}

cmd_data() {
    local force=""
    [[ "${1:-}" == "--force" ]] && force="--force"
    cmd_data_cc "$force"
    cmd_data_swde
}

cmd_test() {
    cargo test -p simdhtml -p simdsel --no-fail-fast
    if [[ -x .venv/bin/python ]]; then
        sync_bench
        uv run --group dev --group bench maturin develop --release
        cargo test -p simdhtml-bench --features python --test python_binding
    fi
}

cmd_test_wpt() {
    cargo test -p simdhtml --test wpt_tree_construction -- --nocapture
}

cmd_test_corpus() {
    SIMDHTML_RUN_CORPUS_TESTS=1 cargo test -p simdhtml --test mode_agreement -- --nocapture
}

cmd_css_diff() {
    uv sync --group bench
    uv run --group bench python scripts/css_diff.py
}

cmd_parsel_api_diff() {
    uv sync --group bench
    uv run --group bench python scripts/parsel_api_diff.py
}

cmd_xpath_diff() {
    uv sync --group bench
    mkdir -p target/bench
    cargo run --release -p simdsel --example xpath -- \
        data/commoncrawl/html scripts/xpath_exprs.txt 40 > target/bench/xpath.jsonl
    uv run --group bench python scripts/xpath_diff.py target/bench/xpath.jsonl
}

criterion_targets() {
    local quick="$1"
    if [[ -n "${X_BENCH_TARGETS:-}" ]]; then
        # shellcheck disable=SC2206
        printf '%s\n' ${X_BENCH_TARGETS}
        return
    fi
    if [[ "$quick" == "1" ]]; then
        printf '%s\n' markup parse select extract python_compare
    else
        printf '%s\n' markup parse classify select parse_compare extract python_compare
    fi
}

run_bundle_bench() {
    local quick="$1"
    shift || true
    local targets=()
    while IFS= read -r target; do
        targets+=("$target")
    done < <(criterion_targets "$quick")

    local extra=()
    if [[ "$quick" == "1" ]]; then
        extra=(--quick)
        export SIMDHTML_BENCH_LIMIT="${SIMDHTML_BENCH_LIMIT:-60}"
        export SIMDHTML_BENCH_BYTES="${SIMDHTML_BENCH_BYTES:-$((16 * 1024 * 1024))}"
    fi

    for target in "${targets[@]}"; do
        local bin="$BUNDLE_ROOT/bin/$target"
        if [[ ! -x "$bin" ]]; then
            echo "[skip] $target: no bundled binary" >&2
            continue
        fi
        echo "== bench ${target} (bundle) =="
        "$bin" --bench "${extra[@]}" "$@"
    done
}

run_bench() {
    local quick="$1"
    shift || true
    if [[ "${BUNDLE:-0}" == "1" && "${X_FORCE_SOURCE:-0}" != "1" ]]; then
        run_bundle_bench "$quick" "$@"
        return
    fi
    sync_bench

    local targets=()
    while IFS= read -r target; do
        targets+=("$target")
    done < <(criterion_targets "$quick")

    local features=(external python)
    local feature_arg=()
    feature_arg=(--features "$(IFS=,; echo "${features[*]}")")

    local extra=()
    if [[ "$quick" == "1" ]]; then
        extra=(--quick)
        export SIMDHTML_BENCH_LIMIT="${SIMDHTML_BENCH_LIMIT:-60}"
        export SIMDHTML_BENCH_BYTES="${SIMDHTML_BENCH_BYTES:-$((16 * 1024 * 1024))}"
    fi

    for target in "${targets[@]}"; do
        echo "== bench ${target} =="
        cargo bench -p simdhtml-bench --bench "$target" "${feature_arg[@]}" -- "${extra[@]}" "$@"
    done
}


cmd_bundle() {
    local data="sample"
    local cc_docs=500
    local swde_docs=500
    local format="zst"
    local with_warc=0
    local out="target/bundle"

    while (($#)); do
        case "$1" in
            --data) data="$2"; shift 2 ;;
            --cc-docs) cc_docs="$2"; shift 2 ;;
            --swde-docs) swde_docs="$2"; shift 2 ;;
            --format) format="$2"; shift 2 ;;
            --with-warc) with_warc=1; shift ;;
            --out) out="$2"; shift 2 ;;
            *) echo "bundle: unknown option $1" >&2; return 2 ;;
        esac
    done
    case "$data" in sample|full|none) ;; *) echo "bundle: --data must be sample|full|none" >&2; return 2 ;; esac
    case "$format" in zst|gz|none) ;; *) echo "bundle: --format must be zst|gz|none" >&2; return 2 ;; esac

    sync_bench
    echo "== build bench binaries =="
    PYO3_PYTHON="${PYO3_PYTHON:-$PWD/.venv/bin/python}"         cargo bench -p simdhtml-bench --features external,python --no-run
    cargo build --release -p simdhtml-bench --bin cc-export

    local stamp bundle_dir
    stamp="$(date +%Y%m%d-%H%M%S)"
    bundle_dir="$out/simdhtml-bundle-$stamp"
    mkdir -p "$out"
    rm -rf "$bundle_dir"

    local data_args=(--data "$data" --cc-docs "$cc_docs" --swde-docs "$swde_docs")
    [[ "$with_warc" == "1" ]] && data_args+=(--with-warc)
    python3 scripts/bundle.py --out "$bundle_dir" "${data_args[@]}"

    local base archive
    base="$(basename "$bundle_dir")"
    case "$format" in
        zst)
            archive="${bundle_dir}.tar.zst"
            if command -v zstd >/dev/null 2>&1; then
                tar -C "$out" -cf - "$base" | zstd -T0 -19 -o "$archive"
            else
                echo "[warn] zstd not found; falling back to .tar.gz" >&2
                archive="${bundle_dir}.tar.gz"
                tar -C "$out" -czf "$archive" "$base"
            fi
            ;;
        gz)
            archive="${bundle_dir}.tar.gz"
            tar -C "$out" -czf "$archive" "$base"
            ;;
        none)
            archive="${bundle_dir}.tar"
            tar -C "$out" -cf "$archive" "$base"
            ;;
    esac

    echo "[bundle] archive: $archive"
    du -h "$archive"
    echo "upload: scp '$archive' <server>:"
    echo "unpack: tar -xf '$(basename "$archive")' && cd '$base' && ./x bench"
}

cmd="${1:-help}"
shift || true
case "$cmd" in
    data) cmd_data "$@" ;;
    data-cc) cmd_data_cc "$@" ;;
    data-swde) cmd_data_swde ;;
    test) cmd_test ;;
    test-wpt) cmd_test_wpt ;;
    test-corpus) cmd_test_corpus ;;
    xpath-diff) cmd_xpath_diff ;;
    css-diff) cmd_css_diff ;;
    parsel-api-diff) cmd_parsel_api_diff ;;
    check)
        cargo fmt --all -- --check
        cargo clippy --workspace --all-targets
        ;;
    fmt) cargo fmt --all ;;
    clippy) cargo clippy --workspace --all-targets ;;
    bench) run_bench 0 "$@" ;;
    bench-quick) run_bench 1 "$@" ;;
    bench-targets) criterion_targets 0 ;;
    bundle) cmd_bundle "$@" ;;
    help|-h|--help) usage ;;
    *) echo "unknown command: $cmd" >&2; usage; exit 2 ;;
esac
