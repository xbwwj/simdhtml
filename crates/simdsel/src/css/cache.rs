use std::cell::{OnceCell, RefCell};

use rustc_hash::FxHashMap;
use simdhtml::{Document, NodeId};

use crate::bitset::BitSet;

use super::ast::Compound;

#[derive(Debug)]
pub(super) struct ChildIndex {
    pub(super) children: Vec<NodeId>,
    pub(super) child_start: Vec<u32>,
    pub(super) element_child_count: Vec<u32>,
    pub(super) element_index: Vec<u32>,
}

/// Document-derived query cache: tag/attribute/compound masks and a child index.
/// It never borrows or owns the document, so it is only valid for the document
/// it was built from.  The one-shot API creates a throwaway one per call;
/// [`crate::Selector`] shares one across a selector chain.
#[derive(Debug, Default)]
pub struct QueryCache {
    child_index: OnceCell<ChildIndex>,
    tag_masks: RefCell<FxHashMap<String, BitSet>>,
    attr_masks: RefCell<FxHashMap<String, BitSet>>,
    bare_compounds: RefCell<FxHashMap<Compound, BitSet>>,
}

impl QueryCache {
    /// Creates an empty query cache.
    #[must_use]
    pub fn root() -> Self {
        Self::default()
    }

    pub(super) fn get_tag_mask(&self, name: &str) -> Option<BitSet> {
        self.tag_masks.borrow().get(name).cloned()
    }

    pub(super) fn put_tag_mask(&self, name: String, mask: BitSet) {
        self.tag_masks.borrow_mut().insert(name, mask);
    }

    pub(super) fn get_attr_mask(&self, name: &str) -> Option<BitSet> {
        self.attr_masks.borrow().get(name).cloned()
    }

    pub(super) fn put_attr_mask(&self, name: String, mask: BitSet) {
        self.attr_masks.borrow_mut().insert(name, mask);
    }

    pub(super) fn get_compound(&self, compound: &Compound) -> Option<BitSet> {
        self.bare_compounds.borrow().get(compound).cloned()
    }

    pub(super) fn put_compound(&self, compound: Compound, mask: BitSet) {
        self.bare_compounds.borrow_mut().insert(compound, mask);
    }

    /// Children of every node, in one pass over the preorder tape: count per
    /// parent, then fill in node order, so each parent's children arrive in
    /// increasing id order.
    pub(super) fn child_index(&self, doc: &Document<'_>, n: usize) -> &ChildIndex {
        self.child_index.get_or_init(|| {
            let node = |id: u32| doc.node(id);
            let mut child_start = vec![0u32; n + 1];
            for id in 1..n as u32 {
                let parent = node(id).parent();
                if (parent as usize) < n {
                    child_start[parent as usize] += 1;
                }
            }
            // Prefix sum: counts become start offsets.
            let mut sum = 0u32;
            for start in &mut child_start[..n] {
                let count = *start;
                *start = sum;
                sum += count;
            }
            child_start[n] = sum;

            let mut children = vec![0u32; sum as usize];
            let mut cursor = child_start.clone();
            let mut element_child_count = vec![0u32; n];
            let mut element_index = vec![0u32; n];
            for id in 1..n as u32 {
                let parent = node(id).parent();
                if parent as usize >= n {
                    continue;
                }
                children[cursor[parent as usize] as usize] = id;
                cursor[parent as usize] += 1;
                if node(id).is_element() {
                    element_child_count[parent as usize] += 1;
                    element_index[id as usize] = element_child_count[parent as usize];
                }
            }

            ChildIndex {
                children,
                child_start,
                element_child_count,
                element_index,
            }
        })
    }
}
