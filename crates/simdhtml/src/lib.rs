//! Fast read-only HTML parsing into a preorder [`FlatDoc`].
//!
//! [`build_markup_index`] is stage-1: it only records tag/text/raw positions.
//! [`parse`] runs stage-2: tokenizer, normalizer, and the preorder [`FlatDoc`]
//! builder.

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

mod document;
mod entities;
mod flat;
mod markup;
mod normalize;
mod raw;
pub mod simd;
mod stage1;
mod tagcode;
mod tokenizer;

use std::fmt;

pub use entities::decode_html_entities;
pub use flat::{
    AttrIter, AttrRef, DocFlags, FlatDoc, Node, NodeKind, Quote, TagKind, TagToken, Warning,
    WarningKind,
};
pub use markup::{MarkupFlags, MarkupIndex};
pub use stage1::{Stage1Error, build_markup_index};

/// Benchmark-only wrapper around the raw region scan.
///
/// Returns `(span_count, flags)` so the microbenchmark can time Pass A without
/// exposing [`crate::raw`] types. Enabled by the `bench-internal` feature.
#[cfg(feature = "bench-internal")]
#[doc(hidden)]
#[must_use]
pub fn bench_scan_raw_spans(input: &[u8]) -> (usize, MarkupFlags) {
    let (spans, flags) = raw::scan_raw_spans(input);
    (spans.len(), flags)
}

/// A node index in a flattened document.
pub type NodeId = u32;

/// Sentinel used for "no node".
pub const NO_NODE: NodeId = u32::MAX;

/// Sentinel used for "no tag".
pub const NO_TAG: u32 = u32::MAX;

/// How strictly [`parse`] treats constructs outside the fast path.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ParseMode {
    /// Best effort: keep the flattened document and expose diagnostics in
    /// [`FlatDoc::flags`].  This is the mode the benchmark and selector use.
    #[default]
    Fast,
    /// If the fast path reports a structural problem, request the html5ever
    /// fallback.  The fallback itself is a later milestone; for now this
    /// returns [`ParseError::FallbackRequired`].
    Safe,
    /// Reject structural problems instead of recovering.
    Strict,
}

/// Fatal parse errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseError {
    /// Input exceeds the `u32` span limit.
    InputTooLarge,
    /// Input is not valid UTF-8.
    InvalidUtf8(simd::Utf8Error),
    /// [`ParseMode::Safe`] would need the html5ever fallback.
    FallbackRequired(DocFlags),
    /// [`ParseMode::Strict`] rejected the document.
    Malformed(DocFlags),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputTooLarge => write!(f, "input is larger than u32::MAX bytes"),
            Self::InvalidUtf8(err) => write!(f, "invalid utf-8: {err}"),
            Self::FallbackRequired(flags) => {
                write!(f, "document requires fallback (flags: {flags:?})")
            }
            Self::Malformed(flags) => write!(f, "malformed document (flags: {flags:?})"),
        }
    }
}

impl std::error::Error for ParseError {}

impl From<Stage1Error> for ParseError {
    fn from(_: Stage1Error) -> Self {
        Self::InputTooLarge
    }
}

