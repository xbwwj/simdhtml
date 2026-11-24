//! CSS selector compilation and set-at-a-time matching over [`simdhtml::Document`],
//! with a parsel-shaped [`Selector`] facade.

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
    Count, Html, MatchError, Projection, QueryCache, Records, Texts, extract, root_element, select,
    select_as, select_as_within, select_as_within_cache, select_count, select_count_within,
    select_within, select_within_cache,
};
pub use parser::{
    AttrOp, AttrPred, Combinator, Complex, Compound, Pseudo, PseudoElement, RelativeSelector,
    Selector as CssSelector, SelectorError, parse_selector,
};
pub use record::{MatchKind, MatchRecord};
pub use selector::{Selector, record_attrib, record_text, select_query_within_cache};
pub use simdhtml::{
    AttrQuote, DOC_ROOT, Document, DocumentFlags, EncodingMode, NO_NODE, NO_TAG, Node, NodeId,
    NodeKind, ParseOptions, RepairMode, TagKind, TagToken, Warning, WarningKind,
};
pub use xpath::{NodeRef, RefKind, Value, XPath, XPathError};

/// CSS AST types under a distinct module, avoiding a name clash with the
/// high-level [`Selector`].
pub mod css {
    pub use crate::parser::*;
}
