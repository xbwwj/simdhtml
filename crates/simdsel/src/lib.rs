//! Fast CSS and XPath selection over HTML, with a [parsel]-shaped [`Selector`].
//!
//! ```
//! use simdsel::Selector;
//!
//! let html = r#"<div class="item"><h2>First &amp; best</h2><a href="/one">one</a></div>"#;
//!
//! let sel = Selector::from_text(html)?;
//! assert_eq!(sel.css(".item h2::text")?.get(), Some("First & best".into()));
//! assert_eq!(sel.css("a::attr(href)")?.get(), Some("/one".into()));
//! assert_eq!(sel.xpath("//div/@class")?.get(), Some("item".into()));
//! # Ok::<(), simdsel::Error>(())
//! ```
//!
//! [parsel]: https://github.com/scrapy/parsel
//!
//! # Selecting
//!
//! [`Selector::css`] and [`Selector::xpath`] return a new [`Selector`], so
//! queries chain. A query searches inside the current results, in document
//! order; the results themselves are not matched unless you use `:scope`.
//!
//! ```
//! # use simdsel::Selector;
//! let html = r#"<ul><li><b>1</b></li><li><b>2</b></li></ul>"#;
//! let items = Selector::from_text(html)?.css("li")?;
//!
//! assert_eq!(items.len(), 2);
//! assert_eq!(items.css("b::text")?.getall(), vec!["1", "2"]);
//! # Ok::<(), simdsel::Error>(())
//! ```
//!
//! Two pseudo-elements are part of the extraction language rather than standard
//! CSS: `::text` selects text content and `::attr(name)` selects an attribute
//! value.
//!
//! | Method | Result |
//! | --- | --- |
//! | [`get`](Selector::get) / [`getall`](Selector::getall) | first / all result strings |
//! | [`text`](Selector::text) / [`texts`](Selector::texts) | first / all text contents |
//! | [`attrib`](Selector::attrib) | attributes of the first element |
//! | [`len`](Selector::len) / [`is_empty`](Selector::is_empty) | number of results |
//! | [`split`](Selector::split) | one single-result selector per result |
//! | [`css_count`](Selector::css_count) | count without materializing results |
//!
//! For an element, `get()` is the original source slice, not a re-serialization.
//! Text and attribute results are decoded.
//!
//! # Ownership
//!
//! [`Selector`] borrows its input when the bytes are already available, and owns
//! it when they are consumed:
//!
//! | Constructor | Input | Charset |
//! | --- | --- | --- |
//! | [`from_bytes`](Selector::from_bytes) | `&[u8]`, borrowed | sniffed |
//! | [`from_text`](Selector::from_text) | `&str`, borrowed | UTF-8 |
//! | [`from_vec`](Selector::from_vec) | `Vec<u8>`, owned | sniffed |
//! | [`from_string`](Selector::from_string) | `String`, owned | UTF-8 |
//! | [`new`](Selector::new) | an already parsed [`Document`] | - |
//!
//! A [`Selector`] is reference counted and therefore neither `Send` nor `Sync`:
//! parse and select on the thread that consumes the result.
//!
//! # XPath
//!
//! [`Selector::xpath`] returns the parsel-shaped result (elements, text and
//! scalars as strings). For direct access to node-sets and typed values, compile
//! an [`XPath`] once and evaluate it against [`Selector::doc`]:
//!
//! ```
//! use simdsel::{Selector, Value, XPath};
//!
//! let sel = Selector::from_text("<p>x</p>")?;
//! let value = XPath::new("string(//p)")?.eval(sel.doc())?;
//!
//! assert!(matches!(value, Value::Str(text) if text == "x"));
//! # Ok::<(), simdsel::Error>(())
//! ```
//!
//! # Errors
//!
//! Every fallible operation returns [`Result`]. Parse, CSS and XPath failures
//! are variants of [`Error`].
//!
//! # Compatibility
//!
//! simdsel follows the HTML specification and CSS/XPath standards rather than
//! lxml's tree and serialization. The deliberate differences from parsel are
//! listed in [compatibility.md].
//!
//! [compatibility.md]: https://github.com/xbwwj/simdhtml/blob/main/crates/simdsel-py/docs/compatibility.md

mod bitset;
mod css;
mod dom;
mod record;

pub mod error;
pub mod facade;
pub mod xpath;

/// Result projections for [`Selector::css_as`](crate::Selector::css_as).
pub mod projection {
    pub use crate::css::select::{Count, Html, Projection, Texts};
}

pub use error::{Error, Result};
pub use facade::Selector;
pub use projection::{Count, Html, Projection, Texts};
pub use xpath::XPath;
pub use xpath::value::{NodeRef, RefKind, Value};

pub use simdhtml::document::Document;
pub use simdhtml::parse::{EncodingMode, ParseOptions, RepairMode};

/// The types most programs need.
pub mod prelude {
    pub use crate::projection::{Count, Html, Projection, Texts};
    pub use crate::{Document, Error, Result, Selector, Value, XPath};
}
