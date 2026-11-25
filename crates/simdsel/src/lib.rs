//! CSS selector compilation and set-at-a-time matching over [`simdhtml::document::Document`],
//! with a parsel-shaped [`facade::Selector`].

pub mod bitset;
pub mod css;
mod dom;
pub mod error;
pub mod facade;
pub mod record;
pub mod xpath;

pub mod prelude {
    pub use crate::bitset::BitSet;
    pub use crate::css::ast::{
        AttrOp, AttrPred, Combinator, Complex, Compound, Pseudo, PseudoElement, RelativeSelector,
        SelectorList,
    };
    pub use crate::css::cache::QueryCache;
    pub use crate::css::eval::root_element;
    pub use crate::css::parser::parse_selector;
    pub use crate::css::select::{
        Count, Html, Projection, Records, Texts, extract, select, select_as, select_as_within,
        select_as_within_cache, select_count, select_count_within, select_within,
        select_within_cache,
    };
    pub use crate::error::{CssErrorKind, Error, XPathErrorKind};
    pub use crate::facade::{Selector, record_attrib, record_text, select_query_within_cache};
    pub use crate::record::{MatchKind, MatchRecord};
    pub use crate::xpath::XPath;
    pub use crate::xpath::value::{NodeRef, RefKind, Value};
    pub use simdhtml::document::Document;
    pub use simdhtml::document::flags::{Diagnostic, DiagnosticKind, DocumentFlags};
    pub use simdhtml::document::id::{DOC_ROOT, NO_ATTR, NO_NODE, NO_TAG, NodeId, TagId};
    pub use simdhtml::document::namespace::Namespace;
    pub use simdhtml::document::node::{Node, NodeKind};
    pub use simdhtml::parse::{EncodingMode, ParseOptions, RepairMode};
    pub use simdhtml::tokenizer::{AttrQuote, TagKind, TagToken};
}
