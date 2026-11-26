use super::id::{NodeId, TagId};
use crate::tokenizer::TagToken;

/// Tag tokens and the parallel columns that point at them.
#[derive(Clone, Debug, Default)]
pub struct TagTable {
    /// Tag tokens in source order, followed by the ones implied by tree
    /// construction.
    pub tokens: Vec<TagToken>,

    /// Element node created for each start tag token, indexed like `tokens`.
    pub element: Vec<NodeId>,

    /// Adjusted tag name index per tag token (`0` = keep the source spelling).
    ///
    /// Foreign content camel-cases a fixed set of SVG names (`foreignobject`);
    /// one byte per token keeps that out of the hot columns.
    pub name_override: Vec<u8>,

    /// Tag name span, one entry per tag token.  Implied tags carry a zero-length
    /// span and a name in `implied_names`.
    pub name_start: Vec<u32>,
    pub name_len: Vec<u16>,

    /// Byte-sized tag code per tag token, aligned with `tokens` and `element`
    /// (`0` = no dedicated code; compare spans).  One byte per token turns a
    /// query-side tag match into a mask scan instead of a gather over name spans.
    pub code: Vec<u8>,

    /// Number of source tag tokens; later tag indices were implied by tree
    /// construction.
    pub source_count: u32,

    /// Names of the tags implied by tree construction, indexed by
    /// `tag - source_count`.
    pub implied_names: Vec<&'static str>,
}

impl TagTable {
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    #[inline]
    pub fn token(&self, tag: TagId) -> &TagToken {
        &self.tokens[tag.index()]
    }
}
