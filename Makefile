# Data preparation, tests and benchmarks.
#
# Two layers, so local and remote commands are unambiguous:
#
#   make data          fetch the *raw* compressed inputs (WARC download, SWDE submodule)
#   make extract-data  derive the working set from them (7z x + index.tsv)
#   make bench-quick   run the benchmarks (never fetches or extracts implicitly)
#
# The same file serves a source checkout and a prebuilt bundle: when `bin/`
# holds bench binaries, everything runs from those and cargo is not touched.

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
# run mode: prebuilt bundle vs source checkout
# ---------------------------------------------------------------------------
BENCH_BIN_DIR ?= bin
BENCH_MODE := $(if $(wildcard $(BENCH_BIN_DIR)/parse),bundle,source)

BENCH := cargo bench -p simdhtml-bench
FEATURES_markup         :=
FEATURES_parse          :=
FEATURES_classify       :=
FEATURES_select         :=
FEATURES_extract        := --features external
FEATURES_parse_compare  := --features external
FEATURES_python_compare := --features python

ARGS       ?=
QUICK_ARGS := --quick --noplot
QUICK_TARGETS := markup parse select extract
FULL_TARGETS  := markup parse classify select parse_compare extract python_compare
ifeq ($(BENCH_MODE),bundle)
# The bundle ships no CPython, so the python comparison is source-only.
FULL_TARGETS := $(filter-out python_compare,$(FULL_TARGETS))
endif

# embedded CPython for the python comparison (source only; the bundle has none)
VENV_PY := $(CURDIR)/.venv/bin/python
ifeq ($(wildcard $(VENV_PY)),)
PY_SITE :=
else
PY_SITE := $(shell $(VENV_PY) -c 'import sysconfig; print(sysconfig.get_paths()["purelib"])')
endif

# quick-run sampling knobs (never applied to the full run)
SIMDHTML_BENCH_LIMIT ?= 60
SIMDHTML_BENCH_BYTES ?= 16777216

# ---------------------------------------------------------------------------
# help / status
# ---------------------------------------------------------------------------
help:  ## list targets
	@echo "usage: make <target>"
	@echo
	@grep -hE '^[a-z][a-z0-9_-]*:.*?## ' $(MAKEFILE_LIST) \
		| awk -F':.*?## ' '{ printf "  %-16s %s\n", $$1, $$2 }'
	@echo
	@echo "mode: $(BENCH_MODE) (override with BENCH_BIN_DIR=...)"

info:  ## show mode, data and prebuilt binaries
	@echo "mode:        $(BENCH_MODE)"
	@echo "warc:        $(WARC) $(if $(wildcard $(WARC)),[ok],[missing: make data-cc])"
	@echo "swde source: $(SWDE_ARCS) $(if $(wildcard $(SWDE_ARCS)/auto.7z),[ok],[missing: make data-swde])"
	@echo "wpt:         $(WPT) $(if $(wildcard $(WPT_RES)/adoption01.dat),[ok],[missing: make data-wpt])"
	@echo "swde html:   $(SWDE_HTML) $(if $(wildcard $(SWDE_HTML)/auto),[ok],[missing: make extract-data])"
	@echo "swde index:  $(SWDE_INDEX) $(if $(wildcard $(SWDE_INDEX)),[ok],[missing: make extract-data])"
	@echo "bench bins:  $(BENCH_BIN_DIR) $(if $(wildcard $(BENCH_BIN_DIR)/parse),[prebuilt],[built from source])"

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
# benchmarks
# ---------------------------------------------------------------------------
bench: require-data require-extract  ## full criterion run
	@for t in $(FULL_TARGETS); do \
		$(MAKE) --no-print-directory bench-$$t ARGS="$(ARGS)" || exit $$?; \
	done

