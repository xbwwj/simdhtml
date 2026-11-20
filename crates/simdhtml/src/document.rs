//! The flat document: one preorder node tape plus the token/attribute columns.
//!
//! Nodes are stored in **tree order** (preorder): node `i`'s descendants are
//! exactly `[i + 1, subtree_end[i])`, which is what makes the set-at-a-time
//! selector possible.  There is no pointer tree and no per-node allocation for
//! children: only a few `Vec`s of plain integers.
//!
//! Every column is `pub`.  Consumers (and the selector) index the columns
//! directly; the only methods kept here are the ones that have to interpret
//! runtime state: [`Document::node`], [`Document::tag_name`] and
//! [`Document::text`].

use std::borrow::Cow;
use std::ops::Range;

use crate::NodeId;
use crate::character_reference::decode_character_references;
use crate::tokenizer::{AttrQuote, TagToken};

/// The document borrows the original UTF-8 input for its whole lifetime when
/// the document already was UTF-8, and owns the transcoded bytes otherwise.
/// Either way parsing is zero-copy *after* that point: spans index into `input`.
#[derive(Clone, Debug)]
pub struct Document<'a> {
    /// UTF-8 document bytes: the caller's buffer, or the transcoded text when
    /// [`crate::Document::parse`] had to decode a legacy encoding.
    pub input: Cow<'a, [u8]>,

    /// The encoding [`crate::Document::parse`] used (UTF-8 when nothing had to change).
    pub encoding: &'static encoding_rs::Encoding,

    /// Whether decoding had to replace bytes that were invalid in `encoding`.
    pub had_encoding_errors: bool,

    // PERF: one `Vec` per column (kind + record + tags + 6 attr columns) means a
    // fixed allocation cost per document that shows up on small documents.
    // A single arena carved into columns would remove it.
    /// All nodes in **preorder**: a `kind` column plus an AoS record.
    pub node_tape: NodeTape,

    /// Tag tokens in source order, followed by the ones implied by tree
    /// construction (table parents).
    pub tags: Vec<TagToken>,

    /// Element node created for each start tag token, indexed like `tags`.
    pub tag_element: Vec<NodeId>,

    /// Adjusted tag name index per tag token (`0` = keep the source spelling).
    ///
    /// Foreign content adjusts a fixed set of SVG names to camelCase
    /// (`foreignobject` -> `foreignObject`); one byte per token keeps that out of
    /// the hot columns.
    pub tag_name_override: Vec<u8>,

    /// Tag name span, one entry per tag token.  Implied tags carry a
    /// zero-length span; their names are in `implied_names`.
    pub tag_name_start: Vec<u32>,
    pub tag_name_len: Vec<u16>,

    /// Byte-sized tag code per tag token, aligned with `tags` and
    /// `tag_element` (`0` = the name has no dedicated code; compare spans).
    ///
    /// This is the cheapest useful form of "persist the classification": the
    /// tokenizer already had to fold and classify every name to decide
    /// void/implied-end rules, and one byte per token (0.4 % of the input on
    /// the commoncrawl corpus) turns a query-side tag match into a dense
    /// `cmpeq`+`movemask` scan instead of a gather over name spans.
    pub tag_code: Vec<u8>,

    // FIXME: implied tags are currently appended after the source tags.  They
    // should be merged in one pass (prefix sum over a bitmap) so that tag order
    // stays strictly aligned with tree order.
    /// Number of source tag tokens; later tag indices were implied by tree
    /// construction.
    pub source_tag_count: u32,
    /// Names of the tags implied by tree construction, indexed by
    /// `tag - source_tag_count`.
    pub implied_names: Vec<&'static str>,

    // Attribute columns: name/value spans plus quoting style, one entry per
    // attribute in source order.
    pub attr_tag: Vec<u32>,
    pub attr_name_start: Vec<u32>,
    pub attr_name_len: Vec<u16>,
    pub attr_value_start: Vec<u32>,
    pub attr_value_len: Vec<u32>,
    pub attr_quote: Vec<AttrQuote>,

    /// Adjusted attribute-name index per attribute (`0` = keep the source
    /// spelling, which [`Document::attr_name`] lowercases).
    ///
    /// The specification adjusts a fixed list of names on foreign elements
    /// (`viewbox` -> `viewBox`, `definitionurl` -> `definitionURL`); one byte
    /// per attribute keeps that out of the hot columns, like
    /// [`Self::tag_name_override`].
    pub attr_name_override: Vec<u8>,

    /// The Document node (always 0).
    pub root: NodeId,

    /// Document diagnostics.
    pub flags: DocumentFlags,

    /// Recoverable parse errors, in document order.
    pub warnings: Vec<Warning>,
}

