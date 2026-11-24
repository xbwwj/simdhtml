# simdhtml

A SIMD-accelerated HTML parser and CSS selector.

- Rust crates: [`simdhtml`](./crates/simdhtml) (parser) and
  [`simdsel`](./crates/simdsel) (CSS/XPath selection).
- Python binding: [`simdsel-py`](./crates/simdsel-py).

## Performance results

The following figures show how fast simdhtml extracts data from HTML. The task is to extract main content from most webpages and extract structured data from specified sites. The bench currently runs on an AMD Ryzen 5600 processor with single core. You can reproduce the result on your device with `make bench`.

[![Main content and link extraction: Common Crawl, parse + 18 CSS selectors](./figures/main-content-and-link-extraction.svg)](./figures/main-content-and-link-extraction.svg)

[![Structured data extraction: SWDE, per-site class selectors](./figures/structured-data-extraction.svg)](./figures/structured-data-extraction.svg)

We also reached 98% HTML compliance in `RepairMode::Spec`, which is the
`simdsel (repair)` row.

## Usage

### Python

```python
from simdsel import Selector

html = """<div class="item" data-id="1"><h2>First &amp; best</h2><a href="/one">one</a></div>
<div class="item" data-id="2"><h2>Second</h2><a href="/two">two</a></div>"""

sel = Selector(text=html)

print(sel.css(".item h2::text").getall())       # ['First & best', 'Second']
print(sel.css(".item h2::text").get())          # 'First & best'
print(sel.css(".item a::attr(href)").getall())  # ['/one', '/two']
print(sel.xpath("//div[@data-id='2']/h2/text()").get())  # 'Second'

for item in sel.css(".item"):
    print(item.attrib["data-id"], item.css("h2::text").get())
```

### Rust

```rust
use simdsel::{Selector, Value, XPath};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let html = r#"<div class="item" data-id="1"><h2>First &amp; best</h2><a href="/one">one</a></div>
<div class="item" data-id="2"><h2>Second</h2><a href="/two">two</a></div>"#;

    let root = Selector::from_text(html)?;      // or from_vec(body) for owned bytes

    let titles = root.css(".item h2::text")?;
    println!("{:?}", titles.getall());          // ["First & best", "Second"]
    println!("{:?}", titles.get());             // Some("First & best")

    let links = root.css(".item a::attr(href)")?;
    println!("{:?}", links.getall());           // ["/one", "/two"]

    for record in root.css(".item")?.records() {
        println!("{:?}", simdsel::record_attrib(root.doc(), record));
    }

    if let Value::Str(id) = XPath::new("string(//div[2]/@data-id)")?.eval(root.doc())? {
        println!("id={id}");                    // id=2
    }

    Ok(())
}
```


## Development

`make` is the only entry point; `make` alone lists the targets.

```bash
make data          # fetch raw inputs: Common Crawl WARC + SWDE submodule (once)
make extract-data  # unpack SWDE and build its index (once, ~1 min, 8 GiB disk)
make test          # Rust tests
make test-wpt      # html5lib tree-construction conformance
make test-corpus   # Common Crawl preorder-invariant test
make bench         # full Criterion run: simdhtml/simdsel + html5ever/scraper/dom_query + Python
make bench-quick   # sampled run for iteration (no plots)
make check         # cargo fmt --check + cargo clippy
make info          # current mode, data and binaries
```

Benchmarks stream the corpora instead of loading them: Common Crawl is read
straight from the `.warc.gz`, SWDE from the index that `make extract-data`
writes, so a quick run keeps ~100 documents in memory instead of 8 GiB.

`make bundle` builds a tarball with the prebuilt bench binaries, this
`Makefile`, and the raw corpora (`.warc.gz` + `.7z`) for a machine without a
Rust toolchain:

```bash
make bundle                       # target/bundle/simdhtml-bundle-<stamp>.tar

# on the target machine (needs make + p7zip):
tar -xf simdhtml-bundle-*.tar && cd simdhtml-bundle-*/
make data           # no-op: the raw inputs are in the tarball
make extract-data   # unpack SWDE once
make bench-quick    # runs bin/*, never cargo
```
