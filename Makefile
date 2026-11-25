# Data preparation, tests and benchmarks.
#
# Two layers, so local and remote commands are unambiguous:
#
#   make data          fetch the *raw* compressed inputs (WARC download, SWDE submodule)
#   make extract-data  derive the working set from them (7z x + index.tsv)
#   make bench-quick   run the benchmarks (never fetches or extracts implicitly)
#
# Benches are built once with `make bench-build`, which stages the executables
# under target/bench/bin; `make bench` only runs them.  Copying the whole tree
# (data, target, .venv) to another machine is therefore enough to run the
# benchmarks there: no cargo, no rustc, no network, no recompilation.

SHELL := bash
.SHELLFLAGS := -eu -o pipefail -c
.DELETE_ON_ERROR:
.DEFAULT_GOAL := help
MAKEFLAGS += --no-builtin-rules

# ---------------------------------------------------------------------------
# paths
# ---------------------------------------------------------------------------
DATA       := data
WARC_DIR   := $(DATA)/commoncrawl/warc
WARC_NAME  := CC-MAIN-20261005105623-20261005135623-00993.warc.gz
WARC       := $(WARC_DIR)/$(WARC_NAME)
WARC_URL   := https://data.commoncrawl.org/crawl-data/CC-MAIN-2026-43/segments/1791170053823.21/warc/$(WARC_NAME)
WARC_SIZE  := 958726164

SWDE       := $(DATA)/swde
SWDE_ARCS  := $(SWDE)/swde/sourceCode/sourceCode
WPT        := $(DATA)/wpt
WPT_RES    := $(WPT)/html/syntax/parsing/resources
SWDE_HTML  := $(DATA)/swde-html
SWDE_INDEX := $(DATA)/swde-index.tsv

# ---------------------------------------------------------------------------
# bench binaries: built here, run anywhere
# ---------------------------------------------------------------------------
BENCH          := cargo bench -p simdhtml-bench
BENCH_BIN      ?= target/bench/bin
BENCH_SOURCES  := Cargo.toml Cargo.lock bench/Cargo.toml bench/src bench/benches crates

FEATURES_markup         :=
FEATURES_parse          :=
FEATURES_classify       :=
FEATURES_select         :=
FEATURES_extract        := --features external
FEATURES_parse_compare  := --features external
FEATURES_python_compare := --features python

# `BENCH_BUILD=1` re-runs cargo before the benchmarks instead of using the
# staged binaries as-is.
BENCH_BUILD ?= 0

BENCH_TARGETS := markup parse classify select parse_compare extract python_compare
ARGS          ?=
QUICK_ARGS    := --quick --noplot
QUICK_TARGETS := markup parse select extract

# The embedded-CPython comparison needs the shipped venv to work on this host
# (same Python version and libpython); otherwise it is left out of the `bench`
# set.  `bench-compare` always keeps it, because the scoreboard needs its rows.
VENV_DIR ?= .venv
VENV_PY  := $(abspath $(VENV_DIR))/bin/python
PY_SITE := $(shell $(VENV_PY) -c 'import sysconfig; print(sysconfig.get_paths()["purelib"])' 2>/dev/null)
ifeq ($(strip $(PY_SITE)),)
PY_SITE       :=
BENCH_TARGETS := $(filter-out python_compare,$(BENCH_TARGETS))
endif
FULL_TARGETS := $(BENCH_TARGETS)

# quick-run sampling knobs (never applied to the full run)
SIMDHTML_BENCH_LIMIT ?= 60
SIMDHTML_BENCH_BYTES ?= 16777216

# the scoreboard: the two benches behind figures/*.svg (see perf_chart.py)
COMPARE_TARGETS ?= extract python_compare

# ---------------------------------------------------------------------------
# help / status
# ---------------------------------------------------------------------------
help:  ## list targets
	@echo "usage: make <target>"
	@echo
	@grep -hE '^[a-z][a-z0-9_-]*:.*?## ' $(MAKEFILE_LIST) \
		| awk -F':.*?## ' '{ printf "  %-16s %s\n", $$1, $$2 }'
	@echo
	@echo "bench bins: $(BENCH_BIN)"
	@echo "targets:    $(FULL_TARGETS)"

