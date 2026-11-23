#!/usr/bin/env python3
"""Normalise the benchmark corpora under data/ into data/prepared/*.

The benchmarks want one shape: a directory (or jsonl) of *raw bytes* per
document, next to a label file.  Nothing is transcoded here - the parser is
supposed to sniff and decode (encoding_rs), and measuring that is part of the
job - so documents keep the bytes they had on the wire.

Layout produced:

  data/prepared/swde/<vertical>/<site>/<id>.htm
  data/prepared/swde/labels.jsonl        {vertical, site, attr, pages: {id: [v]}}
  data/prepared/MANIFEST.json            per-dataset shape statistics
  data/prepared/manifests/{cc,swde,wpt}.jsonl

Usage (normally through the project entry point):
  ./x data-swde
  ./x data
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "data"
OUT = DATA / "prepared"

MANIFESTS = OUT / "manifests"
CC_HTML = DATA / "commoncrawl" / "html"
WORKLOAD = OUT / "workload"

SWDE_VERTICALS = [
    "auto",
    "book",
    "camera",
    "job",
    "movie",
    "nbaplayer",
    "restaurant",
    "university",
]

CHARSET_RE = re.compile(rb"""charset\s*=\s*["']?\s*([A-Za-z0-9_.:-]+)""", re.I)


def log(msg: str) -> None:
    print(msg, flush=True)


# ---------------------------------------------------------------------------
# document statistics
# ---------------------------------------------------------------------------


def declared_charset(head: bytes) -> str:
    """Best-effort `<meta charset>` / `content=` scan of the document head."""
    if head.startswith(b"\xef\xbb\xbf"):
        return "utf-8(bom)"
    if head.startswith((b"\xff\xfe", b"\xfe\xff")):
        return "utf-16(bom)"
    m = CHARSET_RE.search(head[:4096])
    if not m:
        return "none"
    return m.group(1).decode("ascii", "replace").lower()


def utf8_share(docs: list[Path], every: int = 1) -> dict:
    """How many documents are valid UTF-8 as they sit on disk."""
    ok = bad = 0
    bad_examples: list[str] = []
    for i, p in enumerate(docs):
        if i % every:
            continue
        b = p.read_bytes()
        try:
            b.decode("utf-8")
            ok += 1
        except UnicodeDecodeError:
            bad += 1
            if len(bad_examples) < 5:
                bad_examples.append(p.name)
    return {
        "utf8_valid": ok,
        "utf8_invalid": bad,
        "utf8_valid_share": round(ok / max(1, ok + bad), 3),
        "utf8_invalid_examples": bad_examples,
    }


def charset_share(docs: list[Path], every: int = 1) -> dict:
    c: Counter[str] = Counter()
    for i, p in enumerate(docs):
        if i % every:
            continue
        with p.open("rb") as f:
            c[declared_charset(f.read(4096))] += 1
    total = sum(c.values()) or 1
    return {
        "declared_charset": {k: round(v / total, 3) for k, v in c.most_common(8)},
    }


# ---------------------------------------------------------------------------
# per-dataset preparation
# ---------------------------------------------------------------------------