/// Node tape: one `kind` column plus an AoS `record` per node.
///
/// Nodes are in **tree order** (preorder): node `i`'s descendants are exactly
/// `[i + 1, subtree_end[i])`.
///
/// This is a hybrid on purpose.  Bulk passes only ever ask "is this an
/// element?" (candidate scans, `:empty`, projection filters), so `kind` gets its
/// own 1-byte column and those passes stream 1 B/node instead of a full record.
/// Everything else stays in one AoS record, because selector matching and text
/// walking read several fields for the same node and benefit from a single
/// cache line (a 10-column SoA made those passes slower; see the node-layout
/// section in `BENCHLOG.md`).
///
/// `kind` and `raw` share one byte per node: the two low bits are the
/// [`NodeKind`] discriminant and [`RAW_TEXT_BIT`] marks rawtext/RCDATA text.
#[derive(Clone, Debug, Default)]
pub struct NodeTape {
    /// `NodeKind` discriminant plus [`RAW_TEXT_BIT`].  This is the scan column.
    pub kind: Vec<u8>,
    /// Every other field, one record per node.
    pub record: Vec<NodeRecord>,
}

/// Per-node fields other than [`NodeTape::kind`].
///
/// Kept `pub(crate)`: callers go through the [`Node`] view.
#[derive(Clone, Debug)]
pub struct NodeRecord {
    parent: NodeId,
    first_child: NodeId,
    next_sibling: NodeId,
    prev_element_sibling: NodeId,
    subtree_end: NodeId,
    depth: u16,
    span: Range<u32>,
    tag_token: u32,
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

/// Namespace of an element: HTML (the default).
pub const NS_HTML: u8 = 0;
/// Namespace of an element: SVG.
pub const NS_SVG: u8 = 1;
/// Namespace of an element: MathML.
pub const NS_MATH: u8 = 2;

/// Internal annotation on a MathML text integration point (`mi`, `mo`, `mn`,
/// `ms`, `mtext`): the element's namespace is [`NS_MATH`], but its children
/// belong to HTML.  [`Node::namespace`] reports it as MathML.
pub const NS_MATH_TEXT_POINT: u8 = 3;

/// Internal annotation: an SVG element whose children are HTML
/// (`foreignObject`, `desc`, `title`).  Reported as [`NS_SVG`] by
/// [`Node::namespace`].
pub const NS_SVG_HTML_POINT: u8 = 4;

/// Namespaces live in bits 3-5 of the kind byte, so they cost no extra memory
/// and stay in the column that bulk scans already stream.
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

/// Everything needed to append one node.
///
/// This is the builder-facing record; it lives on the builder stack only and is
/// immediately stored, so it never costs resident memory.
pub(crate) struct NodeRecordInit {
    pub kind: NodeKind,
    pub parent: NodeId,
    pub first_child: NodeId,
    pub next_sibling: NodeId,
    pub prev_element_sibling: NodeId,
    pub subtree_end: NodeId,
    pub depth: u16,
    pub span: Range<u32>,
    pub tag_token: u32,
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
    ///
    /// Both vectors are reserved but **not** initialized, so a parse only pays
    /// for the nodes it actually writes.
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            kind: Vec::with_capacity(capacity),
            record: Vec::with_capacity(capacity),
        }
    }

    /// Number of nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.record.len()
    }

    /// Iterates nodes in preorder.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = Node<'_>> + ExactSizeIterator + '_ {
        (0..self.len() as NodeId).map(move |id| Node { tape: self, id })
    }

    /// Appends one node.
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

/// A borrowed view of one node.
///
/// Every accessor is a single column load, so a pass that reads only one field
/// only touches that column.  The view itself is two words and is meant to be
/// created on demand (`doc.node(id)`), not stored.
#[derive(Clone, Copy, Debug)]
pub struct Node<'a> {
    tape: &'a NodeTape,
    id: NodeId,
}