info:  ## show data, venv and prebuilt binaries
	@echo "bench bins:  $(BENCH_BIN)"
	@echo "targets:     $(FULL_TARGETS)"
	@echo "compare:     $(COMPARE_TARGETS) ($(SIMDHTML_BENCH_LIMIT) docs / $(shell echo $$(($(SIMDHTML_BENCH_BYTES) / 1048576))) MiB)"
	@printf 'venv:        %s %s\n' "$(VENV_DIR)" "$$( \
		$(VENV_PY) -c 'import sys, parsel, selectolax, bs4; print("python", sys.version.split()[0], "+ bench deps")' 2>/dev/null \
		|| echo 'missing or broken: run make venv (venv-cn / venv-ali)')"
	@echo "warc:        $(WARC) $(if $(wildcard $(WARC)),[ok],[missing: make data-cc])"
	@echo "swde source: $(SWDE_ARCS) $(if $(wildcard $(SWDE_ARCS)/auto.7z),[ok],[missing: make data-swde])"
	@echo "wpt:         $(WPT) $(if $(wildcard $(WPT_RES)/adoption01.dat),[ok],[missing: make data-wpt])"
	@echo "swde html:   $(SWDE_HTML) $(if $(wildcard $(SWDE_HTML)/auto),[ok],[missing: make extract-data])"
	@echo "swde index:  $(SWDE_INDEX) $(if $(wildcard $(SWDE_INDEX)),[ok],[missing: make extract-data])"
	@for t in $(BENCH_TARGETS); do \
		printf '  %-14s %s\n' "$$t" \
			"$$(test -x $(BENCH_BIN)/$$t && echo prebuilt || echo 'missing: make bench-build')"; \
	done

# ---------------------------------------------------------------------------
# raw data: network / submodule only
# ---------------------------------------------------------------------------
data: $(WARC) $(SWDE_ARCS)/auto.7z  ## fetch the raw compressed inputs
data-cc: $(WARC)  ## only the Common Crawl WARC
data-swde: $(SWDE_ARCS)/auto.7z  ## only the SWDE submodule
data-wpt: $(WPT_RES)/adoption01.dat  ## only the WPT submodule (tree-construction tests)

$(WARC):
	@mkdir -p $(WARC_DIR)
	curl -fL -C - --retry 6 --retry-delay 2 -o $@.part $(WARC_URL)
	@test "$$(stat -c%s $@.part)" = "$(WARC_SIZE)" \
		|| { echo "size mismatch: expected $(WARC_SIZE)"; exit 1; }
	mv $@.part $@

$(SWDE_ARCS)/auto.7z:
	git submodule update --init --depth 1 $(SWDE)

$(WPT_RES)/adoption01.dat:
	git submodule update --init --depth 1 $(WPT)

# ---------------------------------------------------------------------------
# working set: unpack the raw inputs (7z is a CLI dependency, once)
# ---------------------------------------------------------------------------
extract-data: $(SWDE_INDEX)  ## unpack SWDE and build the document index
extract-swde: $(SWDE_INDEX)

$(SWDE_HTML)/.stamp: $(SWDE_ARCS)/auto.7z
	@command -v 7z >/dev/null || { echo "7z not found: install p7zip-full"; exit 1; }
	rm -rf $(SWDE_HTML).tmp
	7z x -y -o$(SWDE_HTML).tmp $(SWDE_ARCS)/'*.7z' >/dev/null
	mv $(SWDE_HTML).tmp $(SWDE_HTML)
	touch $@

$(SWDE_INDEX): $(SWDE_HTML)/.stamp
	find $(SWDE_HTML) -name '*.htm' -printf '%s\t%P\n' | LC_ALL=C sort -k2 > $@.tmp
	mv $@.tmp $@

# ---------------------------------------------------------------------------
# guards: benchmarks never download or extract behind your back
# ---------------------------------------------------------------------------
require-data:
	@test -f $(WARC) || { echo "missing $(WARC): run 'make data-cc'"; exit 1; }
require-extract:
	@test -d $(SWDE_HTML) && test -f $(SWDE_INDEX) \
		|| { echo "missing SWDE working set: run 'make extract-data'"; exit 1; }

