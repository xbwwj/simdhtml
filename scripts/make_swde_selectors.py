#!/usr/bin/env python3
"""Build the extraction workloads for the two horizontal benchmarks.

`cc-fixed`: one fixed selector list, applied to every CommonCrawl document.

`swde-per-site`: for each SWDE site, a selector list derived from that site's
own template.  The ground truth values tell us nothing about *where* they are,
and hand-writing 80 wrappers is not the job - the job is to have a stable,
realistic per-site selector set so the same query runs over thousands of
near-identical pages, which is what a targeted crawler does.  So: parse a few
pages per site, count `tag.class` pairs, keep the ones that appear on most
pages of that site, and emit the most frequent ones as the workload.

Writes:

  data/prepared/workload/cc10.sel
  data/prepared/workload/swde/<site>.sel
  data/prepared/workload/swde.manifest     label<TAB>docs_dir<TAB>selectors
"""

from __future__ import annotations

import collections
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
DATA = ROOT / "data" / "prepared"
OUT = DATA / "workload"

# General extraction: text, links, images, and the usual article/table shapes.
# Deliberately plain CSS that every engine supports, and deliberately selector
# text (not precompiled handles) so each engine pays its own compilation.
CC_SELECTORS = [
    "a[href]",
    "img[src]",
    "p",
    "h1",
    "h2",
    "article",
    "main",
    "div.content",
    "table",
    "ul li",
]

# Pages per site used to derive the selector list, and how many selectors.
SAMPLE_PAGES = 8
SELECTORS_PER_SITE = 10
# A selector must appear on at least this share of the sampled pages to count as
# part of the site template.
MIN_PAGE_SHARE = 0.6


def cc() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    path = OUT / "cc10.sel"
    path.write_text(
        "# general extraction: links, images, text, article/table shapes\n"
        + "\n".join(CC_SELECTORS)
        + "\n"
    )
    print(f"cc-fixed: {len(CC_SELECTORS)} selectors -> {path}")


def swde() -> None:
    from selectolax.lexbor import LexborHTMLParser as HTMLParser

    root = DATA / "swde"
    out_dir = OUT / "swde"
    out_dir.mkdir(parents=True, exist_ok=True)
    manifest = []
    for site_dir in sorted(p for p in root.glob("*/*") if p.is_dir()):
        site = site_dir.name
        pages = sorted(site_dir.glob("*.htm"))[:SAMPLE_PAGES]
        if not pages:
            continue
        hits: collections.Counter[str] = collections.Counter()
        matched: collections.Counter[str] = collections.Counter()
        trees = []
        for page in pages:
            try:
                tree = HTMLParser(page.read_bytes())
            except Exception:
                continue
            trees.append(tree)
            seen = set()
            for node in tree.css("*"):
                tag = node.tag
                for cls in (node.attributes.get("class") or "").split():
                    sel = f"{tag}.{cls}"
                    if sel not in seen:
                        seen.add(sel)
                        hits[sel] += 1
        keep = []
        for sel, count in hits.items():
            if count < MIN_PAGE_SHARE * len(pages):
                continue
            # A field selector matches a handful of nodes per page, not the
            # whole template; keeping the result set small also keeps the
            # measurement about query speed instead of result-set size.
            name = sel.replace(".", "")
            if not name.isalnum():
                continue
            matched[sel] = sum(len(tree.css(sel)) for tree in trees) // max(1, len(trees))
            if 1 <= matched[sel] <= 100:
                keep.append((count, matched[sel], sel))
        keep.sort(reverse=True)
        selectors = [sel for _, _, sel in keep[:SELECTORS_PER_SITE]]
        if not selectors:
            continue
        (out_dir / f"{site}.sel").write_text("\n".join(selectors) + "\n")
        manifest.append(f"swde:{site}\t{site_dir}\t{out_dir / f'{site}.sel'}")

    (OUT / "swde.manifest").write_text("\n".join(manifest) + "\n")
    print(f"swde-per-site: {len(manifest)} sites -> {OUT / 'swde.manifest'}")


if __name__ == "__main__":
    which = sys.argv[1:] or ["cc", "swde"]
    if "cc" in which:
        cc()
    if "swde" in which:
        swde()
