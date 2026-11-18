//! CSS selector compilation, set-at-a-time matching, and a parsel-shaped
//! [`Selector`] facade over [`simdhtml::FlatDoc`].
//!
//! The low-level API compiles a CSS selector into an AST and evaluates it
//! against an immutable document:
//!
//! ```text
//! parse_selector -> select(&FlatDoc<'_>, &CssSelector) -> MatchRecord
//! ```
//!
//! The high-level [`Selector`] borrows a [`FlatDoc`] plus the current result
//! set, so chained `.css()` calls stay tied to the caller-owned document.

#![warn(missing_docs)]

mod bitset;
mod error;
mod matcher;
mod parser;
mod record;
mod selector;

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
pub use selector::Selector;
pub use simdhtml::{
    AttrIter, AttrRef, DocFlags, FlatDoc, MarkupFlags, MarkupIndex, NO_NODE, NO_TAG, Node, NodeId,
    NodeKind, ParseError, ParseMode, Quote, TagKind, TagToken, Warning, WarningKind,
    build_markup_index, parse,
};

/// CSS AST types under a distinct module, avoiding a name clash with the
/// high-level [`Selector`].
pub mod css {
    pub use crate::parser::*;
}