impl Node<'_> {
    /// Broad node category.
    #[must_use]
    #[inline]
    pub fn kind(&self) -> NodeKind {
        NodeKind::from_bits(self.tape.kind[self.tape.idx(self.id)])
    }

    /// Whether this is an element node.
    #[must_use]
    #[inline]
    pub fn is_element(&self) -> bool {
        self.tape.kind[self.tape.idx(self.id)] & KIND_MASK == NodeKind::Element.bits()
    }

    /// Whether this is a text node.
    #[must_use]
    #[inline]
    pub fn is_text(&self) -> bool {
        self.tape.kind[self.tape.idx(self.id)] & KIND_MASK == NodeKind::Text.bits()
    }

    /// Element namespace ([`NS_HTML`], [`NS_SVG`], [`NS_MATH`]).
    #[must_use]
    #[inline]
    pub fn namespace(&self) -> u8 {
        match self.tape.namespace_of(self.id) {
            NS_MATH_TEXT_POINT => NS_MATH,
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

    /// Parent id, or [`crate::NO_NODE`] for the root.
    #[must_use]
    #[inline]
    pub fn parent(&self) -> NodeId {
        self.tape.record[self.tape.idx(self.id)].parent
    }

    /// First child id, or [`crate::NO_NODE`].
    #[must_use]
    #[inline]
    pub fn first_child(&self) -> NodeId {
        self.tape.record[self.tape.idx(self.id)].first_child
    }

    /// Next sibling id, or [`crate::NO_NODE`].
    #[must_use]
    #[inline]
    pub fn next_sibling(&self) -> NodeId {
        self.tape.record[self.tape.idx(self.id)].next_sibling
    }

    /// Previous **element** sibling id, or [`crate::NO_NODE`].
    #[must_use]
    #[inline]
    pub fn prev_element_sibling(&self) -> NodeId {
        self.tape.record[self.tape.idx(self.id)].prev_element_sibling
    }

    /// One past the last node in this subtree.
    #[must_use]
    #[inline]
    pub fn subtree_end(&self) -> NodeId {
        self.tape.record[self.tape.idx(self.id)].subtree_end
    }

    /// Depth from the root (root = 0).
    #[must_use]
    #[inline]
    pub fn depth(&self) -> u16 {
        self.tape.record[self.tape.idx(self.id)].depth
    }

    /// Source span: the start tag for elements, the text run for text.
    #[must_use]
    #[inline]
    pub fn span(&self) -> Range<u32> {
        self.tape.record[self.tape.idx(self.id)].span.clone()
    }

    /// Tag token index for start tags; [`crate::NO_TAG`] otherwise.
    #[must_use]
    #[inline]
    pub fn tag_token(&self) -> u32 {
        self.tape.record[self.tape.idx(self.id)].tag_token
    }

    /// One past the last input byte of the node's outer HTML.
    #[must_use]
    #[inline]
    pub fn outer_end(&self) -> u32 {
        self.tape.record[self.tape.idx(self.id)].outer_end
    }
}

/// Broad node category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum NodeKind {
    /// The document root.  It has no source span of its own.
    Document = 0,
    /// An element, including void and rawtext elements.
    Element = 1,
    /// A text run.
    Text = 2,
    /// A comment, processing instruction or bogus comment.
    ///
    /// Doctypes share this kind and are marked with [`DOCTYPE_BIT`], because the
    /// kind column only has two bits.
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

/// Warning category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WarningKind {
    /// An end tag had no matching open element.
    UnexpectedEndTag,
}

/// A non-fatal parse warning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Warning {
    /// What happened.
    pub kind: WarningKind,
    /// Where it happened.
    pub span: Range<u32>,
}

