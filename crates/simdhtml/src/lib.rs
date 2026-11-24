//! HTML parsing into a preorder [`Document`].
//!
//! Pipeline:
//!
//! 1. markup
//! 2. tokenizer
//! 3. tree_builder

mod character_reference;
mod document;
pub mod encoding;
mod error;
pub mod markup;
pub mod simd;
mod tag_code;
pub mod token_data;
mod tokenizer;
mod tree_builder;

pub use crate::character_reference::decode_character_references;
pub use crate::document::{
    Document, DocumentFlags, NS_HTML, NS_MATH_TEXT_POINT, NS_MATHML, NS_SVG, NS_SVG_HTML_POINT,
    Node, NodeKind, Warning, WarningKind,
};
pub use crate::document::{normalize_newlines, replace_nulls};
pub use crate::error::{ParseError, Result};
pub use crate::markup::{MarkupDiagnostics, MarkupError, MarkupIndex, OpaqueRegion, RegionKind};
pub use crate::tag_code::code_of as tag_code_of;
pub use crate::tokenizer::{AttrQuote, TagKind, TagToken};

/// A node index in a flattened document.
pub type NodeId = u32;

/// Sentinel used for "no node".
pub const NO_NODE: NodeId = u32::MAX;
/// Sentinel used for "no tag".
pub const NO_TAG: u32 = u32::MAX;

/// Where the parser gets its UTF-8 bytes from.
///
/// The input to a parser is *bytes*, not text: the specification starts with
/// the encoding sniffing algorithm, and roughly half of the real web is not
/// UTF-8.  This enum is the choice between running that algorithm
/// ([`Self::Sniff`]), trusting a charset the caller already knows
/// ([`Self::Explicit`]) and trusting the caller that the bytes already are
/// UTF-8 ([`Self::Utf8`]).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EncodingMode {
    /// The specification's sniffing algorithm (see [`encoding`]): byte order
    /// mark, then `<meta charset>` in the first 1024 bytes, then UTF-8 if the
    /// bytes validate and windows-1252 otherwise.
    #[default]
    Sniff,
    /// The caller already knows the charset: an HTTP `Content-Type` header, or
    /// an explicit `encoding=` argument in a binding.  A byte order mark still
    /// wins; `<meta charset>` declarations no longer do.  Bytes that are
    /// invalid in `encoding` are replaced and reported through
    /// [`Document::had_encoding_errors`].
    ///
    /// [`encoding::lookup`] turns a label such as `"gbk"` into the `Encoding`.
    Explicit(&'static encoding_rs::Encoding),
    /// The input already is UTF-8: the document borrows it unchanged, and
    /// bytes that are not UTF-8 are an error instead of being read as some
    /// legacy encoding.
    Utf8,
}

/// How much WHATWG tree-construction recovery to apply.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RepairMode {
    /// For most web crawling load, there is no need to repair HTML,
    /// the default [`RepairMode::None`] is sufficient.
    #[default]
    None,
    /// Strictly follows the HTML spec's insertion-mode machine, including
    /// implied tags, foster parenting, the adoption agency, templates and
    /// foreign content.
    Spec,
}

/// Everything that changes how bytes become a [`Document`].
///
/// The pipeline has two independent knobs - how far tree construction follows
/// the specification, and where the UTF-8 bytes come from - and the parser used
/// to expose one function per combination.  They are collected here instead, so
/// [`Document::parse_with`] is the only entry point that takes arguments.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ParseOptions {
    /// How much tree-construction recovery to apply.
    pub repair: RepairMode,
    /// How to obtain UTF-8 bytes from the input.
    pub encoding: EncodingMode,
}

impl ParseOptions {
    /// The defaults: [`RepairMode::None`] and [`EncodingMode::Sniff`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            repair: RepairMode::None,
            encoding: EncodingMode::Sniff,
        }
    }

    /// Sets [`Self::repair`].
    #[must_use]
    pub const fn repair(mut self, repair: RepairMode) -> Self {
        self.repair = repair;
        self
    }

    /// Sets [`Self::encoding`].
    #[must_use]
    pub const fn encoding(mut self, encoding: EncodingMode) -> Self {
        self.encoding = encoding;
        self
    }
}

impl<'a> Document<'a> {
    /// Parses `input` into a flattened document with default [`ParseOptions`].
    ///
    /// The input is *bytes*, not a string: the encoding is sniffed and decoded
    /// per the HTML specification (see [`encoding`]).
    ///
    /// The default [`RepairMode`] is [`RepairMode::None`]: no implied tags and
    /// no adoption-agency machinery.  Ask for [`RepairMode::Spec`] through
    /// [`Document::parse_with`] when the WHATWG tree is required.
    ///
    /// # Errors
    ///
    /// [`ParseError::InputTooLarge`] for oversized input.
    pub fn parse(input: &'a [u8]) -> Result<Self> {
        Self::parse_with(input, ParseOptions::new())
    }

