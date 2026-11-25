//! CSS selector compilation and set-at-a-time matching over [`simdhtml::Document`],
//! with a parsel-shaped [`Selector`] facade.

mod bitset;
pub mod css;
mod dom;
mod error;
mod facade;
mod record;
pub mod xpath;

pub use bitset::BitSet;
pub use css::{
    AttrOp, AttrPred, Combinator, Complex, Compound, Count, Html, MatchError, Projection, Pseudo,
    PseudoElement, QueryCache, Records, RelativeSelector, SelectorError, SelectorList, Texts,
    extract, parse_selector, root_element, select, select_as, select_as_within,
    select_as_within_cache, select_count, select_count_within, select_within, select_within_cache,
};
pub use error::Error;
pub use facade::{Selector, record_attrib, record_text, select_query_within_cache};
pub use record::{MatchKind, MatchRecord};
pub use simdhtml::{
    AttrQuote, DOC_ROOT, Diagnostic, DiagnosticKind, Document, DocumentFlags, EncodingMode,
    NO_NODE, NO_TAG, Node, NodeId, NodeKind, ParseOptions, RepairMode, TagKind, TagToken,
};
pub use xpath::{NodeRef, RefKind, Value, XPath, XPathError};
