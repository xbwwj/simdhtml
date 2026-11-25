use std::ops::Range;

use super::id::{NodeId, TagId};
use super::namespace::{NS_MASK, NS_SHIFT, Namespace};
use super::node::{Node, NodeKind};

/// Set on text nodes whose bytes came from rawtext/RCDATA.
pub(crate) const RAW_TEXT_BIT: u8 = 0b100;

/// Set on text nodes whose data needs input-stream newline preprocessing.
pub(crate) const NORMALIZE_NEWLINES_BIT: u8 = 0b1000_0000;

/// Set on [`NodeKind::Comment`] nodes that are doctypes.
pub(crate) const DOCTYPE_BIT: u8 = 0b0100_0000;

pub(crate) const KIND_MASK: u8 = 0b011;

/// One `kind` byte plus one AoS `record` per node, in preorder: node `i`'s
/// descendants are exactly `[i + 1, subtree_end[i])`.
///
/// The split exists for bulk passes that only ask "is this an element?" and so
/// stream 1 byte per node; every other field stays in one record because
/// matching and text walking read several of them per node.
#[derive(Clone, Debug, Default)]
pub struct NodeTape {
    /// `NodeKind` discriminant plus [`RAW_TEXT_BIT`].  This is the scan column.
    pub kind: Vec<u8>,
    pub record: Vec<NodeRecord>,
}

impl NodeTape {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            kind: Vec::with_capacity(capacity),
            record: Vec::with_capacity(capacity),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.record.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.record.is_empty()
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = Node<'_>> + ExactSizeIterator + '_ {
        (0..self.len()).map(move |id| Node {
            tape: self,
            id: NodeId::new(id as u32),
        })
    }

    pub(crate) fn push(&mut self, init: NodeRecordInit) -> NodeId {
        let id = NodeId::new(self.record.len() as u32);
        self.kind.push(
            init.kind.bits()
                | if init.raw_text { RAW_TEXT_BIT } else { 0 }
                | if init.doctype { DOCTYPE_BIT } else { 0 }
                | if init.normalize_newlines {
                    NORMALIZE_NEWLINES_BIT
                } else {
                    0
                }
                | ((init.ns.bits() << NS_SHIFT) & NS_MASK),
        );
        self.record.push(NodeRecord {
            parent: init.parent,
            first_child: init.first_child,
            next_sibling: init.next_sibling,
            subtree_end: init.subtree_end,
            span: init.span,
            tag_token: init.tag_token,
            outer_end: init.outer_end,
        });
        id
    }

    #[inline]
    pub(crate) fn idx(&self, id: NodeId) -> usize {
        debug_assert!(id.index() < self.len(), "node id out of bounds");
        id.index()
    }

    /// Element namespace; [`Namespace::HTML`] for text and document nodes.
    #[must_use]
    pub fn namespace_of(&self, id: NodeId) -> Namespace {
        Namespace::from_bits((self.kind[self.idx(id)] & NS_MASK) >> NS_SHIFT)
    }

    pub(crate) fn outer_end_of(&self, id: NodeId) -> u32 {
        self.record[self.idx(id)].outer_end
    }

    pub(crate) fn subtree_end_of(&self, id: NodeId) -> NodeId {
        self.record[self.idx(id)].subtree_end
    }

    pub(crate) fn set_subtree_end(&mut self, id: NodeId, end: NodeId) {
        let i = self.idx(id);
        self.record[i].subtree_end = end;
    }

    pub(crate) fn set_outer_end(&mut self, id: NodeId, end: u32) {
        let i = self.idx(id);
        self.record[i].outer_end = end;
    }

    pub(crate) fn set_first_child(&mut self, id: NodeId, child: NodeId) {
        let i = self.idx(id);
        self.record[i].first_child = child;
    }

    pub(crate) fn set_next_sibling(&mut self, id: NodeId, next: NodeId) {
        let i = self.idx(id);
        self.record[i].next_sibling = next;
    }
}

/// Per-node fields other than [`NodeTape::kind`].
#[derive(Clone, Debug)]
pub struct NodeRecord {
    pub(crate) parent: NodeId,
    pub(crate) first_child: NodeId,
    pub(crate) next_sibling: NodeId,
    pub(crate) subtree_end: NodeId,
    pub(crate) span: Range<u32>,
    pub(crate) tag_token: TagId,
    pub(crate) outer_end: u32,
}

/// Everything needed to append one node; lives on the builder stack only.
pub(crate) struct NodeRecordInit {
    pub kind: NodeKind,
    pub parent: NodeId,
    pub first_child: NodeId,
    pub next_sibling: NodeId,
    pub subtree_end: NodeId,
    pub span: Range<u32>,
    pub tag_token: TagId,
    pub outer_end: u32,
    pub raw_text: bool,
    /// Set on [`NodeKind::Comment`] nodes that came from a doctype.
    pub doctype: bool,
    /// Apply newline preprocessing to this text node's data.
    pub normalize_newlines: bool,
    /// Element namespace.
    pub ns: Namespace,
}
