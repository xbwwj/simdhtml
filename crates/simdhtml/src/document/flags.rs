use std::ops::Range;

bitflags::bitflags! {
    /// Summary flags for recoverable parse problems.
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
    pub struct DocumentFlags: u32 {
        /// At least one [`DiagnosticKind::UnexpectedEndTag`] was reported.
        const UNEXPECTED_END_TAG = 1 << 0;

        /// The markup scan reported unbalanced tag starts/ends.
        const UNBALANCED_TAG = 1 << 1;

        /// The document uses a construct outside the fast path.
        const NEEDS_FALLBACK = 1 << 2;

        /// An implied end tag was emitted for a void element or an open element.
        const IMPLIED_END = 1 << 3;

        /// A tag start was still open when the input ended.
        const EOF_IN_TAG = 1 << 4;

        /// A `<!--` comment was not terminated before EOF.
        const EOF_IN_COMMENT = 1 << 5;

        /// A `<!...>` / `<?...>` declaration was not terminated before EOF.
        const EOF_IN_DOCTYPE = 1 << 6;

        /// A raw text element (script/style/...) had no matching end tag.
        const EOF_IN_RAW_TEXT = 1 << 7;

        /// A `<![CDATA[` section was not terminated before EOF.
        const EOF_IN_CDATA = 1 << 8;
    }
}

/// One recoverable parse problem with its source span.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub span: Range<u32>,
}

/// A recoverable parse problem.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticKind {
    /// An end tag had no matching open element.
    UnexpectedEndTag,
}
