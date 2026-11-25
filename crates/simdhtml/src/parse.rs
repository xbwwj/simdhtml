use encoding_rs::Encoding;

use crate::document::Document;
use crate::error::{ParseError, Result};
use crate::{encoding, markup, tokenizer, tree_builder};

/// Everything that changes how bytes become a [`Document`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ParseOptions {
    pub repair: RepairMode,
    pub encoding: EncodingMode,
}

/// How the input bytes are turned into UTF-8.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EncodingMode {
    /// Sniff first 1024 bytes, decide using BOM and `<meta>`.
    #[default]
    Sniff,

    /// Caller already knows the charset (e.g. from Content-Type).
    /// BOM will still win, `<meta>` is ignored.
    Explicit(&'static Encoding),

    /// The input is already UTF-8. Non UTF-8 bytes now become an error.
    Utf8,
}

/// How much effort to recover malformed HTMLs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RepairMode {
    /// For most web crawling load, there is no need to repair HTML,
    /// the default [`RepairMode::None`] is sufficient.
    #[default]
    None,

    /// HTML spec's tree-construction algorith, including implied tags,
    /// foster parenting, the adoption agency, templates and foreign content.
    Spec,
}

impl<'a> Document<'a> {
    /// Parses `input` into a flattened document with default [`ParseOptions`].
    pub fn parse(input: &'a [u8]) -> Result<Self> {
        Self::parse_with(input, ParseOptions::default())
    }

    /// Parses `input` with explicit [`ParseOptions`].
    pub fn parse_with(input: &'a [u8], options: ParseOptions) -> Result<Self> {
        let decoded = match options.encoding {
            EncodingMode::Sniff => encoding::decode(input),
            EncodingMode::Explicit(encoding) => encoding::decode_with(input, encoding),
            EncodingMode::Utf8 => {
                encoding::validate_utf8(input).map_err(|_| ParseError::InvalidUtf8)?;
                encoding::Decoded {
                    bytes: std::borrow::Cow::Borrowed(input),
                    encoding: encoding_rs::UTF_8,
                    errors: false,
                }
            }
        };
        parse_inner(decoded, options.repair)
    }
}

impl Document<'static> {
    /// Parses `input`, taking ownership of it.
    pub fn parse_owned(input: Vec<u8>) -> Result<Self> {
        Self::parse_owned_with(input, ParseOptions::default())
    }

    pub fn parse_owned_with(input: Vec<u8>, options: ParseOptions) -> Result<Self> {
        let decoded = match options.encoding {
            EncodingMode::Sniff => encoding::decode_owned(input),
            EncodingMode::Explicit(encoding) => encoding::decode_with_owned(input, encoding),
            EncodingMode::Utf8 => {
                encoding::validate_utf8(&input).map_err(|_| ParseError::InvalidUtf8)?;
                encoding::Decoded {
                    bytes: std::borrow::Cow::Owned(input),
                    encoding: encoding_rs::UTF_8,
                    errors: false,
                }
            }
        };
        parse_inner(decoded, options.repair)
    }
}

