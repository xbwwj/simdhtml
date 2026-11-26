# Compatibility with Parsel

simdsel supports the same selector API as Parsel, so most existing CSS and XPath queries should work without changes. However, it does not try to reproduce every detail of Parsel or lxml. Here are the differences worth knowing about.

## HTML tree construction

simdsel and Parsel build their HTML trees differently.

- By default, `mode="none"` skips HTML tree repair and keeps the tree close to the parsed start and end tags.
- Set `mode="spec"` to follow the HTML specification. This includes inserting missing elements such as `<html>`, `<head>`, `<body>` and `<tbody>`, as well as handling foster parenting and the adoption agency algorithm. This mode achieves 98% WPT conformance.
- Because of these differences, malformed HTML and HTML fragments may produce different results from Parsel. Use `mode="spec"` when you need a tree closer to what a browser would build.

## HTML serialization

Calling `get()` on an element returns its original HTML from the input, rather than serializing the element again.

This means the original tag casing, attribute quotes and entity spelling are preserved. Parsel and lxml serialize the element, which may change its formatting. Text and attribute values, however, are decoded in both libraries.

One exception: when repairing has to create the same element twice (the adoption agency does this), the extra copy has no source of its own, so `get()` returns the range of the tag it was copied from.

## CSS selectors

simdsel supports the `::text` and `::attr(name)` extensions familiar from Parsel.

There is one difference when chaining selectors: in simdsel, `css()` searches within the current results, but does not automatically match the current element itself. Parsel includes the current element through its `descendant-or-self::` behavior. Use `:scope` when you also want to match the current element in simdsel.

Positional pseudo-classes may also return different results for malformed HTML, since the two libraries can build different trees.

## XPath support

simdsel supports the XPath 1.0 core, including node tests, common axes, predicates, operators and all 27 core functions.

Some features are not currently supported:

- The `following`, `preceding`, `ancestor-or-self` and `namespace` axes.
- The `comment()` and `processing-instruction()` node tests.
- Variables. Namespace prefixes are rejected too: write the local name (`rect`, not `svg:rect`).

Text and attribute nodes are returned as strings. Scalar results are also converted to strings. For example, `count()` returns `"1.0"`, while boolean results are `"1"` or `"0"`.

## API differences

A few parts of the API behave differently from Parsel:

- `Selector(text=..., type="text")` treats the input as a text value. Subsequent `css()` and `xpath()` calls return empty results. Parsel instead parses the string as HTML.
- Accessing `.attrib` on a scalar result raises `AttributeError`. The error message differs: simdsel reports `'str' object`, while Parsel reports `'float'` or `'bool'`.
- Documents are immutable. `drop()` and assigning to `root` are not supported.
- simdsel also provides a few extensions: `.text`, `parse_many()` and the `mode=` argument.
- An empty input produces an empty result. Parsel instead creates an empty `<html></html>` element.