# ---------------------------------------------------------------------------
# venv: the bench Python deps, installed with pip (no uv, no lockfile)
#
#   make venv       PyPI
#   make venv-cn    Tsinghua mirror
#   make venv-ali   Aliyun mirror
#
# The venv is recreated by default: a stale one is exactly what breaks when the
# machine's Python differs from the one that built it.  `VENV_REUSE=1` keeps the
# existing venv and only installs the missing packages.
# ---------------------------------------------------------------------------
VENV_PYTHON    ?= python3
VENV_REUSE     ?= 0
VENV_CHECK     ?= import parsel, selectolax, bs4
# keep in sync with pyproject.toml [dependency-groups] bench
BENCH_PY_DEPS  ?= 'parsel>=1.10' 'selectolax>=1.0' 'beautifulsoup4>=4.12'
PIP_ARGS       ?=

venv:  ## .venv with parsel/selectolax/bs4, pip from PyPI (recreates .venv)
venv-cn:  ## same, pip from the Tsinghua mirror (recreates .venv)
venv-ali:  ## same, pip from the Aliyun mirror (recreates .venv)
venv-cn: PIP_ARGS := -i https://pypi.tuna.tsinghua.edu.cn/simple
venv-ali: PIP_ARGS := -i http://mirrors.cloud.aliyuncs.com/pypi/simple/ \
	--trusted-host mirrors.cloud.aliyuncs.com

venv venv-cn venv-ali:
	@echo "[venv] $(VENV_DIR) from $(if $(PIP_ARGS),$(PIP_ARGS),default PyPI)"
	@if [ "$(VENV_REUSE)" != 1 ]; then rm -rf $(VENV_DIR); fi
	$(VENV_PYTHON) -m venv $(VENV_DIR) \
		|| { echo "$(VENV_PYTHON) cannot create a venv: install python3-venv / a matching libpython"; exit 1; }
	$(VENV_PY) -m pip install --disable-pip-version-check $(PIP_ARGS) $(BENCH_PY_DEPS)
	@$(VENV_PY) -c '$(VENV_CHECK)' \
		|| { echo "bench deps are not importable in $(VENV_DIR)"; exit 1; }
	@$(VENV_PY) -c 'import sys; print("[venv] python", sys.version.split()[0], "ready")'

# ---------------------------------------------------------------------------
# benchmarks: `bench-build` stages the executables, `bench` just runs them
# ---------------------------------------------------------------------------
bench: require-data require-extract $(if $(filter 1,$(BENCH_BUILD)),bench-build)  ## full criterion run
	@for t in $(FULL_TARGETS); do \
		$(MAKE) --no-print-directory bench-$$t ARGS="$(ARGS)" || exit $$?; \
	done

bench-quick: require-data require-extract $(if $(filter 1,$(BENCH_BUILD)),bench-build)  ## sampled run for iteration (no plots)
	@for t in $(QUICK_TARGETS); do \
		SIMDHTML_BENCH_LIMIT=$(or $(SIMDHTML_BENCH_LIMIT),60) \
		SIMDHTML_BENCH_BYTES=$(or $(SIMDHTML_BENCH_BYTES),16777216) \
		$(MAKE) --no-print-directory bench-$$t ARGS="$(QUICK_ARGS)" || exit $$?; \
	done

# The scoreboard run: `extract` (simdsel vs scraper/dom_query) and
# `python_compare` (parsel/selectolax/beautifulsoup4) are the only benches
# behind figures/*.svg.  The sample is pinned to the chart口径 (60 docs /
# 16 MiB) because bs4 costs seconds per iteration and would dominate a full
# 500-document run; raise SIMDHTML_BENCH_LIMIT to trade time for precision.
bench-compare: require-data require-extract $(if $(filter 1,$(BENCH_BUILD)),bench-build)  ## scoreboard run (extract + python) on the chart sample
	@for t in $(COMPARE_TARGETS); do \
		SIMDHTML_BENCH_LIMIT=$(SIMDHTML_BENCH_LIMIT) \
		SIMDHTML_BENCH_BYTES=$(SIMDHTML_BENCH_BYTES) \
		$(MAKE) --no-print-directory bench-$$t ARGS="$(ARGS)" || exit $$?; \
	done

