use std::ops::Range;

use super::id::{NO_NODE, NodeId, TagId};
use super::namespace::Namespace;
use super::tape::{DOCTYPE_BIT, KIND_MASK, NodeTape, RAW_TEXT_BIT};

/// A borrowed view of specific node in the document.
#[derive(Clone, Copy, Debug)]
pub struct Node<'a> {
    pub(crate) tape: &'a NodeTape,
    pub(crate) id: NodeId,
}

impl Node<'_> {
    #[must_use]
    #[inline]
    pub fn kind(&self) -> NodeKind {
        NodeKind::from_bits(self.tape.kind[self.tape.idx(self.id)])
    }

    #[must_use]
    #[inline]
    pub fn is_element(&self) -> bool {
        self.tape.kind[self.tape.idx(self.id)] & KIND_MASK == NodeKind::Element.bits()
    }

    #[must_use]
    #[inline]
    pub fn is_text(&self) -> bool {
        self.tape.kind[self.tape.idx(self.id)] & KIND_MASK == NodeKind::Text.bits()
    }

    /// Element namespace; integration points report their foreign namespace.
    #[must_use]
    #[inline]
    pub fn namespace(&self) -> Namespace {
        match self.tape.namespace_of(self.id) {
            Namespace::MATH_TEXT_POINT => Namespace::MATHML,
            Namespace::SVG_HTML_POINT => Namespace::SVG,
            other => other,
        }
    }

    /// Whether this node is a comment, PI or bogus comment.
    #[must_use]
    #[inline]
    pub fn is_comment(&self) -> bool {
        self.tape.kind[self.tape.idx(self.id)] & KIND_MASK == NodeKind::Comment.bits()
    }

    /// Whether this node is a comment that came from a doctype.
    #[must_use]
    #[inline]
    pub fn is_doctype(&self) -> bool {
        self.tape.kind[self.tape.idx(self.id)] & DOCTYPE_BIT != 0
    }

    /// Whether this text node is the content of a raw text element (never decoded).
    #[must_use]
    #[inline]
    pub fn is_raw_text(&self) -> bool {
        self.tape.kind[self.tape.idx(self.id)] & RAW_TEXT_BIT != 0
    }

    #[must_use]
    #[inline]
    pub fn parent(&self) -> NodeId {
        self.tape.record[self.tape.idx(self.id)].parent
    }

    #[must_use]
    #[inline]
    pub fn first_child(&self) -> NodeId {
        self.tape.record[self.tape.idx(self.id)].first_child
    }

    #[must_use]
    #[inline]
    pub fn next_sibling(&self) -> NodeId {
        self.tape.record[self.tape.idx(self.id)].next_sibling
    }

    #[must_use]
    pub fn prev_element_sibling(&self) -> NodeId {
        let parent = self.parent();
        if parent == NO_NODE {
            return NO_NODE;
        }
        let mut prev = NO_NODE;
        let mut child = self.tape.record[self.tape.idx(parent)].first_child;
        while child != NO_NODE && child != self.id {
            if self.tape.kind[self.tape.idx(child)] & KIND_MASK == NodeKind::Element.bits() {
                prev = child;
            }
            child = self.tape.record[self.tape.idx(child)].next_sibling;
        }
        prev
    }

    #[must_use]
    #[inline]
    pub fn subtree_end(&self) -> NodeId {
        self.tape.record[self.tape.idx(self.id)].subtree_end
    }

    #[must_use]
    #[inline]
    pub fn span(&self) -> Range<u32> {
        self.tape.record[self.tape.idx(self.id)].span.clone()
    }

    #[must_use]
    #[inline]
    pub fn tag_token(&self) -> TagId {
        self.tape.record[self.tape.idx(self.id)].tag_token
    }

    #[must_use]
    #[inline]
    pub fn outer_end(&self) -> u32 {
        self.tape.record[self.tape.idx(self.id)].outer_end
    }
}

/// Broad node category: the two low bits of the kind column.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum NodeKind {
    Document = 0,
    Element = 1,
    Text = 2,
    /// Comments, processing instructions, bogus comments and doctypes; the
    /// doctype flag lives in [`DOCTYPE_BIT`] because the category only has two
    /// bits.
    Comment = 3,
}

impl NodeKind {
    #[inline]
    pub const fn bits(self) -> u8 {
        match self {
            Self::Document => 0,
            Self::Element => 1,
            Self::Text => 2,
            Self::Comment => 3,
        }
    }

    #[inline]
    pub(crate) const fn from_bits(bits: u8) -> Self {
        match bits & KIND_MASK {
            1 => Self::Element,
            2 => Self::Text,
            3 => Self::Comment,
            _ => Self::Document,
        }
    }
}
