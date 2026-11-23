use std::ops::Range;

use simdhtml::NodeId;

/// One selected item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MatchRecord {
    pub node: NodeId,
    pub kind: MatchKind,
    pub span: Range<u32>,
    pub outer_end: u32,
    pub attr: Option<Range<u32>>,
}

/// What a match record refers to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MatchKind {
    Element,
    Text,
    Attr,
}
