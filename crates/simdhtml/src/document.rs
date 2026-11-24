use std::borrow::Cow;
use std::ops::Range;

use encoding_rs::Encoding;

use crate::character_reference::decode_character_references;
use crate::tokenizer::{AttrQuote, TagToken};

/// A node index in a flattened document.
pub type NodeId = u32;

/// Sentinel used for "no node".
pub const NO_NODE: NodeId = u32::MAX;
/// Sentinel used for "no tag".
pub const NO_TAG: u32 = u32::MAX;

/// The document node: always node `0`, and the only node without a parent.
pub const DOC_ROOT: NodeId = 0;

pub type TagId = u32;

/// A parsed HTML document. It borrows the input when that already was UTF-8 and
/// owns the transcoded bytes otherwise.
#[derive(Clone, Debug)]
pub struct Document<'a> {
    pub input: Cow<'a, [u8]>,
    pub encoding: &'static Encoding,

    /// Whether invalid bytes were replaced during decoding.
    pub had_encoding_errors: bool,

    // PERF: one `Vec` per column costs a fixed number of allocations per
    // document, which shows on small ones; a single arena carved into columns
    // would remove them.
    /// All nodes in preorder: a `kind` column plus an AoS record.
    pub node_tape: NodeTape,

    /// Tag tokens in source order, followed by the ones implied by tree
    /// construction.
    pub tags: Vec<TagToken>,

    /// Element node created for each start tag token, indexed like `tags`.
    pub tag_element: Vec<NodeId>,

    /// Adjusted tag name index per tag token (`0` = keep the source spelling).
    ///
    /// Foreign content camel-cases a fixed set of SVG names (`foreignobject`);
    /// one byte per token keeps that out of the hot columns.
    pub tag_name_override: Vec<u8>,

    /// Tag name span, one entry per tag token.  Implied tags carry a zero-length
    /// span and a name in `implied_names`.
    pub tag_name_start: Vec<u32>,
    pub tag_name_len: Vec<u16>,

    /// Byte-sized tag code per tag token, aligned with `tags` and `tag_element`
    /// (`0` = no dedicated code; compare spans).  One byte per token turns a
    /// query-side tag match into a mask scan instead of a gather over name spans.
    pub tag_code: Vec<u8>,

    // FIXME: implied tags are appended after the source tags; merging them in
    // one pass would keep tag order aligned with tree order.
    /// Number of source tag tokens; later tag indices were implied by tree
    /// construction.
    pub source_tag_count: u32,
    /// Names of the tags implied by tree construction, indexed by
    /// `tag - source_tag_count`.
    pub implied_names: Vec<&'static str>,

    // Attribute columns: name/value spans plus quoting style, one entry per
    // attribute in source order.
    pub attr_tag: Vec<TagId>,
    pub attr_name_start: Vec<u32>,
    pub attr_name_len: Vec<u16>,
    pub attr_value_start: Vec<u32>,
    pub attr_value_len: Vec<u32>,
    pub attr_quote: Vec<AttrQuote>,

    /// Adjusted attribute-name index per attribute (`0` = keep the source
    /// spelling, which [`Document::attr_name`] lowercases).  Foreign elements
    /// adjust a fixed list (`viewbox` -> `viewBox`); one byte per attribute.
    pub attr_name_override: Vec<u8>,

    /// Document diagnostics.
    pub flags: DocumentFlags,

    /// Recoverable parse errors, in document order.
    pub warnings: Vec<Warning>,
}

/// One `kind` byte plus one AoS `record` per node, in preorder: node `i`'s
/// descendants are exactly `[i + 1, subtree_end[i])`.
///
/// The split exists for bulk passes that only ask "is this an element?" and so
/// stream 1 byte per node; every other field stays in one record because
/// matching and text walking read several of them per node.
#[derive(Clone, Debug, Default)]
pub struct NodeTape {
    /// `NodeKind` discriminant plus [`RAW_TEXT_BIT`].  This is the scan column.
    pub kind: Vec<u8>,
    pub record: Vec<NodeRecord>,
}

/// Per-node fields other than [`NodeTape::kind`].
#[derive(Clone, Debug)]
pub struct NodeRecord {
    parent: NodeId,
    first_child: NodeId,
    next_sibling: NodeId,
    prev_element_sibling: NodeId,
    subtree_end: NodeId,
    depth: u16,
    span: Range<u32>,
    tag_token: TagId,
    outer_end: u32,
}

/// Applies HTML's newline preprocessing: `\r\n` and `\r` become `\n`.
#[must_use]
pub fn normalize_newlines(text: Cow<'_, str>) -> Cow<'_, str> {
    if !text.as_bytes().contains(&b'\r') {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(ch);
        }
    }
    Cow::Owned(out)
}

