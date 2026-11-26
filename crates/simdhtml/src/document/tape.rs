use std::ops::Range;

use super::id::{NodeId, TagId};
use super::namespace::{NS_MASK, NS_SHIFT, Namespace};
use super::node::{Node, NodeKind};

/// Set on text nodes whose bytes came from rawtext/RCDATA.
pub(crate) const RAW_TEXT_BIT: u8 = 0b100;

/// Set on [`NodeKind::Comment`] nodes that are doctypes.
pub(crate) const DOCTYPE_BIT: u8 = 0b0100_0000;

pub(crate) const KIND_MASK: u8 = 0b011;

/// The `NodeTape` is a mixed SoA/AoS preorder recording of all the nodes in a document.
///
/// Preorder allows us to inspect a node `i` and its descendants with a dense scan range
/// `[i, subtree_end[i]]`.
///
/// Mixed SoA/AoS: the `kind` field is pulled out individually, because for bulk passes we
/// only ask "is this an element", while other fields keep staying together because matching
/// and tree walking read several of them per node--the data locality overwhelms.
#[derive(Clone, Debug, Default)]
pub struct NodeTape {
    /// `NodeKind` discriminant plus the raw-text flag.
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

    pub fn len(&self) -> usize {
        self.record.len()
    }

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
                | ((init.ns.bits() << NS_SHIFT) & NS_MASK),
        );
        self.record.push(NodeRecord {
            parent: init.parent,
            first_child: NodeId::NONE,
            next_sibling: NodeId::NONE,
            subtree_end: if matches!(init.kind, NodeKind::Text | NodeKind::Comment) {
                NodeId::new(id.get() + 1)
            } else {
                NodeId::NONE
            },
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
    kind: NodeKind,
    parent: NodeId,
    span: Range<u32>,
    tag_token: TagId,
    outer_end: u32,
    raw_text: bool,
    /// Set on [`NodeKind::Comment`] nodes that came from a doctype.
    doctype: bool,
    /// Element namespace.
    ns: Namespace,
}

impl NodeRecordInit {
    pub(crate) fn document(input_len: u32) -> Self {
        Self {
            kind: NodeKind::Document,
            ns: Namespace::HTML,
            parent: NodeId::NONE,
            span: 0..0,
            tag_token: TagId::NONE,
            outer_end: input_len,
            raw_text: false,
            doctype: false,
        }
    }

    pub(crate) fn element(
        span: Range<u32>,
        tag_token: TagId,
        ns: Namespace,
        parent: NodeId,
    ) -> Self {
        Self {
            kind: NodeKind::Element,
            ns,
            parent,
            span,
            tag_token,
            outer_end: 0,
            raw_text: false,
            doctype: false,
        }
    }

    pub(crate) fn text(span: Range<u32>, parent: NodeId, raw_text: bool) -> Self {
        Self {
            kind: NodeKind::Text,
            ns: Namespace::HTML,
            parent,
            outer_end: span.end,
            span,
            tag_token: TagId::NONE,
            raw_text,
            doctype: false,
        }
    }

    pub(crate) fn comment(span: Range<u32>, parent: NodeId, doctype: bool) -> Self {
        Self {
            kind: NodeKind::Comment,
            ns: Namespace::HTML,
            parent,
            outer_end: span.end,
            span,
            tag_token: TagId::NONE,
            raw_text: false,
            doctype,
        }
    }
}
