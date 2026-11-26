# simdsel

Fast HTML parsing and CSS/XPath selection, powered by Rust and SIMD, with a
[parsel](https://github.com/scrapy/parsel)-compatible selector API.

```bash
uv add simdsel
# or
pip install simdsel
```

```python
from simdsel.parsel import Selector

html = '<div class="item" data-id="1"><h2>First &amp; best</h2><a href="/one">one</a></div>'
sel = Selector(text=html)

sel.css(".item h2::text").get()        # 'First & best'
sel.css("a::attr(href)").get()         # '/one'
sel.xpath("//div/@data-id").get()      # '1'
```

```{toctree}
:hidden:
usage
scrapy
compatibility
api
```