/// Replaces U+0000 with U+FFFD, as every HTML tokenizer state does.
///
/// The data model has no NUL: it is replaced when the character is emitted, so
/// doing it on extraction keeps the parser's spans valid and costs one byte
/// scan on documents that contain no NUL (the overwhelming majority).
#[must_use]
pub fn replace_nulls(text: Cow<'_, str>) -> Cow<'_, str> {
    if !text.as_bytes().contains(&0) {
        return text;
    }
    Cow::Owned(text.replace('\0', "\u{fffd}"))
}

/// Set on text nodes whose bytes came from rawtext/RCDATA.
pub const RAW_TEXT_BIT: u8 = 0b100;

/// Set on text nodes whose data needs input-stream newline preprocessing.
pub const NORMALIZE_NEWLINES_BIT: u8 = 0b1000_0000;

/// Set on [`NodeKind::Comment`] nodes that are doctypes.
pub const DOCTYPE_BIT: u8 = 0b0100_0000;

pub const NS_HTML: u8 = 0;
pub const NS_SVG: u8 = 1;
pub const NS_MATHML: u8 = 2;

/// A MathML text integration point (`mi`, `mo`, `mn`, `ms`, `mtext`)
/// the element is MathML, but its children belong to HTML.
pub const NS_MATH_TEXT_POINT: u8 = 3;

/// An SVG element whose children are HTML (`foreignObject`, `desc`, `title`).
pub const NS_SVG_HTML_POINT: u8 = 4;

// Namespaces live in bits 3-5 of the kind byte: no extra memory, same column.
const NS_SHIFT: u32 = 3;
const NS_MASK: u8 = 0b111 << NS_SHIFT;

const KIND_MASK: u8 = 0b011;

impl NodeKind {
    #[inline]
    const fn bits(self) -> u8 {
        match self {
            Self::Document => 0,
            Self::Element => 1,
            Self::Text => 2,
            Self::Comment => 3,
        }
    }

    #[inline]
    const fn from_bits(bits: u8) -> Self {
        match bits & KIND_MASK {
            1 => Self::Element,
            2 => Self::Text,
            3 => Self::Comment,
            _ => Self::Document,
        }
    }
}

/// Everything needed to append one node; lives on the builder stack only.
pub(crate) struct NodeRecordInit {
    pub kind: NodeKind,
    pub parent: NodeId,
    pub first_child: NodeId,
    pub next_sibling: NodeId,
    pub prev_element_sibling: NodeId,
    pub subtree_end: NodeId,
    pub depth: u16,
    pub span: Range<u32>,
    pub tag_token: TagId,
    pub outer_end: u32,
    pub raw_text: bool,
    /// Set on [`NodeKind::Comment`] nodes that came from a doctype.
    pub doctype: bool,
    /// Apply newline preprocessing to this text node's data.
    pub normalize_newlines: bool,
    /// Element namespace ([`NS_HTML`], [`NS_SVG`], [`NS_MATH`]); 0 for text.
    pub ns: u8,
}

impl NodeTape {
    /// Creates an empty store with room for `capacity` nodes.
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            kind: Vec::with_capacity(capacity),
            record: Vec::with_capacity(capacity),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.record.len()
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = Node<'_>> + ExactSizeIterator + '_ {
        (0..self.len() as NodeId).map(move |id| Node { tape: self, id })
    }

    pub(crate) fn push(&mut self, init: NodeRecordInit) -> NodeId {
        let id = self.record.len() as NodeId;
        self.kind.push(
            init.kind.bits()
                | if init.raw_text { RAW_TEXT_BIT } else { 0 }
                | if init.doctype { DOCTYPE_BIT } else { 0 }
                | if init.normalize_newlines {
                    NORMALIZE_NEWLINES_BIT
                } else {
                    0
                }
                | if init.normalize_newlines {
                    NORMALIZE_NEWLINES_BIT
                } else {
                    0
                }
                | ((init.ns << NS_SHIFT) & NS_MASK),
        );
        self.record.push(NodeRecord {
            parent: init.parent,
            first_child: init.first_child,
            next_sibling: init.next_sibling,
            prev_element_sibling: init.prev_element_sibling,
            subtree_end: init.subtree_end,
            depth: init.depth,
            span: init.span,
            tag_token: init.tag_token,
            outer_end: init.outer_end,
        });
        id
    }

    #[inline]
    fn idx(&self, id: NodeId) -> usize {
        debug_assert!((id as usize) < self.len(), "node id out of bounds");
        id as usize
    }

    /// Element namespace; [`NS_HTML`] for text and document nodes.
    #[must_use]
    pub fn namespace_of(&self, id: NodeId) -> u8 {
        (self.kind[self.idx(id)] & NS_MASK) >> NS_SHIFT
    }

    pub(crate) fn depth_of(&self, id: NodeId) -> u16 {
        self.record[self.idx(id)].depth
    }

    pub(crate) fn outer_end_of(&self, id: NodeId) -> u32 {
        self.record[self.idx(id)].outer_end
    }

    pub(crate) fn subtree_end_of(&self, id: NodeId) -> NodeId {
        self.record[self.idx(id)].subtree_end
    }

    pub(crate) fn set_subtree_end(&mut self, id: NodeId, end: NodeId) {
        let i = self.idx(id);
        self.record[i].subtree_end = end;
    }

    pub(crate) fn set_outer_end(&mut self, id: NodeId, end: u32) {
        let i = self.idx(id);
        self.record[i].outer_end = end;
    }

    pub(crate) fn set_first_child(&mut self, id: NodeId, child: NodeId) {
        let i = self.idx(id);
        self.record[i].first_child = child;
    }

    pub(crate) fn set_next_sibling(&mut self, id: NodeId, next: NodeId) {
        let i = self.idx(id);
        self.record[i].next_sibling = next;
    }

    pub(crate) fn set_prev_element_sibling(&mut self, id: NodeId, prev: NodeId) {
        let i = self.idx(id);
        self.record[i].prev_element_sibling = prev;
    }
}

/// A borrowed view of one node: two words, created on demand, one column load
/// per accessor.
#[derive(Clone, Copy, Debug)]
pub struct Node<'a> {
    tape: &'a NodeTape,
    id: NodeId,
}

