use std::fmt::Display;

/// How much WHATWG tree-construction recovery to apply.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RepairMode {
    /// No repair: a start tag opens a node, a matching end tag closes it, void
    /// elements close immediately, unmatched end tags are dropped.  The tape is
    /// well-formed but not the spec's tree.  This is the default.
    #[default]
    None,
    /// The specification's insertion-mode machine: implied tags, foster
    /// parenting, the adoption agency, templates and foreign content.
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