def prep_swde() -> dict:
    src = DATA / "swde" / "swde" / "sourceCode" / "sourceCode"
    dst = OUT / "swde"
    dst.mkdir(parents=True, exist_ok=True)

    for v in SWDE_VERTICALS:
        archive = src / f"{v}.7z"
        target = dst / v
        if target.is_dir() and any(target.iterdir()):
            log(f"  swde/{v}: already extracted")
            continue
        if not archive.exists():
            log(f"  swde/{v}: missing {archive.name}, skipped")
            continue
        log(f"  swde/{v}: extracting {archive.name} ({archive.stat().st_size/2**20:.0f} MiB)")
        subprocess.run(
            ["7z", "x", "-y", f"-o{dst}", str(archive)],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        # 7z puts the vertical's contents under dst/<vertical>/ but the name
        # inside the archive may differ (nbaplayer.7z -> nbaplayer/), so only
        # rename when the archive produced something else.
        if not target.is_dir():
            cands = [p for p in dst.iterdir() if p.is_dir() and p.name != v]
            if len(cands) == 1:
                cands[0].rename(target)

    # Ground truth: "<vertical>-<site>-<attr>.txt" in `groundtruth/<vertical>/`,
    # shipped as groundtruth.7z rather than checked in beside the archives.
    gt_root = src / "groundtruth"
    if not all((gt_root / v).is_dir() for v in SWDE_VERTICALS) and (src / "groundtruth.7z").exists():
        log(f"  swde: extracting groundtruth.7z")
        subprocess.run(
            ["7z", "x", "-y", f"-o{src}", str(src / "groundtruth.7z")],
            check=True,
            stdout=subprocess.DEVNULL,
        )
    labels_path = dst / "labels.jsonl"
    wrote = skipped_empty = 0
    with labels_path.open("w", encoding="utf-8") as out:
        for v in SWDE_VERTICALS:
            gt_dir = gt_root / v
            if not gt_dir.is_dir():
                log(f"  swde: no ground truth for {v}, skipped")
                continue
            for f in sorted(gt_dir.glob("*.txt")):
                lines = f.read_text(encoding="utf-8", errors="replace").splitlines()
                if len(lines) < 3:
                    continue
                head = lines[0].lstrip("\ufeff").split("\t")
                if len(head) < 3:
                    continue
                _vertical, site, attr = head[0], head[1], head[2]
                # Header counts: pages, ?, values, pages with a non-empty value.
                counts = lines[1].split("\t")
                nonempty = int(counts[3]) if len(counts) > 3 and counts[3].isdigit() else -1
                if nonempty == 0:
                    skipped_empty += 1
                    continue
                pages: dict[str, list[str]] = {}
                for line in lines[2:]:
                    parts = line.split("\t")
                    if len(parts) < 2 or not parts[1].isdigit():
                        continue
                    pid, n = parts[0], int(parts[1])
                    if n == 0:
                        continue  # "<NULL>": the page has no value for this attribute
                    pages[pid] = parts[2 : 2 + n]
                if not pages:
                    skipped_empty += 1
                    continue
                rec = {"vertical": v, "site": site, "attr": attr, "pages": pages}
                out.write(json.dumps(rec, ensure_ascii=False) + "\n")
                wrote += 1
    log(
        f"  swde: {wrote} attribute label files -> {labels_path.name}"
        f" ({skipped_empty} all-NULL attributes skipped)"
    )

    docs = sorted(p for p in dst.rglob("*.htm"))
    paths = [p for p in docs]
    return {
        "documents": len(docs),
        "bytes": sum(p.stat().st_size for p in paths),
        "verticals": [v for v in SWDE_VERTICALS if (dst / v).is_dir()],
        "sites": len({p.parent.name for p in paths}),
        "labels": wrote,
        **charset_share(paths, every=20),
        **utf8_share(paths, every=20),
    }




# ---------------------------------------------------------------------------
# benchmark manifests
# ---------------------------------------------------------------------------


def size_bucket(size: int) -> str:
    if size < 8 * 1024:
        return "<8KiB"
    if size < 32 * 1024:
        return "8-32KiB"
    if size < 128 * 1024:
        return "32-128KiB"
    if size < 512 * 1024:
        return "128-512KiB"
    if size < 2 * 1024 * 1024:
        return "512KiB-2MiB"
    return ">2MiB"


def rel(path: Path) -> str:
    return path.relative_to(ROOT).as_posix()


def write_jsonl(path: Path, rows: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as out:
        for row in rows:
            out.write(json.dumps(row, ensure_ascii=False, separators=(",", ":")) + "\n")
    log(f"  manifest: {len(rows)} rows -> {path.relative_to(ROOT)}")


def prep_cc_manifest(per_bucket: int, seed: int) -> None:
    """Export the Common Crawl sample as a size-stratified manifest."""
    if not CC_HTML.is_dir():
        log("  cc: data/commoncrawl/html missing, skipped")
        return
    meta: dict[str, dict] = {}
    manifest = CC_HTML / "manifest.jsonl"
    if manifest.is_file():
        for line in manifest.read_text(encoding="utf-8").splitlines():
            if line.strip():
                row = json.loads(line)
                meta[row.get("file", "")] = row
    buckets: dict[str, list[tuple[str, Path, int]]] = defaultdict(list)
    for path in sorted(CC_HTML.glob("*.html")):
        try:
            size = path.stat().st_size
        except OSError:
            continue
        url = meta.get(path.name, {}).get("url", path.name)
        buckets[size_bucket(size)].append((url, path, size))

    rows: list[dict] = []
    for bucket, entries in sorted(buckets.items()):
        entries.sort(key=lambda item: hashlib.sha1(f"{seed}:{item[0]}".encode()).digest())
        chosen = entries if per_bucket <= 0 else entries[:per_bucket]
        for url, path, size in chosen:
            rows.append(
                {
                    "id": f"cc:{path.name}",
                    "path": rel(path),
                    "bytes": size,
                    "bucket": bucket,
                    "url": url,
                    "queries": rel(WORKLOAD / "cc10.sel") if (WORKLOAD / "cc10.sel").is_file() else None,
                }
            )
    write_jsonl(MANIFESTS / "cc.jsonl", rows)


def prep_swde_manifest() -> None:
    root = OUT / "swde"
    if not root.is_dir():
        log("  swde: prepared documents missing, skipped")
        return
    rows: list[dict] = []
    for path in sorted(root.rglob("*.htm")) + sorted(root.rglob("*.html")):
        try:
            size = path.stat().st_size
        except OSError:
            continue
        site = path.parent.name
        vertical = path.parent.parent.name
        # One per-site selector set, derived from the site's own template
        # (`make_swde_selectors.py`): `tag.class`, i.e. the shape a crawler
        # author writes.  Positional devtools paths were removed (BENCHLOG
        # step115) - they measure the repair rules, not crawling.
        queries = WORKLOAD / "swde" / f"{site}.sel"
        rows.append(
            {
                "id": f"swde:{site}/{path.name}",
                "path": rel(path),
                "bytes": size,
                "bucket": vertical,
                "vertical": vertical,
                "site": site,
                "queries": rel(queries) if queries.is_file() else None,
            }
        )
    write_jsonl(MANIFESTS / "swde.jsonl", rows)


def prep_wpt_manifest() -> None:
    root = DATA / "wpt" / "html" / "syntax" / "parsing" / "resources"
    if not root.is_dir():
        log("  wpt: data/wpt missing, skipped")
        return
    rows = [
        {"id": f"wpt:{path.name}", "path": rel(path), "bytes": path.stat().st_size}
        for path in sorted(root.glob("*.dat"))
    ]
    write_jsonl(MANIFESTS / "wpt.jsonl", rows)


PREP = {
    "swde": prep_swde,
}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("datasets", nargs="*", default=list(PREP), help="swde")
    ap.add_argument(
        "--cc-per-bucket",
        type=int,
        default=200,
        help="Common Crawl documents per size bucket (0 = all)",
    )
    ap.add_argument("--seed", type=int, default=0, help="Common Crawl sample seed")
    ap.add_argument(
        "--skip-manifests", action="store_true", help="do not write data/prepared/manifests"
    )
    ap.add_argument(
        "--manifests-only",
        action="store_true",
        help="only regenerate data/prepared/manifests (skip extraction/labels)",
    )
    args = ap.parse_args()

    if not args.datasets:
        args.datasets = list(PREP)
    unknown = [d for d in args.datasets if d not in PREP]
    if unknown:
        ap.error(f"unknown dataset(s): {unknown}")

    OUT.mkdir(parents=True, exist_ok=True)
    manifest_path = OUT / "MANIFEST.json"
    manifest = {}
    if manifest_path.exists():
        manifest = json.loads(manifest_path.read_text())

    if not args.manifests_only:
        for name in args.datasets:
            log(f"== {name}")
            st = PREP[name]()
            manifest[name] = st
            log(f"  {json.dumps(st, ensure_ascii=False)}")

    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    log(f"\nmanifest -> {manifest_path}")

    if not args.skip_manifests:
        log("== benchmark manifests")
        prep_cc_manifest(args.cc_per_bucket, args.seed)
        prep_swde_manifest()
        prep_wpt_manifest()
    return 0


if __name__ == "__main__":
    sys.exit(main())