bench-python_compare: export PYO3_PYTHON = $(VENV_PY)
bench-python_compare: export PYTHONPATH = $(PY_SITE)

bench-build: export PYO3_PYTHON = $(VENV_PY)
bench-build: export PYTHONPATH = $(PY_SITE)
bench-build:  ## build the bench executables into target/bench/bin (cargo, once)
	@for t in $(BENCH_TARGETS); do \
		$(MAKE) --no-print-directory build-bench-$$t || exit $$?; \
	done
	@du -sh $(BENCH_BIN)

# Separate targets so `$(FEATURES_$*)` is expanded with the real benchmark
# name; inside the shell loop `$(FEATURES_$$t)` would not expand.
build-bench-%: export PYO3_PYTHON = $(VENV_PY)
build-bench-%: export PYTHONPATH = $(PY_SITE)
build-bench-%:
	@echo "== build $* =="
	@mkdir -p $(BENCH_BIN)
	$(BENCH) --no-run --bench $* $(FEATURES_$*) --message-format=json \
		| sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' \
		| while read -r exe; do install -m 755 "$$exe" "$(BENCH_BIN)/$*"; done
	@test -x "$(BENCH_BIN)/$*" || { echo "no executable for $*"; exit 1; }

# Binaries are staged by name so a copied tree needs neither cargo nor the
# registry; the warning catches results measured from stale executables.
# `--bench` is what `cargo bench` would have passed; without it criterion runs
# the process once without measuring.  CARGO_TARGET_DIR pins the result
# directory to target/criterion, which perf_chart.py reads.
bench-%:
	@test -x "$(BENCH_BIN)/$*" \
		|| { echo "no $(BENCH_BIN)/$*: run 'make bench-build' on the build machine"; exit 1; }
	@test -z "$$(find $(BENCH_SOURCES) -type f -newer $(BENCH_BIN)/$* -print -quit)" \
		|| echo "warning: sources are newer than $(BENCH_BIN)/$* (run 'make bench-build')"
	@echo "== bench $* =="
	CARGO_TARGET_DIR=$(abspath target) "$(BENCH_BIN)/$*" --bench $(ARGS)

# ---------------------------------------------------------------------------
# tests
# ---------------------------------------------------------------------------
test:  ## Rust tests
	cargo test -p simdhtml -p simdsel --no-fail-fast
test-wpt: $(WPT_RES)/adoption01.dat  ## html5lib tree-construction conformance
	cargo test -p simdhtml --test wpt_tree_construction -- --nocapture
test-corpus: require-data  ## Common Crawl preorder/invariant test
	SIMDHTML_RUN_CORPUS_TESTS=1 cargo test -p simdhtml --test mode_agreement -- --nocapture
diff: require-data  ## parsel/lxml differential harness (builds the PyO3 extension)
	uv sync --group bench
	uv run --no-sync --group bench python scripts/css_diff.py
	uv run --no-sync --group bench python scripts/parsel_api_diff.py
	mkdir -p target/bench
	cargo run --release --quiet -p simdsel --example xpath -- cc scripts/xpath_exprs.txt 40 >target/bench/xpath.jsonl
	uv run --no-sync --group bench python scripts/xpath_diff.py target/bench/xpath.jsonl

# ---------------------------------------------------------------------------
# housekeeping
# ---------------------------------------------------------------------------
check:  ## cargo fmt --check + cargo clippy
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets
fmt:  ## cargo fmt
	cargo fmt --all
clippy:  ## cargo clippy
	cargo clippy --workspace --all-targets
clean-extract:  ## remove the derived SWDE tree and index
	rm -rf $(SWDE_HTML) $(SWDE_INDEX)
clean-data:  ## remove the raw inputs
	rm -rf $(WARC_DIR)
	-git submodule deinit -f $(SWDE)
	-git submodule deinit -f $(WPT)
clean: clean-extract  ## cargo clean + derived data + staged binaries
	rm -rf $(BENCH_BIN)
	cargo clean

.PHONY: help info data data-cc data-swde data-wpt extract-data extract-swde \
	require-data require-extract bench bench-quick bench-compare bench-build \
	venv venv-cn venv-ali \
	test test-wpt test-corpus diff check fmt clippy \
	clean-extract clean-data clean
