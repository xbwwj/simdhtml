use std::borrow::Cow;
use std::ops::Range;

use simdhtml::character_reference::decode_character_references;
use simdhtml::document::Document;
use simdhtml::document::id::{NO_TAG, NodeId};
use simdhtml::document::text::replace_nulls;

/// Raw attribute-name bytes (not lowercased, not interned).
pub(crate) fn attr_name<'a>(doc: &'a Document<'_>, attr: u32) -> &'a [u8] {
    let start = doc.attrs.name_start[attr as usize] as usize;
    let end = start + doc.attrs.name_len[attr as usize] as usize;
    &doc.input[start..end]
}

/// Raw attribute value bytes (references not decoded): predicates that only
/// compare or split on whitespace need no decoding.  Callers comparing against
/// decoded text check for `&` first and fall back to [`attr_value`].
#[inline]
pub(crate) fn attr_value_raw<'a>(doc: &'a Document<'_>, attr: u32) -> &'a [u8] {
    let start = doc.attrs.value_start[attr as usize] as usize;
    let end = start + doc.attrs.value_len[attr as usize] as usize;
    &doc.input[start..end]
}

/// Attribute value bytes with character references decoded.
pub(crate) fn attr_value<'a>(doc: &'a Document<'_>, attr: u32) -> Cow<'a, str> {
    let start = doc.attrs.value_start[attr as usize] as usize;
    let end = start + doc.attrs.value_len[attr as usize] as usize;
    decode_span(doc, start..end)
}

/// Decodes a raw attribute value span.
pub(crate) fn attr_value_span<'a>(doc: &'a Document<'_>, span: &Range<u32>) -> Cow<'a, str> {
    decode_span(doc, span.start as usize..span.end as usize)
}

fn decode_span<'a>(doc: &'a Document<'_>, span: Range<usize>) -> Cow<'a, str> {
    let raw = std::str::from_utf8(&doc.input[span]).expect("Document input is valid UTF-8");
    replace_nulls(decode_character_references(raw, true))
}

/// Attribute index range owned by an element; empty for non-elements.
pub(crate) fn attr_range(doc: &Document<'_>, node: NodeId) -> Range<u32> {
    let tag = doc.node(node).tag_token();
    if tag == NO_TAG {
        return 0..0;
    }
    doc.tags.token(tag).attr_range.clone()
}

/// ASCII-case-insensitive equality against a known lowercase identifier, one
/// 8-byte word at a time (the generic slice version is a byte loop and dominates
/// `find_attr`).  Folding with `0x20` also collapses `[@[\]^_`-`{`, which a
/// selector name never contains.
#[inline]
fn name_eq(candidate: &[u8], name: &[u8]) -> bool {
    if candidate.len() != name.len() {
        return false;
    }
    if candidate.len() <= 8 {
        let mut a = [0u8; 8];
        let mut b = [0u8; 8];
        a[..candidate.len()].copy_from_slice(candidate);
        b[..name.len()].copy_from_slice(name);
        (u64::from_le_bytes(a) | 0x2020_2020_2020_2020)
            == (u64::from_le_bytes(b) | 0x2020_2020_2020_2020)
    } else {
        candidate.eq_ignore_ascii_case(name)
    }
}

/// First attribute with `name` (HTML keeps the first duplicate).
#[inline]
pub(crate) fn find_attr(doc: &Document<'_>, node: NodeId, name: &str) -> Option<u32> {
    find_attr_bytes(doc, node, name.as_bytes())
}

#[inline]
pub(crate) fn find_attr_bytes(doc: &Document<'_>, node: NodeId, name: &[u8]) -> Option<u32> {
    let range = attr_range(doc, node);
    let mut attr = range.start;
    while attr < range.end {
        if name_eq(attr_name(doc, attr), name) {
            return Some(attr);
        }
        attr += 1;
    }
    None
}

/// Concatenated character data of a subtree, in tree order.
pub(crate) fn text_content(doc: &Document<'_>, node: NodeId) -> String {
    let n = doc.node(node);
    if n.is_text() {
        return doc.text(node).into_owned();
    }
    let mut out = String::new();
    for child in node.get()..n.subtree_end().get() {
        let child = NodeId::new(child);
        if doc.node(child).is_text() {
            out.push_str(&doc.text(child));
        }
    }
    out
}

/// Original bytes of a node's outer HTML.
pub(crate) fn outer_html<'a>(doc: &'a Document<'_>, node: NodeId) -> &'a [u8] {
    let n = doc.node(node);
    let span = n.span();
    &doc.input[span.start as usize..n.outer_end() as usize]
}