    /// Parses `input` with explicit [`ParseOptions`].
    ///
    /// [`RepairMode::None`] skips HTML error recovery entirely: the tree is the
    /// flat result of "a start tag opens, a matching end tag closes", which is
    /// cheaper and is what a caller gets to choose when the markup is already
    /// well-formed (or when it wants the unrepaired nesting).
    ///
    /// # Errors
    ///
    /// [`ParseError::InvalidUtf8`] when [`EncodingMode::Utf8`] was asked for
    /// and the bytes are not UTF-8, and [`ParseError::InputTooLarge`] for
    /// oversized input.
    pub fn parse_with(input: &'a [u8], options: ParseOptions) -> Result<Self> {
        let decoded = match options.encoding {
            EncodingMode::Sniff => encoding::decode(input),
            EncodingMode::Explicit(encoding) => encoding::decode_with(input, encoding),
            EncodingMode::Utf8 => {
                encoding::validate_utf8(input).map_err(ParseError::InvalidUtf8)?;
                encoding::Decoded {
                    bytes: std::borrow::Cow::Borrowed(input),
                    encoding: encoding_rs::UTF_8,
                    errors: false,
                }
            }
        };
        build(decoded, options.repair)
    }
}

impl Document<'static> {
    /// Parses `input`, taking ownership of it.
    ///
    /// This is [`Document::parse`] for a buffer the caller would otherwise have
    /// to keep alive next to the document (a fetched response body, or the
    /// result of a binding's argument conversion).
    ///
    /// Transcoding a legacy encoding still allocates; what this avoids is the
    /// document borrowing the caller's buffer, which would tie it to a local
    /// and stop [`crate::Document`] from being returned from a function.
    ///
    /// # Errors
    ///
    /// [`ParseError::InputTooLarge`] for oversized input.
    pub fn parse_owned(input: Vec<u8>) -> Result<Self> {
        Self::parse_owned_with(input, ParseOptions::new())
    }

    /// [`Document::parse_owned`] with explicit [`ParseOptions`].
    ///
    /// # Errors
    ///
    /// [`ParseError::InvalidUtf8`] when [`EncodingMode::Utf8`] was asked for
    /// and the bytes are not UTF-8, and [`ParseError::InputTooLarge`] for
    /// oversized input.
    pub fn parse_owned_with(input: Vec<u8>, options: ParseOptions) -> Result<Self> {
        let decoded = match options.encoding {
            EncodingMode::Sniff => encoding::decode_owned(input),
            EncodingMode::Explicit(encoding) => encoding::decode_with_owned(input, encoding),
            EncodingMode::Utf8 => {
                encoding::validate_utf8(&input).map_err(ParseError::InvalidUtf8)?;
                encoding::Decoded {
                    bytes: std::borrow::Cow::Owned(input),
                    encoding: encoding_rs::UTF_8,
                    errors: false,
                }
            }
        };
        build(decoded, options.repair)
    }
}

