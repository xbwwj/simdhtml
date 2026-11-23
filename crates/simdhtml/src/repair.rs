//! How much WHATWG tree-construction recovery to apply.
//!
//! Tree construction itself lives in [`crate::tree_builder`] and
//! [`crate::tree_builder::spec`]: the specification's insertion-mode machine
//! runs over a mutable tree, and the materialiser flattens the result into the
//! preorder tape.  There is deliberately only **one** repair implementation —
//! the standard's — because compatibility is the goal; the only other mode is
//! [`RepairMode::None`], which skips the insertion-mode machine entirely.

use std::fmt::Display;

/// How much WHATWG tree-construction recovery to apply.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RepairMode {
    /// No repair at all: every start tag opens a node, an end tag closes the
    /// current element only when its name matches, an unmatched end tag is
    /// dropped, and void elements still close immediately (void-ness is part of
    /// the element definition, not a repair).
    ///
    /// The result is a well-formed preorder tape but not the spec's tree.
    ///
    /// This is the default: callers that need the specification's tree ask for
    /// [`Self::Spec`] explicitly.
    #[default]
    None,
    /// The specification's insertion-mode machine: implied html/head/body,
    /// implied end tags, table wrappers and foster parenting, template
    /// insertion modes, and the foreign-content rules.
    Spec,
}

impl Display for RepairMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                RepairMode::None => "none",
                RepairMode::Spec => "spec",
            }
        )
    }
}