/// Parses `input` into a flattened document.
///
/// # Errors
///
/// Returns a [`ParseError`] for non-UTF-8/oversized input, or when
/// [`ParseMode::Safe`]/[`ParseMode::Strict`] rejects the document.
pub fn parse(input: &[u8], mode: ParseMode) -> Result<FlatDoc<'_>, ParseError> {
    simd::validate_utf8(input).map_err(ParseError::InvalidUtf8)?;

    let index = build_markup_index(input)?;
    let tokens = tokenizer::tokenize(input, &index);
    let doc = document::build(input, &index, tokens);

    let risky = doc.flags.intersects(
        DocFlags::NEEDS_FALLBACK
            | DocFlags::MISNESTED
            | DocFlags::UNBALANCED_TAG
            | DocFlags::DEPTH_LIMIT,
    );

    match mode {
        ParseMode::Fast => Ok(doc),
        ParseMode::Safe if risky => Err(ParseError::FallbackRequired(doc.flags)),
        ParseMode::Strict if risky => Err(ParseError::Malformed(doc.flags)),
        ParseMode::Safe | ParseMode::Strict => Ok(doc),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(input: &'static str) -> FlatDoc<'static> {
        parse(input.as_bytes(), ParseMode::Fast).unwrap()
    }

    fn elements(doc: &FlatDoc<'_>) -> Vec<String> {
        doc.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.is_element())
            .map(|(i, n)| {
                format!(
                    "{}:{}:{}..{}:end{}:depth{}",
                    i,
                    doc.tag_name(n.tag_token),
                    n.span.start,
                    n.span.end,
                    n.subtree_end,
                    n.depth
                )
            })
            .collect()
    }

    #[test]
    fn simple_tree_shape() {
        let d = doc("<div><p>hi</p><p>yo</p></div>");
        assert_eq!(
            elements(&d),
            vec![
                "1:div:0..5:end6:depth1",
                "2:p:5..8:end4:depth2",
                "4:p:14..17:end6:depth2",
            ]
        );
        assert_eq!(d.text_content(1), "hiyo");
        assert_eq!(d.outer_html(1), b"<div><p>hi</p><p>yo</p></div>");
        assert_eq!(d.outer_html(2), b"<p>hi</p>");
        assert_eq!(d.text_of(3), "hi");
    }

    #[test]
    fn void_elements_and_optional_end_tags() {
        let d = doc("<ul><li>a<li>b</ul><br>");
        let names: Vec<_> = d
            .nodes
            .iter()
            .filter(|n| n.is_element())
            .map(|n| d.tag_name(n.tag_token).to_string())
            .collect();
        assert_eq!(names, vec!["ul", "li", "li", "br"]);
        assert!(d.flags.contains(DocFlags::SYNTHETIC_END));

        let br = d
            .nodes
            .iter()
            .position(|n| n.is_element() && d.tag_name(n.tag_token) == "br")
            .unwrap();
        assert_eq!(d.outer_html(br as NodeId), b"<br>");
    }

    #[test]
    fn attributes_and_entities_are_lazy() {
        let d = doc(r#"<a href="/x?a=1&amp;b=2" title="a &lt; b" disabled>x</a>"#);
        let a = 1;
        let attrs: Vec<_> = d.attr_iter(a).collect();
        assert_eq!(attrs.len(), 3);
        assert_eq!(&*d.attr_value(&attrs[0]), "/x?a=1&b=2");
        assert_eq!(&*d.attr_value(&attrs[1]), "a < b");
        assert_eq!(attrs[2].quote, Quote::Boolean);
        assert_eq!(&*d.text_of(2), "x");
    }

    #[test]
    fn duplicate_attributes_keep_first_wins() {
        let d = doc(r#"<a href="first" HREF="second">x</a>"#);
        assert_eq!(d.tags[0].attrs.len(), 2);
        let attr = d.find_attr_index(1, "href").expect("href");
        assert_eq!(&*d.attr_decoded_value(attr), "first");
    }

    #[test]
    fn raw_text_is_not_entity_decoded() {
        let d = doc("<script>a &amp; b</script><p>x &amp; y</p>");
        let script_text = d.nodes.iter().position(|n| n.is_text()).unwrap();
        assert_eq!(&*d.text_of(script_text as NodeId), "a &amp; b");
        let p_text = d.nodes.iter().rposition(|n| n.is_text()).unwrap();
        assert_eq!(&*d.text_of(p_text as NodeId), "x & y");
    }

    #[test]
    fn implied_end_outer_html_is_original() {
        let d = doc("<ul><li>a<li>b</ul>");
        let first_li = 2;
        assert_eq!(d.outer_html(first_li), b"<li>a");
        let ul = 1;
        assert_eq!(d.outer_html(ul), b"<ul><li>a<li>b</ul>");
    }

    #[test]
    fn safe_and_strict_request_fallback() {
        let err = parse(b"</div>", ParseMode::Safe).unwrap_err();
        assert!(matches!(err, ParseError::FallbackRequired(_)));
        let err = parse(b"</div>", ParseMode::Strict).unwrap_err();
        assert!(matches!(err, ParseError::Malformed(_)));
        assert!(parse(b"<div></div>", ParseMode::Safe).is_ok());
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        assert!(matches!(
            parse(b"<p>\xFF</p>", ParseMode::Fast),
            Err(ParseError::InvalidUtf8(_))
        ));
    }
}
