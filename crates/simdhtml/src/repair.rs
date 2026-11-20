//! How much WHATWG tree-construction recovery to apply.
//!
//! Tree construction itself lives in [`crate::tree_builder`] and
//! [`crate::tree_builder::spec`]: the specification's insertion-mode machine
//! runs over a mutable tree, and the materialiser flattens the result into the
//! preorder tape.  There is deliberately only **one** *specification* repair
//! implementation — the standard's — because compatibility is the goal.
//!
//! [`RepairMode::Practical`] is not a second interpretation of the standard: it
//! is the fast stack machine plus exactly the structural repairs that decide
//! *where content lands* in table-layout markup.  It is measured, not asserted:
//! on the SWDE field-extraction workload it recovers the table skeleton that
//! selector-driven extraction depends on, at a fraction of the specification
//! path's cost (see `BENCHLOG.md`).  Anything it does not repair is documented
//! on the variant.

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
    /// The specification's machine without the formatting-element machinery.
    ///
    /// It builds the same structural tree as [`Self::Spec`] - implied
    /// `html`/`head`/`body`, implied end tags, the table skeleton, foster
    /// parenting, foreign content - but it does not maintain the list of active
    /// formatting elements, does not reconstruct them, and answers `</b>`-style
    /// end tags with the "any other end tag" rule instead of the adoption
    /// agency.
    ///
    /// That is the split the workloads ask for: field extraction addresses the
    /// structure of the document (where content lands), not the reparenting of
    /// misnested inline formatting.  For markup with no misnested formatting the
    /// tree is identical to `Spec`; where the two differ, `Practical` produces
    /// the flatter tree.
    Practical,
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
                RepairMode::Practical => "practical",
                RepairMode::Spec => "spec",
            }
        )
    }
}
