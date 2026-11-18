#!/usr/bin/env python3
"""Synthetic-corpora cold-path comparison harness.

Measures parse + first select for simdsel and (when installed) parsel / lxml /
selectolax.  Every measured iteration re-parses the document and re-runs the
query; no warm query cache is used.

Examples:

    uv run --group bench python crates/simdsel-py/benchmarks/synthetic_corpora.py
    uv run --group bench python crates/simdsel-py/benchmarks/synthetic_corpora.py --sizes 64KiB,512KiB,4MiB --reps 5
    uv run --group bench python crates/simdsel-py/benchmarks/synthetic_corpora.py --backends simdsel,parsel,lxml --json target/bench/synthetic_corpora.json

The script has no mandatory third-party dependencies.  Missing backends are
reported as unavailable and skipped.
"""

from __future__ import annotations

import argparse
import json
import multiprocessing as mp
import statistics
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterable, Optional

UNITS = {"b": 1, "kib": 1024, "mib": 1024**2, "kb": 1000, "mb": 1000**2}


def parse_size(text: str) -> int:
    text = text.strip().lower()
    for suffix, scale in sorted(UNITS.items(), key=lambda kv: -len(kv[0])):
        if text.endswith(suffix):
            return int(float(text[: -len(suffix)]) * scale)
    return int(text)


def size_label(num_bytes: int) -> str:
    if num_bytes >= 1024**2:
        return f"{num_bytes / 1024**2:.0f} MiB"
    return f"{num_bytes / 1024:.0f} KiB"


@dataclass(frozen=True)
class Query:
    name: str
    text: str


@dataclass(frozen=True)
class Corpus:
    name: str
    html: str
    queries: tuple[Query, ...]


def repeat_to(target: int, head: str, item: str, tail: str) -> str:
    parts = [head]
    size = len(head.encode()) + len(tail.encode())
    n = 0
    while size < target:
        parts.append(item.format(n=n))
        size += len(item.format(n=n).encode())
        n += 1
    parts.append(tail)
    return "".join(parts)


def make_corpora(sizes: Iterable[int]) -> list[Corpus]:
    corpora: list[Corpus] = []
    for target in sizes:
        cards = repeat_to(
            target,
            '<html><head></head><body><div id="list">',
            '<div class="item" data-id="{n}"><h2 class="title">Item &amp; {n}</h2>'
            '<a class="link" href="/item/{n}">link</a>'
            '<span class="price">${n}</span>'
            '<p class="desc">Description <b>with</b> markup</p></div>',
            "</div></body></html>",
        )
        corpora.append(
            Corpus(
                f"cards/{size_label(target)}",
                cards,
                (
                    Query("css-title-text", ".item .title::text"),
                    Query("css-href", ".item > a.link::attr(href)"),
                    Query("css-complex-child", "div.item > a.link[href^='/item/']::attr(href)"),
                    Query("css-not-class", ".item:not(.missing) .title::text"),
                    Query("css-attr-suffix", "[data-id$='7'] .price::text"),
                ),
            )
        )

        table = repeat_to(
            target,
            "<html><body><table><tbody>",
            "<tr class=\"row\"><td class=\"name\">{n}</td><td class=\"price\">${n}</td></tr>",
            "</tbody></table></body></html>",
        )
        corpora.append(
            Corpus(
                f"table/{size_label(target)}",
                table,
                (
                    Query("css-price", ".price::text"),
                    Query("css-odd-row", "tr:nth-child(2n+1)"),
                    Query("css-odd-price", "tr:nth-child(odd) td.price::text"),
                    Query("css-row-child", "tr.row > td.name::text"),
                ),
            )
        )

        listing = repeat_to(
            target,
            "<html><body><ul>",
            '<li class="item"><a href="/{n}">{n}</a></li>',
            "</ul></body></html>",
        )
        corpora.append(
            Corpus(
                f"list/{size_label(target)}",
                listing,
                (
                    Query("css-link-text", "li:nth-child(2n+1) a::text"),
                    Query("css-href", "li a::attr(href)"),
                    Query("css-link-attr", "li:nth-child(2n+1) a[href^='/']::attr(href)"),
                ),
            )
        )
    return corpora


@dataclass
class BackendResult:
    name: str
    available: bool
    error: str | None = None


class Backend:
    name = "base"

    def __init__(self) -> None:
        self.available = False
        self.error: str | None = None

    def parse(self, html: str) -> object:
        raise NotImplementedError

    def run(self, parsed: object, query: Query) -> list[str]:
        raise NotImplementedError


class SimdselBackend(Backend):
    name = "simdsel"

    def __init__(self) -> None:
        super().__init__()
        try:
            import simdsel  # type: ignore

            self.module = simdsel
            self.available = True
        except Exception as exc:  # pragma: no cover - depends on build
            self.error = str(exc)

    def parse(self, html: str) -> object:
        return self.module.Selector(text=html)

    def run(self, parsed: object, query: Query) -> list[str]:
        return parsed.css(query.text).getall()


