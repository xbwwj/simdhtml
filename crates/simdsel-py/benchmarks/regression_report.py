#!/usr/bin/env python3
"""Compare two benchmark JSON files and write a Markdown regression report.

Usage:
    python crates/simdsel-py/benchmarks/regression_report.py \
        --old crates/simdsel-py/benchmarks/archive/pre-soa.json \
        --new target/bench/synthetic_corpora.json \
        --out target/bench/regression.md
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


def load(path: Path) -> dict[tuple[str, str, str], dict[str, Any]]:
    rows = json.loads(path.read_text(encoding="utf-8"))
    return {
        (r.get("backend", ""), r.get("corpus", ""), r.get("query", "")): r
        for r in rows
    }


def fmt_ms(value: Any) -> str:
    if value is None:
        return "-"
    return f"{value:.2f}"


def fmt_delta(old: Any, new: Any) -> str:
    if old is None or new is None or old == 0:
        return "-"
    return f"{(new - old) / old * 100:+.1f}%"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--old", type=Path, required=True)
    ap.add_argument("--new", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    old = load(args.old)
    new = load(args.new)

    lines = [
        "# Benchmark regression",
        "",
        f"- old: `{args.old}`",
        f"- new: `{args.new}`",
        "",
        "| backend | corpus | query | old best ms | new best ms | delta | status |",
        "|---|---|---|---:|---:|---:|---|",
    ]
    improved = regressed = timed_out = 0
    for key in sorted(set(old) | set(new)):
        o = old.get(key, {})
        n = new.get(key, {})
        ob = o.get("best_ms")
        nb = n.get("best_ms")
        status = n.get("status", "ok")
        if status == "timeout":
            timed_out += 1
        if ob is not None and nb is not None:
            if nb < ob:
                improved += 1
            elif nb > ob:
                regressed += 1
        lines.append(
            "| {} | {} | {} | {} | {} | {} | {} |".format(
                key[0],
                key[1],
                key[2],
                fmt_ms(ob),
                fmt_ms(nb),
                fmt_delta(ob, nb),
                status,
            )
        )

    lines += [
        "",
        "## Summary",
        "",
        f"- improved: {improved}",
        f"- regressed: {regressed}",
        f"- timeout: {timed_out}",
        "",
        "## Notes",
        "",
        "- `parse-only` rows separate parser cost from selection cost; "
        "`selector delta` is approximate because best-of runs are independent.",
        "- The new path stores tag/attribute structure in flat SoA arrays and "
        "uses per-query tag masks instead of rebuilding a node-wide tag index.",
        "- Some 512 KiB regressions are expected from the extra structural "
        "arrays and attribute spans; 4 MiB and tag/attribute-heavy queries "
        "should improve.",
        "- `timeout` means the case exceeded `--timeout-seconds`; it is not "
        "a correctness result.",
    ]
    args.out.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
