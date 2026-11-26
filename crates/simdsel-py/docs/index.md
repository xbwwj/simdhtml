# simdsel

**Gigabytes fast HTML parsing and CSS/XPath selection for Python, powered by Rust and SIMD.**

simdsel brings native parsing performance to Python web scraping. Its
Parsel-compatible API makes it easy to integrate into existing extraction
workflows, while a high-performance Rust engine handles the heavy lifting.

```bash
uv add simdsel
```

Or install with pip:

```bash
pip install simdsel
```

## Get started

Start extracting data with a familiar selector API:

```python
from simdsel.parsel import Selector

html = """
<article class="product" data-id="42">
    <h2>Mechanical Keyboard</h2>
    <a href="/products/42">View product</a>
</article>
"""

sel = Selector(text=html)

title = sel.css(".product h2::text").get()
url = sel.css(".product a::attr(href)").get()
product_id = sel.xpath("//article/@data-id").get()

print(title)       # Mechanical Keyboard
print(url)         # /products/42
print(product_id)  # 42
```

See [Usage](usage.md) for more examples, and the [API Reference](api.rst)
for every method.

## Built for high-throughput extraction

Parsing has been a huge bottleneck for large-scale web crawlers.
simdsel is designed to make this part of the pipeline faster.

- **Fast**: Over 4000 page/s/core in real-world workloads, 7x-150x faster than commonly used parsers.
- **Familiar**: CSS, XPath, `::text`, `::attr()` and familiar Parsel-style extraction methods.
- **Flexible**: Choose between a fast parsing path and specification-oriented HTML tree construction.
- **Automatic**: Adapt to available CPU features at runtime. No configuration needed.
- **Reliable**: Built in Rust; `unsafe` is confined to SIMD intrinsics and the FFI boundary.

## Benchmarks

Real-world measurements (Common Crawl and SWDE) and the methodology live in
the [simdhtml repository](https://github.com/xbwwj/simdhtml#benchmarks).

## Parsing modes

Different workloads have different requirements. simdsel provides two
parsing modes:

| Mode   | Behavior                                                             | Best for                                                          |
| ------ | -------------------------------------------------------------------- | ----------------------------------------------------------------- |
| `none` | Fast parsing without implied HTML elements or specification repairs. | Most scraping workloads                                           |
| `spec` | HTML specification tree construction, including browser-like repair. | Extremely malformed pages, or when browser-shaped trees are needed |

The default mode is `none`. Use `mode="spec"` when browser-shaped
HTML trees matter. The specification mode currently achieves 98% WPT
conformance.

## Using simdsel with Scrapy

simdsel provides a familiar selector interface for Scrapy projects.

You can use it directly with `response.body`:

```python
# Inside a spider callback.
from simdsel.parsel import Selector

sel = Selector(
    body=response.body,
    encoding=response.encoding,
    base_url=response.url,
)
url = sel.css("div.product a::attr(href)").get()
```

See [Scrapy integration](scrapy.md) for a complete spider, and for a small
adapter that keeps `response.css()` working in existing projects.

## Compatibility

simdsel follows the familiar Parsel selector API, making migration
straightforward for most extraction workloads.

It is not an lxml clone, however. Its tree construction, serialization
and some XPath behaviors have deliberate differences.

See [Compatibility](compatibility.md) for the details.

## Documentation

- [Usage](usage.md) — Input, selectors, extraction, regex and XPath.
- [Scrapy](scrapy.md) — Direct use and a `response.css()` adapter.
- [Compatibility](compatibility.md) — Differences from parsel and lxml.
- [API Reference](api.rst) — Complete API reference.

```{toctree}
:hidden:

usage
scrapy
compatibility
api
```