bench-quick: require-data require-extract  ## sampled run for iteration (no plots)
	@for t in $(QUICK_TARGETS); do \
		SIMDHTML_BENCH_LIMIT=$(or $(SIMDHTML_BENCH_LIMIT),60) \
		SIMDHTML_BENCH_BYTES=$(or $(SIMDHTML_BENCH_BYTES),16777216) \
		$(MAKE) --no-print-directory bench-$$t ARGS="$(QUICK_ARGS)" || exit $$?; \
	done

bench-python_compare: export PYO3_PYTHON = $(VENV_PY)
bench-python_compare: export PYTHONPATH = $(PY_SITE)

bench-build:  ## build every bench target once (source mode)
	cargo bench -p simdhtml-bench --no-run
	cargo bench -p simdhtml-bench --no-run --features external
	cargo bench -p simdhtml-bench --no-run --features python

ifeq ($(BENCH_MODE),bundle)
bench-%:
	@echo "== bench $* =="
	@test -x "$(BENCH_BIN_DIR)/$*" \
		|| { echo "bundle has no $(BENCH_BIN_DIR)/$*; rebuild it with 'make bundle'"; exit 1; }
	"$(BENCH_BIN_DIR)/$*" $(ARGS)
else
bench-%:
	$(BENCH) --bench $* $(FEATURES_$*) -- $(ARGS)
endif

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
# bundle: prebuilt binaries + raw data + this Makefile
# ---------------------------------------------------------------------------
BUNDLE_OUT     ?= target/bundle
BUNDLE_TARGETS := markup parse classify select parse_compare extract
BUNDLE_STAMP   := $(shell date +%Y%m%d-%H%M%S)

bundle: require-data  ## portable tarball for a machine without a Rust toolchain
	@test "$(BENCH_MODE)" = source \
		|| { echo "build the bundle from a source checkout, not from one"; exit 1; }
	@mkdir -p $(BUNDLE_OUT)
	cargo bench -p simdhtml-bench --no-run --features external --message-format=json \
		| sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' >$(BUNDLE_OUT)/executables.txt
	@stage="$(BUNDLE_OUT)/simdhtml-bundle-$(BUNDLE_STAMP)"; \
	rm -rf "$$stage"; \
	mkdir -p "$$stage/bin" "$$stage/data"; \
	while read -r exe; do \
		name="$$(basename "$$exe" | sed 's/-[0-9a-f]*$$//')"; \
		case " $(BUNDLE_TARGETS) " in *" $$name "*) cp "$$exe" "$$stage/bin/$$name" ;; esac; \
	done <$(BUNDLE_OUT)/executables.txt; \
	for t in $(BUNDLE_TARGETS); do \
		test -x "$$stage/bin/$$t" || { echo "missing $$t binary"; exit 1; }; \
	done; \
	cp Makefile "$$stage/"; \
	mkdir -p "$$stage/bench"; \
	cp -r bench/selectors "$$stage/bench/"; \
	mkdir -p "$$stage/data/commoncrawl/warc"; \
	cp $(WARC) "$$stage/data/commoncrawl/warc/"; \
	mkdir -p "$$stage/data/swde/swde/sourceCode"; \
	cp -r $(SWDE_ARCS) "$$stage/data/swde/swde/sourceCode/"; \
	tar -C $(BUNDLE_OUT) -cf "$(BUNDLE_OUT)/simdhtml-bundle-$(BUNDLE_STAMP).tar" \
		"simdhtml-bundle-$(BUNDLE_STAMP)"; \
	echo "[bundle] $(BUNDLE_OUT)/simdhtml-bundle-$(BUNDLE_STAMP).tar"; \
	du -h "$(BUNDLE_OUT)/simdhtml-bundle-$(BUNDLE_STAMP).tar"

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
clean: clean-extract  ## cargo clean + derived data
	cargo clean

.PHONY: help info data data-cc data-swde data-wpt extract-data extract-swde \
	require-data require-extract bench bench-quick bench-build \
	test test-wpt test-corpus diff bundle check fmt clippy \
	clean-extract clean-data clean
