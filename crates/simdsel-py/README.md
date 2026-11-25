# simdsel

**A gigabytes fast HTML parser and selector, powered by Rust and SIMD.**

`simdsel` is the Python binding for [`simdhtml`](https://github.com/xbwwj/simdhtml).
It keeps the `parsel` selector surface while replacing the parsing and selection
core with a SIMD-accelerated Rust implementation, so scraping code keeps working
with one changed import.

## Install

```bash
pip install simdsel
```

Wheels are CPython abi3 (Python ≥ 3.9) and self-contained: no Rust toolchain, no
libxml2, no lxml.

## Quickstart

```python
from simdsel.parsel import Selector

html = """
<div class="item" data-id="1"><h2>First &amp; best</h2><a href="/one">one</a></div>
<div class="item" data-id="2"><h2>Second</h2><a href="/two">two</a></div>
"""

sel = Selector(text=html)
sel.css(".item h2::text").getall()          # ['First & best', 'Second']
sel.css(".item a::attr(href)").getall()     # ['/one', '/two']
sel.xpath("//div[@data-id='2']/h2/text()").get()   # 'Second'
[item.attrib["data-id"] for item in sel.css(".item")]   # ['1', '2']
```

## parsel compatibility

`from simdsel.parsel import Selector` mirrors `parsel.Selector` for the subset
scraping code actually uses:

| API | |
|---|---|
| `css(query)`, `xpath(query)` | return a `SelectorList` |
| `::text`, `::attr(name)` | pseudo-elements |
| `.get()`, `.getall()` | single value / list |
| `.re(pattern)`, `.re_first(pattern)` | regex extraction |
| `.attrib`, `.text`, `.base_url`, `.namespaces` | |
| `len()`, indexing, iteration, `+` | list behaviour |
| `Selector(text=..., base_url=..., type="html")` | constructor |

`type="xml"`/`"json"` and `root=` (an lxml node) raise `NotImplementedError`: only
HTML is parsed.

`Selector(text=..., mode="spec")` opts into specification-shaped tree
construction (98% WPT conformance) instead of the fast error-recovering default
`mode="none"`.

## Notes

- `simdsel.__version__` reports the build version.
- Elements are backed by a flat preorder tape, so iteration and `len()` are cheap
  and no lxml node objects are allocated.

## Links

- Source, docs and benchmark methodology: <https://github.com/xbwwj/simdhtml>
- Benchmarks: <https://github.com/xbwwj/simdhtml#benchmarks>
- Issues: <https://github.com/xbwwj/simdhtml/issues>

## License

[MIT](https://github.com/xbwwj/simdhtml/blob/main/LICENSE-MIT) or
[Apache 2.0](https://github.com/xbwwj/simdhtml/blob/main/LICENSE-APACHE) at your
option.
