#!/usr/bin/env python3
"""Derive per-site *field* selectors for SWDE from the ground truth.

The template-derived selector set (`make_swde_selectors.py`) is all
`tag.class`: those are the classes a site repeats, not the paths to the fields a
crawler actually wants.  SWDE's values live inside nested tables, so a real
wrapper needs descendant chains and positions (`div > table:nth-of-type(3) >
tr > td:nth-of-type(2)`), which is exactly the shape the template set never
produces.

So: take each (site, attribute) ground-truth value, find the deepest element
holding it on the first page, walk up building either a unique `tag.class` (when
one is unique on the page) or `tag:nth-of-type(k)`, and keep the shortest
selector that still matches a value on a couple of other pages of the same site.

Correctness is not the point (see PLAN: the selectors only have to be stable and
representative); what matters is that the *shape* of a real extraction selector
- multi-compound, positional - is present in the benchmark.

Output:
  data/prepared/workload/swde-gold/<site>.sel
  data/prepared/workload/swde-gold.manifest
"""

from __future__ import annotations

import collections
import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
DATA = ROOT / "data" / "prepared"
OUT = DATA / "workload"

PAGES_PER_ATTR = 4
MAX_SELECTORS_PER_SITE = 12


def element_path(node, tree) -> str | None:
    """CSS path from the document root down to `node`."""
    parts: list[str] = []
    while node is not None and node.tag not in (None, "-undef", "html", "body"):
        attrs = node.attributes
        classes = (attrs.get("class") or "").split()
        anchored = False
        for cls in classes[:1]:
            if cls.replace("-", "").replace("_", "").isalnum():
                candidate = f"{node.tag}.{cls}"
                if len(tree.css(candidate)) == 1:
                    parts.append(candidate)
                    anchored = True
                    break
        if not anchored:
            # Position among same-tag siblings (selectolax has no children list).
            index = 1
            same_tag = 1
            sibling = node.prev
            while sibling is not None:
                if sibling.tag == node.tag:
                    same_tag += 1
                    index += 1
                sibling = sibling.prev
            sibling = node.next
            while sibling is not None:
                if sibling.tag == node.tag:
                    same_tag += 1
                sibling = sibling.next
            if same_tag > 1:
                parts.append(f"{node.tag}:nth-of-type({index})")
            else:
                parts.append(node.tag)
        node = node.parent
    if not parts:
        return None
    return " > ".join(reversed(parts))


def deepest_with_text(tree, value: str):
    """Deepest element whose text starts with `value` (values may be snippets)."""
    best = None
    for node in tree.css("*"):
        text = (node.text() or "").strip()
        if not text or not value:
            continue
        if text == value or (len(value) > 8 and text.startswith(value)):
            best = node  # document order: later nodes are deeper or later siblings
    return best


def main() -> None:
    from selectolax.lexbor import LexborHTMLParser

    labels: dict[str, list[dict]] = collections.defaultdict(list)
    for line in (DATA / "swde" / "labels.jsonl").open(encoding="utf-8"):
        rec = json.loads(line)
        labels[f"{rec['vertical']}-{rec['site']}"].append(rec)

    out_dir = OUT / "swde-gold"
    out_dir.mkdir(parents=True, exist_ok=True)
    manifest = []
    stats = collections.Counter()

    for site, recs in sorted(labels.items()):
        # SWDE dirs carry the page count in the name: "<vertical>-<site>(<n>)".
        site_dirs = list((DATA / "swde").glob(f"*/{site}(*)"))
        if not site_dirs:
            continue
        site_dir = site_dirs[0]
        pages = {p.stem: p for p in sorted(site_dir.glob("*.htm"))}
        selectors: list[str] = []
        for rec in recs:
            derived = None
            for page_id, values in rec["pages"].items():
                if not values or page_id not in pages:
                    continue
                try:
                    tree = LexborHTMLParser(pages[page_id].read_bytes())
                except Exception:
                    continue
                node = None
                for value in values:
                    node = deepest_with_text(tree, value.strip())
                    if node is not None:
                        break
                if node is None:
                    continue
                selector = element_path(node, tree)
                if selector is None:
                    continue
                # Keep it only if it still finds something on the next pages:
                # a selector that matches nowhere is not a field wrapper.
                hits = 0
                checked = 0
                for other_id in list(pages)[:PAGES_PER_ATTR]:
                    if other_id == page_id:
                        continue
                    checked += 1
                    try:
                        other = LexborHTMLParser(pages[other_id].read_bytes())
                    except Exception:
                        continue
                    hits += len(other.css(selector))
                    if checked >= 2:
                        break
                if hits:
                    derived = selector
                    break
            if derived:
                selectors.append(derived)

        selectors = list(dict.fromkeys(selectors))[:MAX_SELECTORS_PER_SITE]
        if not selectors:
            continue
        path = out_dir / f"{site}.sel"
        path.write_text("\n".join(selectors) + "\n")
        manifest.append(f"swde-gold:{site}\t{site_dir}\t{path}")
        for selector in selectors:
            stats["selectors"] += 1
            compounds = sum(1 for _ in selector.split(" > "))
            stats["single" if compounds == 1 else "multi"] += 1
            for token, key in ((":nth-", "nth"), (":not(", "not"), (" + ", "sibling")):
                if token in selector:
                    stats[key] += 1

    (OUT / "swde-gold.manifest").write_text("\n".join(manifest) + "\n")
    print(f"swde-gold: {len(manifest)} sites, {stats['selectors']} selectors")
    print(
        f"  single-compound {stats['single']}, multi-compound {stats['multi']}, "
        f"nth-* {stats['nth']}, :not {stats['not']}, sibling {stats['sibling']}"
    )
    sample = sorted(out_dir.glob("*.sel"))[0]
    print("  sample:", sample.name, "->", sample.read_text().splitlines()[:3])


if __name__ == "__main__":
    main()
