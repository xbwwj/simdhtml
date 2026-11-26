import parsel
import pytest
from simdsel.parsel import Selector

HTML = (
    "<html lang='en'><head><title>T</title></head><body>"
    "<div id='a' class='x y'><p>one</p><p class='y'>two</p><a href='/l1'>l1</a></div>"
    "<div id='b'><span>s1</span><span>s2</span><p>three</p><a href='http://e/x'>l2</a></div>"
    "<table><tr><td>1</td><td>2</td></tr></table>"
    "<h1>heading</h1><h2>sub</h2><p class='content'><span>c</span></p>"
    "</body></html>"
)

EXPRESSIONS = [
    "//p",
    "//p[2]",
    "//div/p[1]",
    "//p/text()",
    "//a/@href",
    "//div[@id='b']/span",
    "//div[contains(@class,'y')]",
    "//p[last()]",
    "//*[@id]",
    "//div//a",
    "/html/body/div[1]",
    "//span[position()>1]",
    "//div | //span",
    "//p[.='two']",
    "//div/following-sibling::div",
    "//span[2]/preceding-sibling::span",
    "count(//p)",
    "//td[2]",
    "//p[@class]",
    "//div[@id='a']/p/text()",
    "//*[@class]/@class",
    "//table//tr[1]//text()",
    "//div[not(@id='a')]",
    "//span[position()=last()]",
    "//a[starts-with(@href,'http')]",
    "//h1",
    "//h2/following-sibling::p[1]",
    "//*[@class='content']//p",
    "string(.)",
    "count(//p)",
    "boolean(//p)",
    "substring-after(//a/@href, '=')",
    "string-length(//p[1])",
    "translate(//p[1], 'on', 'ON')",
    "sum(//td)",
    "round(-2.5)",
    "id('a')",
    "namespace-uri(//p[1])",
]


def canon(values):
    out = []
    for value in values:
        if isinstance(value, str) and value.startswith("<"):
            out.append(parsel.Selector(text=value).get())
        else:
            out.append(value)
    return out


def call(selector, query):
    try:
        return ("ok", canon(selector.xpath(query).getall()))
    except Exception as error:
        return ("err", f"{type(error).__name__}: {str(error).splitlines()[0]}")


@pytest.mark.parametrize("query", EXPRESSIONS)
def test_parsel_xpath(query):
    ours = call(Selector(text=HTML), query)
    theirs = call(parsel.Selector(text=HTML), query)
    assert ours == theirs, query


@pytest.mark.parametrize(
    "html,expr,texts",
    [
        ('<b class="x">1<p class="y">2</b>3', "//b", ["1", "2"]),
        ("<b><i>x</b>y", "//i", ["x", "y"]),
    ],
)
def test_xpath_reused_tag_tokens(html, expr, texts):
    sel = Selector(text=html, mode="spec")
    assert [node.text for node in sel.xpath(expr)] == texts
    assert sel.xpath(f"count({expr})").get() == f"{float(len(texts))}"
