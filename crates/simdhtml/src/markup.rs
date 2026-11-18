//! Stage-1 markup index.
//!
//! The stage-1 scanner only records *where* the markup is, never the per-block
//! bitmaps that were used to find it.

use std::ops::Range;

bitflags::bitflags! {
    /// Diagnostics produced by stage-1.
    ///
    /// Stage-1 is deliberately permissive: malformed input still produces an
    /// index, and the flags tell later stages (or the caller) whether recovery
    /// or a full fallback parser is needed.
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
    pub struct MarkupFlags: u32 {
        /// The number of tag starts and tag ends differs, or they are not in
        /// ascending/alternating order.
        const UNBALANCED_TAG = 1 << 0;

        /// A tag start was still open when the input ended.
        const UNCLOSED_TAG = 1 << 1;

        /// A `<!--` comment was not terminated before EOF.
        const UNCLOSED_COMMENT = 1 << 2;

        /// A `<!...>` / `<?...>` declaration was not terminated before EOF.
        const UNCLOSED_DECLARATION = 1 << 3;

        /// A rawtext element (script/style/...) had no matching end tag.
        const UNCLOSED_RAWTEXT = 1 << 4;

        /// A block failed the bit-parallel fixed point and was repaired with
        /// the candidate-only scalar state machine.
        const SCALAR_REPAIR = 1 << 5;

        /// The document contains a construct outside the v1 fast-path scope
        /// (CDATA, SVG/MathML, `<template>`, ...).
        const NEEDS_FALLBACK = 1 << 6;
    }
}

/// Stage-1 output: tag positions and a few text/raw intervals.
///
/// No bitmaps are kept.  For a typical HTML page the vectors hold on the order
/// of a few percent of the input bytes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MarkupIndex {
    /// Byte offset of each tag's `<`, in ascending order.
    pub tag_starts: Vec<u32>,

    /// Byte offset of each tag's `>`, in ascending order.
    ///
    /// This is the *closing* `>` of the tag, not one past it.
    pub tag_ends: Vec<u32>,

    /// Maximal byte ranges that are plain text (not inside a tag, comment,
    /// doctype, processing instruction or CDATA section).
    ///
    /// Rawtext *content* (script/style/textarea/...) is a text run; the raw
    /// element's start/end tags are excluded as usual.
    pub text_runs: Vec<Range<u32>>,

    /// Byte ranges masked from tag scanning: comments, declarations and
    /// rawtext content.  Kept for diagnostics and for the tokenizer, which
    /// needs to know which text is raw.
    pub raw_ranges: Vec<Range<u32>>,

    /// Diagnostics from stage-1.
    pub flags: MarkupFlags,
}

impl MarkupIndex {
    /// Number of tags found (tag starts).
    #[must_use]
    pub fn tag_count(&self) -> usize {
        self.tag_starts.len()
    }

    /// Whether the tag stream is balanced according to stage-1's cheap check.
    #[must_use]
    pub fn is_balanced(&self) -> bool {
        !self.flags.contains(MarkupFlags::UNBALANCED_TAG)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_are_composable() {
        let mut flags = MarkupFlags::UNBALANCED_TAG | MarkupFlags::UNCLOSED_TAG;
        assert!(flags.contains(MarkupFlags::UNCLOSED_TAG));
        assert!(!flags.contains(MarkupFlags::NEEDS_FALLBACK));
        flags.insert(MarkupFlags::NEEDS_FALLBACK);
        assert!(flags.contains(MarkupFlags::NEEDS_FALLBACK));
        flags.remove(MarkupFlags::UNBALANCED_TAG);
        assert!(!flags.contains(MarkupFlags::UNBALANCED_TAG));
    }

    #[test]
    fn empty_index_is_balanced() {
        let index = MarkupIndex::default();
        assert_eq!(index.tag_count(), 0);
        assert!(index.is_balanced());
    }
}
