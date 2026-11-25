import parsel
import pytest
import simdsel

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
    "textual": "<div id=o><span>s1</span>x1<div id=i>y1</div>x2<span>s2</span></div>",
    "forms": (
        '<html lang="en"><body><div><a href="/x">l</a>'
        "<input type=checkbox checked><input type=text disabled>"
        "<select><option selected>o</option><option>p</option></select>"
        "<p></p><span>s</span><em>e</em></div></body></html>"
    ),
    "case": '<html ID="top"><body><DIV CLASS="Box" DATA-X="1"><p>a</p></DIV></body></html>',
}

CASES = [
    ("nested", ".item"),
    ("nested", ".item .title"),
    ("nested", "#list > .item"),
    ("nested", ".item + .item"),
    ("nested", ".item ~ .item"),
    ("nested", "body div:nth-child(1)"),
    ("nested", "div.item > ul > li::text"),
    ("nested", "[data-id]"),
    ("nested", "[data-id='1']"),
    ("nested", "[data-kind~=b]"),
    ("nested", "[lang|=en]"),
    ("nested", "[href^='/']"),
    ("nested", "[href$='two']"),
    ("nested", "[href*='on']"),
    ("nested", "a[title]"),
    ("nested", "input[checked]"),
    pytest.param("nested", "[data-id!='1']", marks=pytest.mark.xfail(strict=True, reason="parsel/cssselect [attr!=v] implementation detail")),
    ("case", "[data-x=abc i]"),
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
    ("nested", "a::attr(href)"),
    ("nested", ".title::text"),
    ("nested", ".item ::text"),
    ("nested", "div > ::text"),
    ("nested", "*::text"),
    ("textual", "div::text"),
    ("textual", "div ::text"),
    pytest.param("textual", "div *::text", marks=pytest.mark.xfail(strict=True, reason="parsel includes the context element in A *::text")),
    ("nested", "::text"),
    ("case", "DIV.Box"),
    ("case", ".Box"),
    ("case", "*|*"),
    ("nested", "div/*c*/.item"),
    ("nested", "p, a"),
]


def canon(values):
    out = []
    for value in values:
        if value.startswith("<"):
            value = parsel.Selector(text=value).get()
        out.append(value)
    return out


def call(selector, query):
    try:
        return ("ok", canon(selector.css(query).getall()))
    except Exception as error:
        return ("err", f"{type(error).__name__}: {str(error).splitlines()[0]}")


@pytest.mark.parametrize("fixture,query", CASES)
def test_parsel_css(fixture, query):
    html = FIXTURES[fixture]
    ours = call(simdsel.Selector(text=html), query)
    theirs = call(parsel.Selector(text=html), query)
    assert ours == theirs, f"[{fixture}] {query!r}"
