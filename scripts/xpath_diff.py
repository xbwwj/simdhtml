#!/usr/bin/env python3
"""Differential test of our XPath evaluator against lxml, on the *same* tree.

`crates/simdsel/examples/xpath_dump` dumps each parsed document's node tape plus
our XPath results as JSON.  Here we rebuild that exact tree with lxml (no
re-parsing, so no second HTML repair pass) and run the same expressions with
lxml's XPath engine.  Differences are therefore XPath-implementation bugs, not
tree-construction differences -- the CSS engine and the HTML repair rules are
out of scope by construction.

Usage:
    cargo run --release -p simdsel --example xpath_dump -- \
        data/commoncrawl/html scripts/xpath_exprs.txt 40 > /tmp/xpath.jsonl
    .venv/bin/python scripts/xpath_diff.py /tmp/xpath.jsonl
"""
import json
import sys

from lxml import etree


def lower(name):
    """Lowercase and make an HTML name valid as an XML name (for lxml only)."""
    if not isinstance(name, str):
        return name
    return "".join(
        c if (c.isascii() and (c.isalnum() or c in "_.-")) else "_"
        for c in name.lower()
    )


def clean(text):
    """Drop control characters lxml refuses; applied to both sides equally."""
    if not isinstance(text, str):
        return text
    return "".join(c for c in text if c >= " " or c in "\t\n\r")


def build(tree):
    """Rebuild the dumped preorder tape as an lxml tree (structure preserved)."""
    nodes = [None] * len(tree)
    for index, node in enumerate(tree):
        if node["kind"] == "element":
            element = etree.Element(lower(node["name"]))
            for name, value in node["attrs"]:
                if lower(name) not in element.attrib:
                    element.attrib[lower(name)] = clean(value)
            nodes[index] = element
        elif node["kind"] == "text":
            nodes[index] = clean(node["text"])

    children = {}
    for index, node in enumerate(tree):
        if node["kind"] == "document":
            continue
        children.setdefault(node["parent"], []).append(index)

    def attach(parent_index):
        parent = nodes[parent_index]
        previous_element = None
        for index in children.get(parent_index, []):
            node = tree[index]
            if node["kind"] == "text":
                text = nodes[index]
                if previous_element is None:
                    parent.text = (parent.text or "") + text
                else:
                    previous_element.tail = (previous_element.tail or "") + text
            else:
                child = nodes[index]
                parent.append(child)
                attach(index)
                previous_element = child

    roots = [index for index in children.get(0, []) if tree[index]["kind"] == "element"]
    for index in children.get(0, []):
        if tree[index]["kind"] == "element":
            attach(index)
    if not roots:
        return None
    return nodes[roots[0]]


def normalise(value):
    if isinstance(value, bool):
        return ("bool", value)
    if isinstance(value, float):
        return ("number", value)
    if isinstance(value, etree._Element):
        return ("element", lower(value.tag))
    if isinstance(value, str):
        if getattr(value, "is_attribute", False):
            return ("attribute", lower(value.attrname), str(value))
        if getattr(value, "is_text", False) or getattr(value, "is_tail", False):
            return ("text", clean(str(value)))
        return ("string", str(value))
    return ("other", str(value))


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "/tmp/xpath.jsonl"
    exprs = [
        line.strip()
        for line in open(sys.argv[2] if len(sys.argv) > 2 else "scripts/xpath_exprs.txt")
        if line.strip() and not line.startswith("#")
    ]
    totals = [0] * len(exprs)
    agree = [0] * len(exprs)
    mismatches = []
    docs = 0
    skipped_roots = 0
    for line in open(path):
        record = json.loads(line)
        # lxml's model has exactly one root element; our repair currently emits
        # extra roots for content after </html>, which is an HTML-repair gap and
        # not comparable here (it would show up as XPath "errors" in every
        # expression).  Count and skip those documents.
        roots = [
            i
            for i, node in enumerate(record["tree"])
            if node["kind"] == "element" and node["parent"] == 0
        ]
        if len(roots) != 1:
            skipped_roots += 1
            continue
        root = build(record["tree"])
        if root is None:
            continue
        docs += 1
        document = etree.ElementTree(root)
        for index, expr in enumerate(exprs):
            ours = record["results"][index]
            if ours and isinstance(ours[0], dict) and "error" in ours[0]:
                continue
            expected = document.xpath(expr)
            if not isinstance(expected, list):
                expected = [expected]
            want = [normalise(value) for value in expected]
            got = []
            for node in ours:
                if "kind" in node:
                    kind = node["kind"]
                    if kind == "element":
                        got.append(("element", lower(node["name"])))
                    elif kind == "attribute":
                        got.append(("attribute", lower(node["name"]), node["value"]))
                    else:
                        got.append(("text", clean(node["value"])))
                elif "number" in node:
                    got.append(("number", node["number"]))
                elif "string" in node:
                    got.append(("string", node["string"]))
                elif "bool" in node:
                    got.append(("bool", node["bool"]))
            totals[index] += 1
            if got == want:
                agree[index] += 1
            elif len(mismatches) < 25:
                mismatches.append((record["doc"], expr, want[:6], got[:6]))

    print(f"documents: {docs} (skipped {skipped_roots} without a single root element)")
    print(f"{'expression':48} {'agree':>6} {'of':>5}  rate")
    for index, expr in enumerate(exprs):
        total = totals[index]
        if total == 0:
            continue
        rate = agree[index] / total
        flag = "" if rate == 1.0 else "  <=="
        print(f"{expr:48} {agree[index]:6} {total:5}  {rate:5.1%}{flag}")
    all_total = sum(totals)
    all_agree = sum(agree)
    print(f"\noverall: {all_agree}/{all_total} = {all_agree / all_total:.2%}")
    for doc, expr, want, got in mismatches:
        print(f"\n{doc}  {expr}\n  lxml: {want}\n  ours: {got}")


if __name__ == "__main__":
    main()
