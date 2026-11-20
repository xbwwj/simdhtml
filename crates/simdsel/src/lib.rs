//! CSS selector compilation, set-at-a-time matching, and a parsel-shaped
//! [`Selector`] facade over [`simdhtml::Document`].
//!
//! [`Selector`] is the front door of the crate: it owns its document, so a
//! caller never needs to name [`Document`] or import `simdhtml` at all.
//!
//! ```no_run
//! use simdsel::Selector;
//!
//! let sel = Selector::from_text("<div class=item><a href=/x>hi</a></div>").unwrap();
//! for link in sel.css(".item a::attr(href)").unwrap().getall() {
//!     println!("{link}");
//! }
//! ```
//!
//! The low-level API stays separate and document-borrowing: compile a CSS
//! selector into an AST and evaluate it against an immutable document, with the
//! document's lifetime under the caller's control:
//!
//! ```text
//! parse_selector -> select(&Document<'_>, &CssSelector) -> MatchRecord
//! ```
//!
//! Use it when one document feeds many independent queries and the `Rc` in
//! [`Selector`] is not wanted, or when the result is projected without building
//! a result set at all (e.g. [`Count`]).

mod bitset;
mod error;
mod matcher;
mod parser;
mod raw;
mod record;
mod selector;
pub mod xpath;

pub use bitset::BitSet;
pub use error::Error;
pub use matcher::{
    Count, Html, MatchError, Projection, QueryCache, Records, Texts, extract, select, select_as,
    select_as_within, select_as_within_cache, select_count, select_count_within, select_within,
    select_within_cache,
};
pub use parser::{
    AttrOp, AttrPred, Combinator, Complex, Compound, Pseudo, PseudoElement, RelativeSelector,
    Selector as CssSelector, SelectorError, parse_selector,
};
pub use record::{MatchKind, MatchRecord};
pub use selector::{Selector, record_attrib, record_text, select_query_within_cache};
pub use simdhtml::{
    AttrQuote, Document, DocumentFlags, EncodingMode, NO_NODE, NO_TAG, Node, NodeId, NodeKind,
    ParseError, ParseOptions, RepairMode, TagKind, TagToken, Warning, WarningKind,
};
pub use xpath::{NodeRef, RefKind, Value, XPath, XPathError};

/// CSS AST types under a distinct module, avoiding a name clash with the
/// high-level [`Selector`].
pub mod css {
    pub use crate::parser::*;
}
