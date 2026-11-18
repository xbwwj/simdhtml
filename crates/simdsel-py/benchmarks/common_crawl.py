#!/usr/bin/env python3
"""Common Crawl benchmark on tests/data/commoncrawl-one, bucketed by document size.

Measures parse-only and cold `parse + all queries` per backend.  Every measured
iteration re-parses the file and re-runs the queries; no warm query cache.

Run:
    uv run --group bench python crates/simdsel-py/benchmarks/common_crawl.py
    uv run --group bench python crates/simdsel-py/benchmarks/common_crawl.py --reps 3 --json target/bench/common_crawl.json
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
import time
from dataclasses import dataclass
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]

from synthetic_corpora import (  # noqa: E402
    Backend,
    BACKENDS,
    LxmlBackend,
    ParselBackend,
    Query,
    SelectolaxBackend,
    SimdselBackend,
)

CORPUS = ROOT / "tests/data/commoncrawl-one"

QUERIES = (
    Query("a", "a"),
    Query("a-href", "a[href]"),
    Query("h2", "h2"),
    Query("a-attr", "a::attr(href)"),
    Query("div-a", "div a"),
)


@dataclass
class Doc:
    path: Path
    html: str
    size: int


def size_bucket(size: int) -> str:
    if size < 8 * 1024:
        return "<8KiB"
    if size < 32 * 1024:
        return "8-32KiB"
    if size < 128 * 1024:
        return "32-128KiB"
    return ">128KiB"


def load_docs(corpus: Path = CORPUS) -> tuple[list[Doc], int, int]:
    docs: list[Doc] = []
    skipped = 0
    total_bytes = 0
    for path in sorted(corpus.rglob("*.html")):
        raw = path.read_bytes()
        total_bytes += len(raw)
        try:
            html = raw.decode("utf-8")
        except UnicodeDecodeError:
            skipped += 1
            continue
        docs.append(Doc(path, html, len(raw)))
    return docs, skipped, total_bytes


def measure(
    backend: Backend,
    docs: list[Doc],
    queries: tuple[Query, ...],
    reps: int,
) -> tuple[float, float, int]:
    """Returns (best_seconds, median_seconds, matches)."""
    def once() -> tuple[float, int]:
        start = time.perf_counter()
        matches = 0
        for doc in docs:
            parsed = backend.parse(doc.html)
            for query in queries:
                matches += len(backend.run(parsed, query))
        return time.perf_counter() - start, matches

    once()  # warmup
    times: list[float] = []
    matches = 0
    for _ in range(reps):
        elapsed, matches = once()
        times.append(elapsed)
    return min(times), statistics.median(times), matches


def measure_parse_only(
    backend: Backend,
    docs: list[Doc],
    reps: int,
) -> tuple[float, float]:
    def once() -> float:
        start = time.perf_counter()
        for doc in docs:
            backend.parse(doc.html)
        return time.perf_counter() - start

    once()
    times = [once() for _ in range(reps)]
    return min(times), statistics.median(times)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--reps", type=int, default=3)
    parser.add_argument(
        "--backends",
        default="simdsel,parsel,lxml,selectolax",
        help="comma-separated backends",
    )
    parser.add_argument("--json", type=Path, default=None)
    parser.add_argument("--corpus", type=Path, default=CORPUS)
    args = parser.parse_args()

    docs, skipped, total_bytes = load_docs(args.corpus)
    buckets: dict[str, list[Doc]] = {
        "<8KiB": [],
        "8-32KiB": [],
        "32-128KiB": [],
        ">128KiB": [],
    }
    for doc in docs:
        buckets[size_bucket(doc.size)].append(doc)

    selected: list[Backend] = []
    for name in args.backends.split(","):
        name = name.strip()
        factory = BACKENDS.get(name)
        if factory is None:
            continue
        backend = factory()
        if backend.available:
            selected.append(backend)
        else:
            print(f"skip {name}: {backend.error}", file=sys.stderr)

    print(
        f"corpus: {len(docs)} utf8 docs, {total_bytes / 1e6:.2f} MB, "
        f"skipped non-utf8={skipped}, reps={args.reps}, queries={len(QUERIES)}"
    )
    print()
    print(
        f"{'bucket':>10} | {'docs':>4} | {'size MB':>7} | {'backend':>10} | "
        f"{'best ms':>9} | {'MB/s':>8} | {'docs/s':>8} | {'median ms':>9} | matches"
    )
    print("-" * 120)

    results = []
    for label, group in buckets.items():
        if not group:
            continue
        group_bytes = sum(doc.size for doc in group)
        for backend in selected:
            try:
                best, median = measure_parse_only(backend, group, args.reps)
                row = {
                    "bucket": label,
                    "docs": len(group),
                    "bytes": group_bytes,
                    "backend": backend.name,
                    "kind": "parse-only",
                    "best_ms": best * 1e3,
                    "median_ms": median * 1e3,
                    "mb_s": (group_bytes / 1e6) / best,
                    "docs_s": len(group) / best,
                    "matches": 0,
                }
                results.append(row)
                print(
                    f"{label:>10} | {len(group):4} | {group_bytes/1e6:7.2f} | "
                    f"{backend.name:>10} | {best*1e3:9.1f} | {row['mb_s']:8.1f} | "
                    f"{row['docs_s']:8.1f} | {median*1e3:9.1f} | parse-only"
                )
            except Exception as exc:
                print(
                    f"{label:>10} | {len(group):4} | {group_bytes/1e6:7.2f} | "
                    f"{backend.name:>10} | ERROR: {exc}",
                    file=sys.stderr,
                )

        for backend in selected:
            try:
                best, median, matches = measure(backend, group, QUERIES, args.reps)
            except Exception as exc:
                print(
                    f"{label:>10} | {len(group):4} | {group_bytes/1e6:7.2f} | "
                    f"{backend.name:>10} | ERROR: {exc}",
                    file=sys.stderr,
                )
                continue
            row = {
                "bucket": label,
                "docs": len(group),
                "bytes": group_bytes,
                "backend": backend.name,
                "best_ms": best * 1e3,
                "median_ms": median * 1e3,
                "mb_s": (group_bytes / 1e6) / best,
                "docs_s": len(group) / best,
                "matches": matches,
            }
            results.append(row)
            print(
                f"{label:>10} | {len(group):4} | {group_bytes/1e6:7.2f} | "
                f"{backend.name:>10} | {best*1e3:9.1f} | {row['mb_s']:8.1f} | "
                f"{row['docs_s']:8.1f} | {median*1e3:9.1f} | {matches}"
            )

    if args.json:
        args.json.write_text(json.dumps(results, indent=2), encoding="utf-8")
        print(f"\nwrote {args.json}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
