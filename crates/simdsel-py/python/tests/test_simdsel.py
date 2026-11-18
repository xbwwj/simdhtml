"""Smoke tests for the PyO3 binding.

Run with:

    uv run python -m unittest discover -s crates/simdsel-py/python/tests
"""

import unittest

import simdsel


HTML = """<html><head><title>T</title></head><body>
<div id="list">
  <div class="item" data-id="1"><h2 class="title">First &amp; best</h2><a class="link" href="/one">one</a><span class="price">$10</span></div>
  <div class="item" data-id="2"><h2 class="title">Second</h2><a class="link" href="/two">two</a><span class="price">$20</span></div>
</div>
</body></html>"""


class SelectorTest(unittest.TestCase):
    def setUp(self) -> None:
        self.sel = simdsel.Selector(text=HTML)

    def test_css_text_and_attr(self) -> None:
        self.assertEqual(
            self.sel.css(".item .title::text").getall(),
            ["First & best", "Second"],
        )
        self.assertEqual(
            self.sel.css(".item a.link::attr(href)").getall(),
            ["/one", "/two"],
        )

    def test_chaining_and_indexing(self) -> None:
        items = self.sel.css(".item")
        self.assertEqual(len(items), 2)
        self.assertEqual(items[1].css(".title::text").get(), "Second")
        self.assertEqual(items[-1].css(".title::text").get(), "Second")
        self.assertTrue(items)
        self.assertFalse(self.sel.css(".missing"))

    def test_xpath_is_not_exposed(self) -> None:
        self.assertFalse(hasattr(self.sel, "xpath"))

    def test_css_scope_and_contains(self) -> None:
        items = self.sel.css(".item")
        self.assertEqual(items.css(":scope > .title::text").getall(), ["First & best", "Second"])
        self.assertEqual(len(self.sel.css(".item:contains('First')")), 1)

    def test_text_attrib_and_regex(self) -> None:
        item = self.sel.css(".item")
        self.assertEqual(item.attrib["data-id"], "1")
        self.assertIn("First & best", item.text[0])
        self.assertEqual(
            self.sel.css(".title::text").re(r"([A-Z])"),
            ["F", "S"],
        )
        self.assertEqual(
            self.sel.css(".title::text").re_first(r"[A-Z]"),
            "F",
        )

    def test_body_and_errors(self) -> None:
        self.assertEqual(simdsel.Selector(body=b"<p>x</p>").css("p::text").get(), "x")
        batch = simdsel.Selector.parse_many([b"<p>a</p>", b"<p>b</p>"])
        self.assertEqual([item.css("p::text").get() for item in batch], ["a", "b"])
        with self.assertRaises(ValueError):
            self.sel.css("div >")
        with self.assertRaises(NotImplementedError):
            simdsel.Selector(text=HTML, type="xml")


if __name__ == "__main__":
    unittest.main()
