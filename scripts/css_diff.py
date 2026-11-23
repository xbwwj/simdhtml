#!/usr/bin/env python3
"""Differential test of `simdsel.Selector.css` against parsel.

parsel translates CSS to XPath with cssselect and runs it on lxml.html, so it is
the reference for "what a Scrapy spider expects".  Our engine is independent, so
every difference here is a selector-semantics difference, not a parser one
(parsel parses with libxml2, which already differs from us; use `mode=` to pick
how close the tree is).

Usage:
    .venv/bin/python scripts/css_diff.py            # all cases
    .venv/bin/python scripts/css_diff.py ':link'    # only cases containing a substring
"""

import sys

import parsel
import simdsel

# Fixtures are grouped so that a failure points at one shape of markup.
FIXTURES = {
    "nested": (
        '<html lang="en"><head><title>T</title></head><body>'
        '<div id="list" class="box main">'
        '<div class="item" data-id="1" data-kind="a b">'
        '<h2 class="title">First &amp; best</h2>'
        '<a class="link" href="/one" title="a&amp;b">one</a>'
        '<span class="price">$10</span>'
        "<ul><li>i1</li><li>i2</li><li>i3</li></ul>"
        "<input type=checkbox checked><p></p>"
        "</div>"
        '<div class="item" data-id="2">'
        '<h2 class="title">Second</h2>'
        '<a class="link" href="/two">two</a><span class="price">$20</span>'
        "</div></div>"
        "<svg><text>x</text></svg>"
        "<script>var a = 1 < 2;</script>"
        "</body></html>"
    ),
    # Text nodes interleaved with nested elements: exposes result ordering.
    "textual": "<div id=o><span>s1</span>x1<div id=i>y1</div>x2<span>s2</span></div>",
    "forms": (
        '<html lang="en"><body><div><a href="/x">l</a>'
        "<input type=checkbox checked><input type=text disabled>"
        "<select><option selected>o</option><option>p</option></select>"
        "<p></p><span>s</span><em>e</em></div></body></html>"
    ),
    "case": '<html ID="top"><body><DIV CLASS="Box" DATA-X="1"><p>a</p></DIV></body></html>',
}

# (fixture, selector).  Every pair is run through both engines.
CASES = [
    # combinators
    ("nested", ".item"),
    ("nested", ".item .title"),
    ("nested", "#list > .item"),
    ("nested", ".item + .item"),
    ("nested", ".item ~ .item"),
    ("nested", "body div:nth-child(1)"),
    ("nested", "div.item > ul > li::text"),
    # attribute selectors
    ("nested", "[data-id]"),
    ("nested", "[data-id='1']"),
    ("nested", "[data-kind~=b]"),
    ("nested", "[lang|=en]"),
    ("nested", "[href^='/']"),
    ("nested", "[href$='two']"),
    ("nested", "[href*='on']"),
    ("nested", "a[title]"),
    ("nested", "input[checked]"),
    ("nested", "[data-id!='1']"),
    ("case", "[data-x=abc i]"),
    # pseudo-classes
    ("nested", ".item:first-child"),
    ("nested", ".item:nth-child(2n+1)"),
    ("nested", "li:nth-of-type(2)"),
    ("nested", ":root"),
    ("nested", "p:empty"),
    ("nested", "div:not(.item)"),
    ("nested", "div:has(a)"),
    ("nested", ":is(.item, li)"),
    ("nested", ":contains('Second')"),
    ("forms", ":link"),
    ("forms", ":checked"),
    ("forms", ":disabled"),
    ("forms", ":enabled"),
    ("forms", ":lang(en)"),
    # pseudo-elements
    ("nested", "a::attr(href)"),
    ("nested", ".title::text"),
    ("nested", ".item ::text"),
    ("nested", "div > ::text"),
    ("nested", "*::text"),
    ("textual", "div::text"),
    ("textual", "div ::text"),
    ("textual", "div *::text"),
    ("nested", "::text"),
    # parsing corners
    ("case", "DIV.Box"),
    ("case", ".Box"),
    ("case", "*|*"),
    ("nested", "div/*c*/.item"),
    ("nested", "p, a"),
]


def canon(values):
    """Normalise serialisation so only selector results are compared.

    We return source slices, parsel returns lxml's re-serialisation (quoted
    attributes, lowercased names, folded references).  Re-serialising our
    element results through parsel removes that difference; text and attribute
    values are compared verbatim.
    """
    out = []
    for value in values:
        if value.startswith("<"):
            value = parsel.Selector(text=value).get()
        out.append(value)
    return out


def call(selector, query):
    try:
        return ("ok", canon(selector.css(query).getall()))
    except Exception as error:  # noqa: BLE001 - the type is part of the comparison
        return ("err", f"{type(error).__name__}: {str(error).splitlines()[0]}")


def main():
    needle = sys.argv[1] if len(sys.argv) > 1 else ""
    roots = {
        name: (simdsel.Selector(text=html), parsel.Selector(text=html))
        for name, html in FIXTURES.items()
    }
    diffs = 0
    for fixture, query in CASES:
        if needle not in query:
            continue
        ours, theirs = call(roots[fixture][0], query), call(roots[fixture][1], query)
        if ours == theirs:
            continue
        diffs += 1
        print(f"[{fixture}] {query!r}")
        print(f"  simdsel: {short(ours)}")
        print(f"  parsel : {short(theirs)}")

    total = sum(1 for _, query in CASES if needle in query)
    print(f"\n{total - diffs}/{total} cases agree")


def short(result):
    kind, value = result
    if kind == "err":
        return value
    if not isinstance(value, list):
        return repr(value)
    return f"{len(value)} items: {str(value)[:160]}"


if __name__ == "__main__":
    main()