class ParselBackend(Backend):
    name = "parsel"

    def __init__(self) -> None:
        super().__init__()
        try:
            import parsel  # type: ignore

            self.module = parsel
            self.available = True
        except Exception as exc:
            self.error = str(exc)

    def parse(self, html: str) -> object:
        return self.module.Selector(text=html)

    def run(self, parsed: object, query: Query) -> list[str]:
        return parsed.css(query.text).getall()


class LxmlBackend(Backend):
    name = "lxml"

    def __init__(self) -> None:
        super().__init__()
        try:
            from lxml import etree, html  # type: ignore

            self.etree = etree
            self.html = html
            self.available = True
        except Exception as exc:
            self.error = str(exc)

    def parse(self, html: str) -> object:
        if not html:
            return self.html.fromstring("<html></html>")
        try:
            return self.html.fromstring(html)
        except Exception:
            # libxml2 refuses a str with an encoding declaration; bytes are
            # the correct input for real-world documents with <meta charset>.
            try:
                return self.html.fromstring(html.encode("utf-8"))
            except Exception:
                return self.html.fromstring("<html></html>")

    def run(self, parsed: object, query: Query) -> list[str]:
        selector, pseudo = split_css_pseudo(query.text)
        values = parsed.cssselect(selector)
        out: list[str] = []
        for value in values:
            if isinstance(value, str):
                out.append(value)
            elif pseudo is not None:
                kind, name = pseudo
                if kind == "text":
                    out.append(value.text_content())
                else:
                    attr = value.get(name)
                    if attr is not None:
                        out.append(attr)
            else:
                out.append(self.etree.tostring(value, encoding="unicode", with_tail=False))
        return out


def split_css_pseudo(query: str) -> tuple[str, tuple[str, str] | None]:
    if "::attr(" in query and query.endswith(")"):
        base, _, rest = query.partition("::attr(")
        return base, ("attr", rest[:-1].strip())
    if query.endswith("::text"):
        return query[: -len("::text")], ("text", "")
    return query, None


class SelectolaxBackend(Backend):
    name = "selectolax"

    def __init__(self) -> None:
        super().__init__()
        try:
            # selectolax >= 1.0 deprecates the old modest backend.
            from selectolax.lexbor import LexborHTMLParser as parser  # type: ignore

            self.parser = parser
            self.available = True
        except Exception as exc:
            try:
                import selectolax.parser as parser  # type: ignore

                self.parser = parser.HTMLParser
                self.available = True
            except Exception:
                self.error = str(exc)

    def parse(self, html: str) -> object:
        return self.parser(html)

    def run(self, parsed: object, query: Query) -> list[str]:
        selector, pseudo = split_css_pseudo(query.text)
        nodes = parsed.css(selector)
        out: list[str] = []
        for node in nodes:
            if pseudo is None:
                out.append(node.html or "")
            else:
                kind, name = pseudo
                if kind == "text":
                    out.append(node.text(deep=False, strip=False))
                else:
                    value = node.attributes.get(name)
                    if value is not None:
                        out.append(value)
        return out


BACKENDS: dict[str, Callable[[], Backend]] = {
    "simdsel": SimdselBackend,
    "parsel": ParselBackend,
    "lxml": LxmlBackend,
    "selectolax": SelectolaxBackend,
}


@dataclass
class Measurement:
    backend: str
    corpus: str
    query: str
    kind: str
    bytes: int
    best_ms: Optional[float]
    median_ms: Optional[float]
    matches: int
    first: str
    status: str = "ok"


def measure(
    backend: Backend, corpus: Corpus, query: Query, reps: int
) -> Measurement:
    def once() -> tuple[float, list[str]]:
        start = time.perf_counter()
        parsed = backend.parse(corpus.html)
        values = backend.run(parsed, query)
        return (time.perf_counter() - start) * 1000.0, values

    # One warmup iteration keeps imports/lazy one-time work out of the sample.
    _, values = once()
    times: list[float] = []
    for _ in range(reps):
        elapsed, values = once()
        times.append(elapsed)

    return Measurement(
        backend=backend.name,
        corpus=corpus.name,
        query=query.name,
        kind="css",
        bytes=len(corpus.html.encode()),
        best_ms=min(times),
        median_ms=statistics.median(times),
        matches=len(values),
        first=values[0][:80] if values else "",
    )


def _measure_worker(backend, corpus, query, reps, queue) -> None:
    try:
        queue.put(measure(backend, corpus, query, reps))
    except Exception as exc:  # pragma: no cover - depends on backend
        queue.put({"error": str(exc)})


