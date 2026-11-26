# Compatibility with parsel

simdsel is parsel-compatible at the selector level, but it is not an lxml clone.
The deliberate differences are listed here.

## Tree construction

- The tree follows the HTML specification (`mode="spec"`, 98% WPT conformance),
  not libxml2: implied `<html>/<head>/<body>`, `<tbody>`, foster parenting and
  the adoption agency.
- `mode="none"` (the default) skips repair: the tree is the flat start/end-tag
  result.
- As a consequence, fragment documents can produce a different tree than parsel.
  Use `mode="spec"` when browser-shaped trees matter.

## Serialization

- `get()` on an element returns the original source slice: attribute quotes,
  entity spelling and tag case stay as written.
- parsel/lxml re-serialize the node. Text and attribute results are decoded.

## Selecting

- `::text` and `::attr(name)` are supported.
- A chained `css()` searches inside the current results. parsel expands to
  `descendant-or-self::`, so it also matches the current node; use `:scope` to
  include it in simdsel.
- Positional pseudo-classes can differ on malformed input because the trees
  differ (see above).

## XPath

- XPath 1.0 core is supported: node tests, the common axes, predicates,
  operators and the 27 core functions.
- Not supported: the `following`, `preceding` and `ancestor-or-self` axes;
  `comment()` and `processing-instruction()` node tests; variables; namespace
  prefixes (names are matched as local names).
- Text and attribute nodes become strings. Scalars are stringified like Python's
  `repr`: `count()` gives `"1.0"`, booleans give `"1"` / `"0"`.

## API

- `Selector(text=..., type="text")` keeps the string as the selector value;
  later `css()` / `xpath()` calls return empty. parsel re-parses the string as
  HTML.
- `.attrib` on a scalar result raises `AttributeError` (simdsel says
  `'str' object`, parsel says `'float'` / `'bool'`).
- Documents are immutable: `drop()` and `root=` are not supported.
- Extensions: `.text`, `parse_many()` and the `mode=` keyword.
- An empty document yields an empty value; parsel yields `<html></html>`.
