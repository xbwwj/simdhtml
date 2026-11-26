# Using simdsel with Scrapy

Scrapy's default selector is parsel. simdsel exposes a compatible selector
surface, so there are two ways to use it: call it directly in a spider, or
replace `response.selector` through a small downloader middleware. Both examples
use the public `simdsel.parsel` API.

## Directly in a spider

This is the quickest way to try simdsel or to measure it against parsel. Nothing
in Scrapy needs to change.

```python
import scrapy
from simdsel.parsel import Selector


class ProductSpider(scrapy.Spider):
    name = "products"

    def parse(self, response):
        sel = Selector(
            body=response.body,
            encoding=response.encoding,
            base_url=response.url,
        )
        for product in sel.css("div.product"):
            yield {
                "title": product.css("h2::text").get(),
                "url": product.css("a::attr(href)").get(),
            }
```

`body=` takes the raw response bytes and `encoding=` reuses Scrapy's detected
charset. Request scheduling, retries and `response.follow()` are untouched.

A `Selector` is reference counted and not thread-safe: create one per response
and keep it inside the callback. Do not cache it between requests.

The trade-off is that spiders have to call `Selector(...)` instead of
`response.css(...)`.

## Response adapter

For existing spiders, subclass `HtmlResponse` and override `selector`; a
downloader middleware then swaps the response class so `response.css()` and
`response.xpath()` are backed by simdsel.

```python
from functools import cached_property

from scrapy.http import HtmlResponse
from simdsel.parsel import Selector


class SimdselResponse(HtmlResponse):
    @cached_property
    def selector(self):
        return Selector(
            body=self.body,
            encoding=self.encoding,
            base_url=self.url,
        )


class SimdselMiddleware:
    def process_response(self, request, response, spider):
        if isinstance(response, HtmlResponse) and not isinstance(
            response, SimdselResponse
        ):
            return response.replace(cls=SimdselResponse)
        return response
```

Enable the middleware:

```python
DOWNLOADER_MIDDLEWARES = {
    "myproject.middlewares.SimdselMiddleware": 543,
}
```

`Response.replace(cls=...)` keeps the status, headers, URL and body; only the
class changes. The spider code stays as it was:

```python
def parse(self, response):
    for product in response.css("div.product"):
        yield {
            "title": product.css("h2::text").get(),
            "url": product.css("a::attr(href)").get(),
        }
```

Notes:

- Only `HtmlResponse` is converted. `XmlResponse` keeps the parsel selector.
- Keep the `cached_property`: Scrapy reads `response.selector` again on every
  `response.css()` / `response.xpath()` call.
- Third-party code that relies on parsel-only behaviour (`.root`, `drop()`,
  resolved namespaces) will not find it on the simdsel selector. Test the
  extensions your project uses.
- The adapter is deliberately small. Check it against your Scrapy version and
  any custom `Response` subclass in your project.

## Choosing

Use the direct form for benchmarks, one-off scripts and new code. Use the
adapter when existing spiders should keep `response.css()` and the selector swap
should be transparent. Both paths use the same parser and selector engine.
