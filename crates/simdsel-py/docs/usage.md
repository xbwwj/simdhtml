# Usage

## Input

`Selector` accepts decoded text or the bytes off the wire:

```python
from simdsel.parsel import Selector

sel = Selector(text="<p>café</p>")
sel = Selector(body=b"<p>caf\xe9</p>")
sel = Selector(body=b"<p>caf\xe9</p>", encoding="windows-1252")
```

`body=` sniffs a BOM or `<meta charset>`; `encoding=` overrides the sniff.
`text=` is already decoded, so it never sniffs. With an explicit `encoding=`,
`text=` is round-tripped through that encoding first, like parsel.

Parsing is controlled by `mode=`:

| `mode` | Tree |
| --- | --- |
| `"none"` (default) | fast error-recovering parse, no implied tags |
| `"spec"` | HTML specification tree construction, 98% WPT conformance |

```python
selectors = Selector.parse_many([b"<p>a</p>", b"<p>b</p>"])
```

## Selectors

`css()` and `xpath()` return a new `SelectorList`, so queries chain:

```python
sel = Selector(text='<div class="item"><h2>First</h2></div>')

for item in sel.css(".item"):
    print(item.css("h2::text").get())
```

A chained query searches inside the current items; the items themselves are not
matched. Use `:scope` when you need to include them.

A document-level `Selector` represents the root element: `sel.get()` returns its
source slice, and `sel.attrib` its attributes.

## Extracting values

```python
sel.css(".item h2::text").getall()
sel.css(".item a::attr(href)").getall()
sel.css(".item").get()
sel.css(".item").text
sel.css(".item").attrib
```

| API | Result |
| --- | --- |
| `.get()` / `.getall()` | first value / all values |
| `.extract()` / `.extract_first()` | aliases for `getall()` / `get()` |
| `.text` | text content (a string, or a list on `SelectorList`) |
| `.attrib` | attributes of the first element |

For an element, `get()` is the original source slice: attribute quotes, entity
spelling and tag case are kept as written. Text and attribute results are
decoded. `SelectorList` also supports `len()`, indexing, slicing, iteration and
`+`, but it is not a `list` subclass.

`::text` and `::attr(name)` are parsel's extraction pseudo-elements, not
standard CSS.

## Regular expressions

`re()` and `re_first()` apply CPython's `re` module to `get()`:

```python
sel.css("h2::text").re(r"(\w+) & (\w+)")     # ['First', 'best']
sel.css("h2::text").re_first(r"(\w+)")       # 'First'
```

A named group `extract` takes precedence; otherwise `findall` is flattened.
`replace_entities=True` (default) decodes character references except `&amp;`
and `&lt;`. `re_first(pattern, default=...)` returns `default` when there is no
match.

## XPath

`xpath()` evaluates XPath 1.0 at the current items. A node-set becomes element
or text selectors; a scalar becomes one string selector:

```python
sel.xpath("//div[@data-id='2']/h2/text()").get()   # 'Second'
sel.xpath("string(//div[2]/@data-id)").get()       # '2'
sel.xpath("count(//div)").get()                    # '2.0'
```

Relative expressions work on element selectors:

```python
for div in sel.css("div"):
    div.xpath("./h2/text()").get()
```

Namespaces are not resolved: prefixed name tests are rejected, so match by local
name. XPath variables are rejected too. The supported subset is listed in
[compatibility.md](compatibility.md).

## Metadata

```python
sel.base_url
sel.type
sel.namespaces
```

`type="text"` keeps the input string as the selector's value; queries on it
return empty, and `.attrib` raises `AttributeError`.

## Scrapy

See [Using simdsel with Scrapy](scrapy.md) for direct use in a spider and for a
`response.selector` adapter that keeps `response.css()` working.

## Differences from parsel

- Trees follow the HTML specification (`mode="spec"`) or a flat fast parse
  (`mode="none"`), not lxml.
- Fragments and serialization can differ; see [compatibility.md](compatibility.md).
- Documents are immutable: `drop()` and `root=` raise `NotImplementedError`.
- Extensions: `.text`, `parse_many()` and `mode=`.