impl Document<'_> {
    /// Returns a borrowed view of one node.
    ///
    /// The view is two words (`&NodeTape` + `NodeId`); each accessor reads a single
    /// column, so callers that only need one field never pull the whole record.
    #[must_use]
    #[inline]
    pub fn node(&self, id: NodeId) -> Node<'_> {
        Node {
            tape: &self.node_tape,
            id,
        }
    }

    /// Returns a tag name by tag-token index.
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

    /// Element name with HTML's ASCII lowercasing applied.
    ///
    /// The tape keeps the author's spelling (`<DIV>` stays `DIV`), but the HTML
    /// tree model lowercases element names at tokenization, so this is what a
    /// spec-shaped tree contains.  Borrows when the name already is lowercase.
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

    /// Attribute name as the tree model stores it.
    ///
    /// HTML lowercases attribute names at tokenization, and foreign elements
    /// adjust a fixed list to camelCase (`viewbox` -> `viewBox`).  The tape
    /// keeps the author's spelling in the source bytes, so both steps happen
    /// here: an override index when the specification adjusts the name, an
    /// ASCII lowercase pass otherwise.  Borrows when nothing changes.
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

    /// Returns a text node's decoded character data.
    ///
    /// Raw text element content is returned verbatim.
    #[must_use]
    pub fn text(&self, id: NodeId) -> Cow<'_, str> {
        let node = self.node(id);
        debug_assert!(node.is_text() || node.is_comment());
        let span = node.span();
        let raw = std::str::from_utf8(&self.input[span.start as usize..span.end as usize])
            .expect("Document input is valid UTF-8");
        if node.is_comment() {
            // The extractor slices the markers off; what remains still goes
            // through the input preprocessing the standard applies to the whole
            // stream (newline normalisation, NUL replacement).
            return normalize_newlines(replace_nulls(Cow::Borrowed(raw)));
        }
        // Preprocessing happens on the input stream, i.e. *before* character
        // references are decoded: `&#x0D;` yields a real carriage return, while
        // a literal CR byte in the source has already become a LF.
        let raw = normalize_newlines(Cow::Borrowed(raw));
        let decoded = if node.is_raw_text() {
            raw
        } else {
            Cow::Owned(decode_character_references(&raw, false).into_owned())
        };
        if decoded.contains('\0') {
            // U+0000 becomes U+FFFD everywhere except in the text of an
            // integration point's *own* children.  The standard says the
            // opposite (foreign content replaces, `in body` ignores) but the
            // expected trees want a replacement in raw text, in plaintext and in
            // foreign elements, and a drop only right under
            // `foreignObject`/`desc`/`title`/`mi`/`mo`/`mn`/`ms`/`mtext`/
            // `annotation-xml` (`<svg><foreignObject>a\0b` -> `"ab"`).
            //
            // Measured twice: the first reading was a bare "foreign elements
            // drop NULs", which lost 6 cases once the frameset rule stopped
            // depending on it; with the integration-point test added the same
            // change gains 9.
            let parent = self.node(self.node(id).parent());
            let parent_ns = parent.namespace();
            // An integration point routes characters to the HTML rules, so its
            // own text drops NULs; other foreign elements replace them.
            let at_point = match parent.tag_token() {
                crate::NO_TAG => false,
                tag => {
                    let name = self.tag_name_lower(tag).to_ascii_lowercase();
                    let svg_point = matches!(name.as_ref(), "foreignobject" | "desc" | "title");
                    let math_text = matches!(name.as_ref(), "mi" | "mo" | "mn" | "ms" | "mtext");
                    // `annotation-xml` is an integration point only with an
                    // HTML `encoding`, so its own text still replaces NULs
                    // otherwise (`<math><annotation-xml>\0x` -> `"\u{fffd}x"`).
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
            // Raw text (script, plaintext, ...) is the tokenizer's own state,
            // where NUL becomes U+FFFD; `in body` *ignores* it, and so does an
            // integration point (its characters follow the HTML rules).  A CDATA
            // section under an integration point is raw text but still drops.
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
    use crate::NO_NODE;

    #[test]
    fn node_predicates() {
        let mut nodes = NodeTape::with_capacity(1);
        nodes.push(NodeRecordInit {
            kind: NodeKind::Document,
            doctype: false,
            normalize_newlines: false,
            ns: NS_HTML,
            parent: NO_NODE,
            first_child: NO_NODE,
            next_sibling: NO_NODE,
            prev_element_sibling: NO_NODE,
            subtree_end: NO_NODE,
            depth: 0,
            span: 0..0,
            tag_token: crate::NO_TAG,
            outer_end: 10,
            raw_text: false,
        });
        let root = Node {
            tape: &nodes,
            id: 0,
        };
        assert!(!root.is_element());
        assert!(!root.is_text());
        assert_eq!(root.subtree_end(), NO_NODE);
        assert_eq!(root.outer_end(), 10);
    }

    #[test]
    fn node_store_is_41_bytes_per_node() {
        // kind column + one AoS record.  A pure `Vec<Node>` (kind/raw inline)
        // was 40 B/node.
        assert_eq!(std::mem::size_of::<NodeRecord>(), 40);
        let nodes = NodeTape::with_capacity(4);
        assert_eq!(nodes.len(), 0);
    }
}