fn parse_inner(decoded: encoding::Decoded<'_>, repair: RepairMode) -> Result<Document<'_>> {
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
    use crate::character_reference::decode_character_references;
    use crate::document::flags::DocumentFlags;
    use crate::document::id::NodeId;
    use crate::document::namespace::Namespace;
    use crate::document::node::NodeKind;
    use crate::tokenizer::AttrQuote;

    fn doc(input: &'static str) -> Document<'static> {
        Document::parse_with(
            input.as_bytes(),
            ParseOptions {
                repair: RepairMode::Spec,
                ..Default::default()
            },
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
        for child in id.get()..node.subtree_end().get() {
            let child = NodeId::new(child);
            if doc.node(child).is_text() {
                out.push_str(&doc.text(child));
            }
        }
        out
    }

    fn elements(doc: &Document<'_>) -> Vec<String> {
        doc.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.is_element())
            .map(|(i, n)| {
                format!(
                    "{}:{}:{}..{}:end{}",
                    i,
                    doc.tag_name(n.tag_token()),
                    n.span().start,
                    n.span().end,
                    n.subtree_end()
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
                "1:html:0..0:end9",
                "2:head:0..0:end3",
                "3:body:0..0:end9",
                "4:div:0..5:end9",
                "5:p:5..8:end7",
                "7:p:14..17:end9",
            ]
        );
        assert_eq!(text_content(&d, NodeId::new(4)), "hiyo");
        assert_eq!(outer(&d, NodeId::new(4)), b"<div><p>hi</p><p>yo</p></div>");
        assert_eq!(outer(&d, NodeId::new(5)), b"<p>hi</p>");
        assert_eq!(d.text(NodeId::new(6)), "hi");
    }

    #[test]
    fn void_elements_and_optional_end_tags() {
        let d = doc("<ul><li>a<li>b</ul><br>");
        let names: Vec<_> = d
            .nodes
            .iter()
            .filter(|n| n.is_element())
            .map(|n| d.tag_name(n.tag_token()).to_string())
            .collect();
        assert_eq!(names, vec!["html", "head", "body", "ul", "li", "li", "br"]);
        assert!(d.flags.contains(DocumentFlags::IMPLIED_END));

        let br = d
            .nodes
            .iter()
            .position(|n| n.is_element() && d.tag_name(n.tag_token()) == "br")
            .unwrap();
        assert_eq!(outer(&d, NodeId::new(br as u32)), b"<br>");
    }

    #[test]
    fn attributes_and_entities_are_lazy() {
        let d = doc(r#"<a href="/x?a=1&amp;b=2" title="a &lt; b" disabled>x</a>"#);
        let anchor = d
            .nodes
            .iter()
            .position(|n| n.is_element() && d.tag_name(n.tag_token()) == "a")
            .expect("anchor");
        let tag = d.node(NodeId::new(anchor as u32)).tag_token();
        let range = d.tags.token(tag).attr_range.clone();
        assert_eq!(range.len(), 3);
        let value = |a: u32| {
            let start = d.attrs.value_start[a as usize] as usize;
            let end = start + d.attrs.value_len[a as usize] as usize;
            let raw = std::str::from_utf8(&d.input[start..end]).unwrap();
            decode_character_references(raw, true).into_owned()
        };
        assert_eq!(value(range.start), "/x?a=1&b=2");
        assert_eq!(value(range.start + 1), "a < b");
        assert_eq!(d.attrs.quote[range.start as usize + 2], AttrQuote::Empty);
        let text = d.nodes.iter().position(|n| n.is_text()).expect("text");
        assert_eq!(&*d.text(NodeId::new(text as u32)), "x");
    }

    #[test]
    fn duplicate_attributes_keep_first_wins() {
        let d = doc(r#"<a href="first" HREF="second">x</a>"#);
        let range = d.tags.tokens[0].attr_range.clone();
        assert_eq!(range.len(), 2);
        let name = |a: u32| {
            let start = d.attrs.name_start[a as usize] as usize;
            let end = start + d.attrs.name_len[a as usize] as usize;
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
        let script_text = d.nodes.iter().position(|n| n.is_text()).unwrap();
        assert_eq!(&*d.text(NodeId::new(script_text as u32)), "a &amp; b");
        let p_text = d.nodes.iter().rposition(|n| n.is_text()).unwrap();
        assert_eq!(&*d.text(NodeId::new(p_text as u32)), "x & y");
    }

    #[test]
    fn implied_end_outer_html_is_original() {
        let d = doc("<ul><li>a<li>b</ul>");
        let find = |name: &str| {
            d.nodes
                .iter()
                .position(|n| n.is_element() && d.tag_name(n.tag_token()) == name)
                .map(|id| NodeId::new(id as u32))
                .expect("element")
        };
        assert_eq!(outer(&d, find("li")), b"<li>a");
        assert_eq!(outer(&d, find("ul")), b"<ul><li>a<li>b</ul>");
    }

    #[test]
    fn escapable_raw_text_decodes_character_references() {
        let d = doc("<title>a &amp; b</title><script>x &amp; y</script>");
        let texts: Vec<_> = d
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.is_text())
            .map(|(i, _)| d.text(NodeId::new(i as u32)).into_owned())
            .collect();
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0], "a & b", "title is RCDATA: decoded");
        assert_eq!(texts[1], "x &amp; y", "script is raw text: verbatim");
    }

    #[test]
    fn default_repair_mode_is_none() {
        assert_eq!(RepairMode::default(), RepairMode::None);
        let none = Document::parse(b"<p>a").unwrap();
        let spec = Document::parse_with(
            b"<p>a",
            ParseOptions {
                repair: RepairMode::Spec,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(none.nodes.len() < spec.nodes.len());
        assert!(
            !none
                .nodes
                .iter()
                .any(|n| n.is_element() && none.tag_name(n.tag_token()) == "html")
        );
        assert!(
            spec.nodes
                .iter()
                .any(|n| n.is_element() && spec.tag_name(n.tag_token()) == "html")
        );
    }

    #[test]
    fn invalid_utf8_is_rejected_by_utf8_mode() {
        assert!(matches!(
            Document::parse_with(
                b"<p>\xFF</p>",
                ParseOptions {
                    encoding: EncodingMode::Utf8,
                    ..Default::default()
                },
            ),
            Err(ParseError::InvalidUtf8)
        ));
    }

    #[test]
    fn ns_html_is_used_by_default() {
        let d = doc("<p>x");
        assert_eq!(d.node(NodeId::new(1)).namespace(), Namespace::HTML);
    }
}