def measure_with_timeout(
    backend: Backend,
    corpus: Corpus,
    query: Query,
    reps: int,
    timeout_seconds: Optional[float],
) -> Measurement:
    if timeout_seconds is None or timeout_seconds <= 0:
        return measure(backend, corpus, query, reps)

    ctx = mp.get_context("fork")
    queue = ctx.Queue()
    proc = ctx.Process(
        target=_measure_worker,
        args=(backend, corpus, query, reps, queue),
    )
    proc.start()
    proc.join(timeout_seconds)
    if proc.is_alive():
        proc.terminate()
        proc.join()
        return Measurement(
            backend=backend.name,
            corpus=corpus.name,
            query=query.name,
            kind="css",
            bytes=len(corpus.html.encode()),
            best_ms=None,
            median_ms=None,
            matches=0,
            first="",
            status="timeout",
        )

    try:
        result = queue.get_nowait()
    except Exception:
        result = {"error": f"worker exited with code {proc.exitcode}"}
    if isinstance(result, dict):
        return Measurement(
            backend=backend.name,
            corpus=corpus.name,
            query=query.name,
            kind="css",
            bytes=len(corpus.html.encode()),
            best_ms=None,
            median_ms=None,
            matches=0,
            first="",
            status=f"error: {result.get('error', 'unknown')}",
        )
    return result


def measure_parse_only(backend: Backend, corpus: Corpus, reps: int) -> tuple[float, float]:
    def once() -> float:
        start = time.perf_counter()
        backend.parse(corpus.html)
        return (time.perf_counter() - start) * 1000.0

    once()  # warmup
    times = [once() for _ in range(reps)]
    return min(times), statistics.median(times)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--sizes",
        default="64KiB,512KiB,4MiB",
        help="comma-separated sizes (e.g. 64KiB,512KiB,4MiB)",
    )
    parser.add_argument(
        "--backends",
        default="simdsel,parsel,lxml,selectolax",
        help="comma-separated backends",
    )
    parser.add_argument("--reps", type=int, default=5)
    parser.add_argument(
        "--timeout-seconds",
        type=float,
        default=30.0,
        help="per-query timeout; 0 disables (default: 30s)",
    )
    parser.add_argument("--json", type=Path, default=None)
    args = parser.parse_args()

    sizes = [parse_size(part) for part in args.sizes.split(",") if part.strip()]
    corpora = make_corpora(sizes)

    selected: list[Backend] = []
    for name in (part.strip() for part in args.backends.split(",")):
        if not name:
            continue
        factory = BACKENDS.get(name)
        if factory is None:
            print(f"unknown backend: {name}", file=sys.stderr)
            continue
        backend = factory()
        if backend.available:
            selected.append(backend)
        else:
            print(f"skip {name}: {backend.error}", file=sys.stderr)

    if not selected:
        print("no backends available", file=sys.stderr)
        return 1

    print(
        f"{'backend':>11} | {'corpus':>18} | {'query':>20} | {'best ms':>9} | "
        f"{'MB/s':>8} | {'median ms':>9} | {'n':>7} | first"
    )
    print("-" * 130)

    results: list[Measurement] = []
    for corpus in corpora:
        for backend in selected:
            try:
                best_ms, median_ms = measure_parse_only(backend, corpus, args.reps)
            except Exception as exc:
                print(
                    f"{backend.name:>11} | {corpus.name:>18} | {'parse-only':>20} | ERROR: {exc}",
                    file=sys.stderr,
                )
                continue
            result = Measurement(
                backend=backend.name,
                corpus=corpus.name,
                query="parse-only",
                kind="parse",
                bytes=len(corpus.html.encode()),
                best_ms=best_ms,
                median_ms=median_ms,
                matches=0,
                first="",
            )
            results.append(result)
            mbs = (
                (result.bytes / 1e6) / (result.best_ms / 1e3)
                if result.best_ms
                else 0.0
            )
            print(
                f"{backend.name:>11} | {corpus.name:>18} | {result.query:>20} | "
                f"{(result.best_ms or 0):9.2f} | {mbs:8.2f} | "
                f"{(result.median_ms or 0):9.2f} | "
                f"{result.matches:7d} | {result.first!r}"
            )

        for query in corpus.queries:
            for backend in selected:
                try:
                    result = measure_with_timeout(
                        backend, corpus, query, args.reps, args.timeout_seconds
                    )
                except Exception as exc:
                    print(
                        f"{backend.name:>11} | {corpus.name:>18} | {query.name:>20} | ERROR: {exc}",
                        file=sys.stderr,
                    )
                    continue
                results.append(result)
                mbs = (
                    (result.bytes / 1e6) / (result.best_ms / 1e3)
                    if result.best_ms
                    else 0.0
                )
                print(
                    f"{backend.name:>11} | {corpus.name:>18} | {query.name:>20} | "
                    f"{(result.best_ms or 0):9.2f} | {mbs:8.2f} | "
                    f"{(result.median_ms or 0):9.2f} | "
                    f"{result.matches:7d} | {result.status:>10} | {result.first!r}"
                )

    if args.json:
        args.json.write_text(
            json.dumps([result.__dict__ for result in results], indent=2),
            encoding="utf-8",
        )
        print(f"\nwrote {args.json}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