impl Node<'_> {
    #[must_use]
    #[inline]
    pub fn kind(&self) -> NodeKind {
        NodeKind::from_bits(self.tape.kind[self.tape.idx(self.id)])
    }

    #[must_use]
    #[inline]
    pub fn is_element(&self) -> bool {
        self.tape.kind[self.tape.idx(self.id)] & KIND_MASK == NodeKind::Element.bits()
    }

    #[must_use]
    #[inline]
    pub fn is_text(&self) -> bool {
        self.tape.kind[self.tape.idx(self.id)] & KIND_MASK == NodeKind::Text.bits()
    }

    /// Element namespace; integration points report their foreign namespace.
    #[must_use]
    #[inline]
    pub fn namespace(&self) -> u8 {
        match self.tape.namespace_of(self.id) {
            NS_MATH_TEXT_POINT => NS_MATHML,
            NS_SVG_HTML_POINT => NS_SVG,
            other => other,
        }
    }

    /// Whether this node is a comment, PI or bogus comment.
    #[must_use]
    #[inline]
    pub fn is_comment(&self) -> bool {
        self.tape.kind[self.tape.idx(self.id)] & KIND_MASK == NodeKind::Comment.bits()
    }

    /// Whether this node is a comment that came from a doctype.
    #[must_use]
    #[inline]
    pub fn is_doctype(&self) -> bool {
        self.tape.kind[self.tape.idx(self.id)] & DOCTYPE_BIT != 0
    }

    /// Whether this text node is the content of a raw text element (never decoded).
    #[must_use]
    #[inline]
    pub fn is_raw_text(&self) -> bool {
        self.tape.kind[self.tape.idx(self.id)] & RAW_TEXT_BIT != 0
    }

    #[must_use]
    #[inline]
    pub fn parent(&self) -> NodeId {
        self.tape.record[self.tape.idx(self.id)].parent
    }

    #[must_use]
    #[inline]
    pub fn first_child(&self) -> NodeId {
        self.tape.record[self.tape.idx(self.id)].first_child
    }

    #[must_use]
    #[inline]
    pub fn next_sibling(&self) -> NodeId {
        self.tape.record[self.tape.idx(self.id)].next_sibling
    }

    #[must_use]
    #[inline]
    pub fn prev_element_sibling(&self) -> NodeId {
        self.tape.record[self.tape.idx(self.id)].prev_element_sibling
    }

    #[must_use]
    #[inline]
    pub fn subtree_end(&self) -> NodeId {
        self.tape.record[self.tape.idx(self.id)].subtree_end
    }

    #[must_use]
    #[inline]
    pub fn depth(&self) -> u16 {
        self.tape.record[self.tape.idx(self.id)].depth
    }

    #[must_use]
    #[inline]
    pub fn span(&self) -> Range<u32> {
        self.tape.record[self.tape.idx(self.id)].span.clone()
    }

    #[must_use]
    #[inline]
    pub fn tag_token(&self) -> u32 {
        self.tape.record[self.tape.idx(self.id)].tag_token
    }

    #[must_use]
    #[inline]
    pub fn outer_end(&self) -> u32 {
        self.tape.record[self.tape.idx(self.id)].outer_end
    }
}

