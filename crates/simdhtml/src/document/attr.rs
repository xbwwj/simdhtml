use super::id::TagId;
use crate::tokenizer::AttrQuote;

/// Attribute spans and their owning tag, in source order.
#[derive(Clone, Debug, Default)]
pub struct AttrTable {
    pub tag: Vec<TagId>,
    pub name_start: Vec<u32>,
    pub name_len: Vec<u16>,
    pub value_start: Vec<u32>,
    pub value_len: Vec<u32>,
    pub quote: Vec<AttrQuote>,

    /// Adjusted attribute-name index per attribute (`0` = keep the source
    /// spelling, which [`super::Document::attr_name`] lowercases).  Foreign
    /// elements adjust a fixed list (`viewbox` -> `viewBox`); one byte per
    /// attribute.
    pub name_override: Vec<u8>,
}

impl AttrTable {
    pub fn len(&self) -> usize {
        self.tag.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tag.is_empty()
    }
}
