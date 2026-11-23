#!/usr/bin/env python3
"""Differential test of the parsel-shaped *API* against parsel.

`scripts/css_diff.py` covers selector semantics on fixtures; this one covers the
API surface around them: `get`/`getall`/`attrib`/`re`/`bool`/`str`, `xpath()`
relative paths and scalar results, `SelectorList` list behaviour, and the
constructor's validation.  Element results are re-serialized through parsel
before comparing, so only API behaviour is measured, not the (documented) fact
that we return source slices and parsel returns lxml's re-serialization.

Usage:
    .venv/bin/python scripts/parsel_api_diff.py
"""

import re as _re

import parsel
import simdsel

HTML = (
    "<html lang=en><body><div id=a class='x y'><p>one</p><p>two\n</p>"
    "<a href=/l>l</a>caf\u00e9</div><div id=b><span>s</span></div>"
    "<table><tr><td>1</td></tr></table></body></html>"
)

# name, callable(selector) -> comparable value
CASES = [
    ("get", lambda s: s.get()),
    ("getall", lambda s: s.getall()),
    ("attrib", lambda s: dict(s.attrib)),
    ("bool", bool),
    ("str", str),
    ("css p", lambda s: s.css("p").getall()),
    ("css p::text", lambda s: s.css("p::text").getall()),
    ("css text attrib", lambda s: dict(s.css("p::text").attrib)),
    ("css empty attrib", lambda s: dict(s.css(".no").attrib)),
    ("get default", lambda s: s.css(".no").get("d")),
    ("xpath //a/@href", lambda s: s.xpath("//a/@href").getall()),
    ("xpath string(.)", lambda s: s.xpath("string(.)").getall()),
    ("xpath union", lambda s: s.xpath("//p | //span").getall()),
    ("xpath count", lambda s: s.xpath("count(//p)").get()),
    ("xpath boolean", lambda s: s.xpath("boolean(//p)").get()),
    ("xpath relative", lambda s: s.css("#a").xpath("./p/text()").getall()),
    ("xpath nested", lambda s: s.xpath('//div[@id="a"]/p[2]/text()').getall()),
    ("xpath substring-after", lambda s: s.xpath("substring-after(//a/@href, '=')").getall()),
    ("xpath string-length", lambda s: s.xpath("string-length(//p[1])").getall()),
    ("xpath translate", lambda s: s.xpath("translate(//p[1], 'on', 'ON')").getall()),
    ("xpath sum", lambda s: s.xpath("sum(//td)").getall()),
    ("xpath round negative", lambda s: s.xpath("round(-2.5)").getall()),
    ("xpath id", lambda s: s.xpath("id('a')").getall()),
    ("xpath namespace-uri", lambda s: s.xpath("namespace-uri(//p[1])").getall()),
    ("xpath following-sibling", lambda s: s.xpath('//div[@id="a"]/following-sibling::div/span/text()').getall()),
    ("re", lambda s: s.css("p::text").re(r"(o)")),
    ("re dot-star", lambda s: s.css("p::text").re(r".*")),
    ("re multi-group flatten", lambda s: s.css("p::text").re(r"(o)(n)")),
    ("re named extract", lambda s: s.css("p::text").re(r"(?P<extract>o)\w")),
    ("re lookbehind", lambda s: s.css("p::text").re(r"(?<=t)wo")),
    ("re unicode \\w", lambda s: s.css("p::text").re(r"\w+")),
    ("re $ trailing newline", lambda s: s.css("p::text").re(r"e$")),
    ("re compiled flags", lambda s: s.css("p::text").re(_re.compile("TWO", _re.I))),
    ("re no entities", lambda s: s.css("p::text").re(r".*", replace_entities=False)),
    ("re first", lambda s: s.css("p::text").re_first(r"(t)")),
    ("re first default", lambda s: s.css(".no").re_first(r"x", "z")),
    ("re in list", lambda s: s.css("p::text").re(r"[a-z]")),
    ("namespaces", lambda s: sorted(s.namespaces)),
    ("type", lambda s: s.type),
    ("len list", lambda s: len(s.css("p"))),
    ("slice list", lambda s: s.css("p")[0:1].getall()),
    ("negative index", lambda s: s.css("p")[-1].get()),
    ("list +", lambda s: len(s.css("p") + s.css("div"))),
    ("iterate", lambda s: len(list(s.css("p")))),
    ("reversed", lambda s: [x.get() for x in reversed(s.css("p"))]),
    ("css on xpath element", lambda s: s.xpath("//div").css("p::text").getall()),
    ("xpath on css element", lambda s: s.css("div").xpath(".//p/text()").getall()),
]


def canon(value):
    """Re-serialize element results through parsel (see the module docstring)."""
    if isinstance(value, list):
        return [canon(item) for item in value]
    if isinstance(value, dict):
        return {key: canon(item) for key, item in value.items()}
    if isinstance(value, str) and value.startswith("<"):
        return parsel.Selector(text=value).get()
    return value


def call(selector, fn):
    try:
        return ("ok", canon(fn(selector)))
    except Exception as error:  # noqa: BLE001 - the type is part of the comparison
        return ("err", f"{type(error).__name__}: {str(error).splitlines()[0]}")


def main():
    ours, theirs = simdsel.Selector(text=HTML), parsel.Selector(text=HTML)
    diffs = 0
    for name, fn in CASES:
        left, right = call(ours, fn), call(theirs, fn)
        if left == right:
            continue
        diffs += 1
        print(f"[{name}]\n  simdsel: {str(left)[:200]}\n  parsel : {str(right)[:200]}")
    print(f"\n{len(CASES) - diffs}/{len(CASES)} cases agree")


if __name__ == "__main__":
    main()