/// Broad node category: the two low bits of the kind column.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum NodeKind {
    Document = 0,
    Element = 1,
    Text = 2,
    /// Comments, processing instructions, bogus comments and doctypes; the
    /// doctype flag lives in [`DOCTYPE_BIT`] because the category only has two
    /// bits.
    Comment = 3,
}

bitflags::bitflags! {
    /// Document-level diagnostics.
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
    pub struct DocumentFlags: u32 {
        /// An end tag did not match any open element, or an element was
        /// implicitly closed by tree construction.
        const UNEXPECTED_END_TAG = 1 << 0;

        /// The markup scan reported unbalanced tag starts/ends.
        const UNBALANCED_TAG = 1 << 1;

        /// The open-element stack exceeded the configured limit.
        const DEPTH_LIMIT = 1 << 2;

        /// The document uses a construct outside the fast path.
        const NEEDS_FALLBACK = 1 << 3;

        /// An implied end tag was emitted for a void element or an open element.
        const IMPLIED_END = 1 << 4;
    }
}

/// A non-fatal parse warning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Warning {
    pub kind: WarningKind,
    pub span: Range<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WarningKind {
    /// An end tag had no matching open element.
    UnexpectedEndTag,
}

impl Document<'_> {
    /// Borrowed view of one node.
    #[must_use]
    #[inline]
    pub fn node(&self, id: NodeId) -> Node<'_> {
        Node {
            tape: &self.node_tape,
            id,
        }
    }

    #[must_use]
    pub fn tag_name(&self, tag: u32) -> &str {
        if tag >= self.source_tag_count {
            return self.implied_names[(tag - self.source_tag_count) as usize];
        }
        if let Some(&index) = self.tag_name_override.get(tag as usize)
            && index != 0
        {
            return crate::tree_builder::adjusted_tag_name(index);
        }
        let start = self.tag_name_start[tag as usize] as usize;
        let len = self.tag_name_len[tag as usize] as usize;
        std::str::from_utf8(&self.input[start..start + len]).expect("Document input is valid UTF-8")
    }

    /// Element name as the HTML tree model stores it (ASCII lowercase, except
    /// for adjusted foreign names).
    pub fn tag_name_lower(&self, tag: u32) -> std::borrow::Cow<'_, str> {
        if tag < self.source_tag_count
            && self
                .tag_name_override
                .get(tag as usize)
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
    #[must_use]
    pub fn attr_name(&self, attr: u32) -> Cow<'_, str> {
        if let Some(&index) = self.attr_name_override.get(attr as usize)
            && index != 0
        {
            return Cow::Borrowed(crate::tree_builder::adjusted_attr_name(index));
        }
        let start = self.attr_name_start[attr as usize] as usize;
        let len = self.attr_name_len[attr as usize] as usize;
        let raw = &self.input[start..start + len];
        if raw.iter().any(|b| b.is_ascii_uppercase()) {
            Cow::Owned(String::from_utf8_lossy(raw).to_ascii_lowercase())
        } else {
            Cow::Borrowed(std::str::from_utf8(raw).expect("Document input is valid UTF-8"))
        }
    }

    /// Decoded character data of a text node or comment; raw text is verbatim.
    #[must_use]
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
                        && self.tags[tag as usize].attr_range.clone().any(|attr| {
                            let start = self.attr_name_start[attr as usize] as usize;
                            let len = self.attr_name_len[attr as usize] as usize;
                            if !self.input[start..start + len].eq_ignore_ascii_case(b"encoding") {
                                return false;
                            }
                            let vs = self.attr_value_start[attr as usize] as usize;
                            let vl = self.attr_value_len[attr as usize] as usize;
                            let value = &self.input[vs..vs + vl];
                            value.eq_ignore_ascii_case(b"text/html")
                                || value.eq_ignore_ascii_case(b"application/xhtml+xml")
                        });
                    svg_point || math_text || annotation_xml
                }
            };
            // Raw text is the tokenizer's own state, where NUL becomes U+FFFD;
            // `in body` and integration points drop it instead.
            let replace = if parent_ns == NS_HTML {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_store_is_41_bytes_per_node() {
        // `NodeKind` + `RAW_TEXT_BIT` column, plus one record per node.
        assert_eq!(1 + std::mem::size_of::<NodeRecord>(), 41);
    }
}