fn build(decoded: encoding::Decoded<'_>, repair: RepairMode) -> Result<Document<'_>> {
    let index = markup::scan(&decoded.bytes)?;
    let tokens = tokenizer::tokenize(&decoded.bytes, &index);
    let mut doc = tree_builder::build_with(decoded.bytes, &index, tokens, repair);
    doc.encoding = decoded.encoding;
    doc.had_encoding_errors = decoded.errors;
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeId;

    fn doc(input: &'static str) -> Document<'static> {
        Document::parse_with(
            input.as_bytes(),
            ParseOptions::new().repair(RepairMode::Spec),
        )
        .unwrap()
    }

    /// Outer HTML bytes of a node: element tags or character data.
    fn outer<'a>(doc: &'a Document<'_>, id: NodeId) -> &'a [u8] {
        let node = doc.node(id);
        match node.kind() {
            NodeKind::Document => &doc.input,
            NodeKind::Element | NodeKind::Text | NodeKind::Comment => {
                let span = node.span();
                &doc.input[span.start as usize..node.outer_end() as usize]
            }
        }
    }

    /// Concatenated character data of a subtree.
    fn text_content(doc: &Document<'_>, id: NodeId) -> String {
        let node = doc.node(id);
        if node.is_text() {
            return doc.text(id).into_owned();
        }
        let mut out = String::new();
        for child in id..node.subtree_end() {
            if doc.node(child).is_text() {
                out.push_str(&doc.text(child));
            }
        }
        out
    }

    fn elements(doc: &Document<'_>) -> Vec<String> {
        doc.node_tape
            .iter()
            .enumerate()
            .filter(|(_, n)| n.is_element())
            .map(|(i, n)| {
                format!(
                    "{}:{}:{}..{}:end{}:depth{}",
                    i,
                    doc.tag_name(n.tag_token()),
                    n.span().start,
                    n.span().end,
                    n.subtree_end(),
                    n.depth()
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
                "1:html:0..0:end9:depth1",
                "2:head:0..0:end3:depth2",
                "3:body:0..0:end9:depth2",
                "4:div:0..5:end9:depth3",
                "5:p:5..8:end7:depth4",
                "7:p:14..17:end9:depth4",
            ]
        );
        assert_eq!(text_content(&d, 4), "hiyo");
        assert_eq!(outer(&d, 4), b"<div><p>hi</p><p>yo</p></div>");
        assert_eq!(outer(&d, 5), b"<p>hi</p>");
        assert_eq!(d.text(6), "hi");
    }

    #[test]
    fn void_elements_and_optional_end_tags() {
        let d = doc("<ul><li>a<li>b</ul><br>");
        let names: Vec<_> = d
            .node_tape
            .iter()
            .filter(|n| n.is_element())
            .map(|n| d.tag_name(n.tag_token()).to_string())
            .collect();
        assert_eq!(names, vec!["html", "head", "body", "ul", "li", "li", "br"]);
        assert!(d.flags.contains(DocumentFlags::IMPLIED_END));

        let br = d
            .node_tape
            .iter()
            .position(|n| n.is_element() && d.tag_name(n.tag_token()) == "br")
            .unwrap();
        assert_eq!(outer(&d, br as NodeId), b"<br>");
    }

    #[test]
    fn attributes_and_entities_are_lazy() {
        let d = doc(r#"<a href="/x?a=1&amp;b=2" title="a &lt; b" disabled>x</a>"#);
        let anchor = d
            .node_tape
            .iter()
            .position(|n| n.is_element() && d.tag_name(n.tag_token()) == "a")
            .expect("anchor");
        let tag = d.node(anchor as NodeId).tag_token();
        let range = d.tags[tag as usize].attr_range.clone();
        assert_eq!(range.len(), 3);
        let value = |a: u32| {
            let start = d.attr_value_start[a as usize] as usize;
            let end = start + d.attr_value_len[a as usize] as usize;
            let raw = std::str::from_utf8(&d.input[start..end]).unwrap();
            decode_character_references(raw, true).into_owned()
        };
        assert_eq!(value(range.start), "/x?a=1&b=2");
        assert_eq!(value(range.start + 1), "a < b");
        assert_eq!(d.attr_quote[range.start as usize + 2], AttrQuote::Empty);
        let text = d.node_tape.iter().position(|n| n.is_text()).expect("text");
        assert_eq!(&*d.text(text as NodeId), "x");
    }

    #[test]
    fn duplicate_attributes_keep_first_wins() {
        let d = doc(r#"<a href="first" HREF="second">x</a>"#);
        let range = d.tags[0].attr_range.clone();
        assert_eq!(range.len(), 2);
        let name = |a: u32| {
            let start = d.attr_name_start[a as usize] as usize;
            let end = start + d.attr_name_len[a as usize] as usize;
            &d.input[start..end]
        };
        let first = range
            .clone()
            .find(|&a| name(a).eq_ignore_ascii_case(b"href"))
            .expect("href");
        assert_eq!(first, range.start);
    }

    #[test]
    fn raw_text_is_not_entity_decoded() {
        let d = doc("<script>a &amp; b</script><p>x &amp; y</p>");
        let script_text = d.node_tape.iter().position(|n| n.is_text()).unwrap();
        assert_eq!(&*d.text(script_text as NodeId), "a &amp; b");
        let p_text = d.node_tape.iter().rposition(|n| n.is_text()).unwrap();
        assert_eq!(&*d.text(p_text as NodeId), "x & y");
    }

    #[test]
    fn implied_end_outer_html_is_original() {
        let d = doc("<ul><li>a<li>b</ul>");
        let find = |name: &str| {
            d.node_tape
                .iter()
                .position(|n| n.is_element() && d.tag_name(n.tag_token()) == name)
                .expect("element") as NodeId
        };
        assert_eq!(outer(&d, find("li")), b"<li>a");
        assert_eq!(outer(&d, find("ul")), b"<ul><li>a<li>b</ul>");
    }

    #[test]
    fn escapable_raw_text_decodes_character_references() {
        let d = doc("<title>a &amp; b</title><script>x &amp; y</script>");
        let texts: Vec<_> = d
            .node_tape
            .iter()
            .enumerate()
            .filter(|(_, n)| n.is_text())
            .map(|(i, _)| d.text(i as NodeId).into_owned())
            .collect();
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0], "a & b", "title is RCDATA: decoded");
        assert_eq!(texts[1], "x &amp; y", "script is raw text: verbatim");
    }

    #[test]
    fn default_repair_mode_is_none() {
        assert_eq!(RepairMode::default(), RepairMode::None);
        let none = Document::parse(b"<p>a").unwrap();
        let spec =
            Document::parse_with(b"<p>a", ParseOptions::new().repair(RepairMode::Spec)).unwrap();
        assert!(none.node_tape.len() < spec.node_tape.len());
        assert!(
            !none
                .node_tape
                .iter()
                .any(|n| n.is_element() && none.tag_name(n.tag_token()) == "html")
        );
        assert!(
            spec.node_tape
                .iter()
                .any(|n| n.is_element() && spec.tag_name(n.tag_token()) == "html")
        );
    }

    #[test]
    fn invalid_utf8_is_rejected_by_utf8_mode() {
        assert!(matches!(
            Document::parse_with(
                b"<p>\xFF</p>",
                ParseOptions::new().encoding(EncodingMode::Utf8)
            ),
            Err(ParseError::InvalidUtf8(_))
        ));
    }
}
