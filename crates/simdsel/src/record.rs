//! One selected item.

use std::ops::Range;

use simdhtml::NodeId;

/// One selected item. `::text` / `::attr()` need this even in v1.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MatchRecord {
    /// The node the record came from.
    pub node: NodeId,
    /// Record kind.
    pub kind: MatchKind,
    /// Source span (element/text) or attribute value span.
    pub span: Range<u32>,
    /// One past the element's outer HTML; equals `span.end` for text/attr.
    pub outer_end: u32,
    /// Attribute value span for `::attr()`.
    pub attr: Option<Range<u32>>,
}

/// What a match record refers to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MatchKind {
    /// An element node.
    Element,
    /// A text node (from `::text`).
    Text,
    /// An attribute value (from `::attr()`).
    Attr,
}
