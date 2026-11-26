use std::borrow::Cow;

use encoding_rs::Encoding;

use crate::character_reference::decode_character_references;

pub mod attr;
pub mod flags;
pub mod id;
pub mod namespace;
pub mod node;
pub mod tag;
pub mod tape;
pub mod text;

use self::attr::AttrTable;
use self::flags::{Diagnostic, DocumentFlags};
use self::id::{NO_TAG, NodeId, TagId};
use self::namespace::Namespace;
use self::node::Node;
use self::tag::TagTable;
use self::tape::NodeTape;
use self::text::{normalize_newlines, replace_nulls};

/// A parsed HTML document.
///
/// It borrows the input when that already was UTF-8 and owns the transcoded bytes otherwise.
#[derive(Clone, Debug)]
pub struct Document<'a> {
    pub input: Cow<'a, [u8]>,
    pub encoding: &'static Encoding,

    /// Whether invalid bytes were replaced during decoding.
    pub had_encoding_errors: bool,

    /// A tape recording of all nodes, see [`NodeTape`] for optimization details.
    pub nodes: NodeTape,

    pub tags: TagTable,
    pub attrs: AttrTable,

    /// Summary bits for recoverable parse problems.
    pub flags: DocumentFlags,

    /// Recoverable parse problems, in document order.  Carries source spans;
    /// [`Self::flags`] carries the O(1) summary.
    pub diagnostics: Vec<Diagnostic>,
}

impl Document<'_> {
    /// Borrowed view of one node.
    #[inline]
    pub fn node(&self, id: NodeId) -> Node<'_> {
        Node {
            tape: &self.nodes,
            id,
        }
    }

    pub fn tag_name(&self, tag: TagId) -> &str {
        if tag.get() >= self.tags.source_count {
            return self.tags.implied_names[(tag.get() - self.tags.source_count) as usize];
        }
        if let Some(&index) = self.tags.name_override.get(tag.index())
            && index != 0
        {
            return crate::tree_builder::adjusted_tag_name(index);
        }
        let start = self.tags.name_start[tag.index()] as usize;
        let len = self.tags.name_len[tag.index()] as usize;
        std::str::from_utf8(&self.input[start..start + len]).expect("Document input is valid UTF-8")
    }

    /// Element name as the HTML tree model stores it (ASCII lowercase, except
    /// for adjusted foreign names).
    pub fn tag_name_lower(&self, tag: TagId) -> std::borrow::Cow<'_, str> {
        if tag.get() < self.tags.source_count
            && self
                .tags
                .name_override
                .get(tag.index())
                .is_some_and(|&index| index != 0)
        {
            // Adjusted foreign names keep their specified case.
            return std::borrow::Cow::Borrowed(self.tag_name(tag));
        }
        let name = self.tag_name(tag);
        if name.bytes().any(|b| b.is_ascii_uppercase()) {
            std::borrow::Cow::Owned(name.to_ascii_lowercase())
        } else {
            std::borrow::Cow::Borrowed(name)
        }
    }

    /// Attribute name as the HTML tree model stores it (ASCII lowercase, except
    /// for adjusted foreign names).
    pub fn attr_name(&self, attr: u32) -> Cow<'_, str> {
        if let Some(&index) = self.attrs.name_override.get(attr as usize)
            && index != 0
        {
            return Cow::Borrowed(crate::tree_builder::adjusted_attr_name(index));
        }
        let start = self.attrs.name_start[attr as usize] as usize;
        let len = self.attrs.name_len[attr as usize] as usize;
        let raw = &self.input[start..start + len];
        if raw.iter().any(|b| b.is_ascii_uppercase()) {
            Cow::Owned(String::from_utf8_lossy(raw).to_ascii_lowercase())
        } else {
            Cow::Borrowed(std::str::from_utf8(raw).expect("Document input is valid UTF-8"))
        }
    }

    /// Decoded character data of a text node or comment; raw text is verbatim.
    pub fn text(&self, id: NodeId) -> Cow<'_, str> {
        let node = self.node(id);
        debug_assert!(node.is_text() || node.is_comment());
        let span = node.span();
        let raw = std::str::from_utf8(&self.input[span.start as usize..span.end as usize])
            .expect("Document input is valid UTF-8");
        if node.is_comment() {
            return normalize_newlines(replace_nulls(Cow::Borrowed(raw)));
        }
        // Input preprocessing runs before character references are decoded:
        // `&#x0D;` keeps its CR while a literal CR byte becomes LF.
        let raw = normalize_newlines(Cow::Borrowed(raw));
        let decoded = if node.is_raw_text() {
            raw
        } else {
            Cow::Owned(decode_character_references(&raw, false).into_owned())
        };
        if decoded.contains('\0') {
            // WHATWG tree construction drops NUL bytes under an integration
            // point and replaces them elsewhere; the WPT trees also want a
            // replacement in raw text (`<svg><foreignObject>a\0b` -> `"ab"`).
            let parent = self.node(self.node(id).parent());
            let parent_ns = parent.namespace();
            let at_point = match parent.tag_token() {
                NO_TAG => false,
                tag => {
                    let name = self.tag_name_lower(tag).to_ascii_lowercase();
                    let svg_point = matches!(name.as_ref(), "foreignobject" | "desc" | "title");
                    let math_text = matches!(name.as_ref(), "mi" | "mo" | "mn" | "ms" | "mtext");
                    // `annotation-xml` is an integration point only with an
                    // HTML `encoding`.
                    let annotation_xml = name.as_str() == "annotation-xml"
                        && self.tags.token(tag).attr_range.clone().any(|attr| {
                            let start = self.attrs.name_start[attr as usize] as usize;
                            let len = self.attrs.name_len[attr as usize] as usize;
                            if !self.input[start..start + len].eq_ignore_ascii_case(b"encoding") {
                                return false;
                            }
                            let vs = self.attrs.value_start[attr as usize] as usize;
                            let vl = self.attrs.value_len[attr as usize] as usize;
                            let value = &self.input[vs..vs + vl];
                            value.eq_ignore_ascii_case(b"text/html")
                                || value.eq_ignore_ascii_case(b"application/xhtml+xml")
                        });
                    svg_point || math_text || annotation_xml
                }
            };
            // Raw text is the tokenizer's own state, where NUL becomes U+FFFD;
            // `in body` and integration points drop it instead.
            let replace = if parent_ns == Namespace::HTML {
                node.is_raw_text()
            } else {
                !at_point
            };
            if replace {
                Cow::Owned(replace_nulls(decoded).into_owned())
            } else {
                Cow::Owned(decoded.chars().filter(|c| *c != '\0').collect())
            }
        } else {
            decoded
        }
    }
}
