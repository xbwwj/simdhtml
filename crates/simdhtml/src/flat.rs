//! Flattened, read-only document representation.
//!
//! Nodes are stored in preorder.  Node `i`'s descendants are exactly
//! `[i + 1, subtree_end[i])`, which is what makes the set-at-a-time selector
//! possible.  There is no pointer tree and no per-node allocation for
//! children: only a few `Vec`s of plain integers.

use std::borrow::Cow;
use std::ops::Range;

use crate::entities::decode_html_entities;
use crate::{NO_NODE, NodeId};

/// 扁平化的只读文档。
///
/// The document borrows the original UTF-8 input for its whole lifetime.
/// Parsing is therefore zero-copy: spans index into `input`.
#[derive(Clone, Debug)]
pub struct FlatDoc<'a> {
    pub input: &'a [u8],

    // PERF: too many Vec allocs here, bad allocation and RSS remained by glibc
    // should consider capacity prediction and arena.

    // TODO: node should also be SoA.
    /// All nodes in **preorder**.
    pub nodes: Vec<Node>,

    /// Parsed tags.
    pub tags: Vec<TagToken>,

    /// Tag name span start per tag token (SoA for query masks).
    pub tag_name_start: Vec<u32>,
    pub tag_name_len: Vec<u16>,

    // FIXME: 目前本质是虚假节点放在最后，这不好，应该整个 SIMD 临时多 bitmap，连同 HTML 修复一起
    // 直接一次 prefix sum 合并到里面同时去修复 nodeid, 保持严格 preorder 关系。
    // 合成名称放在这吗？intern 其实也合理，或者看下 html 规范，修复的虚假节点类型，是否可以直接
    // static dispatch?
    /// Number of real tag tokens; later tag indices are synthetic.
    pub base_tag_count: u32,
    /// Synthetic tag names, indexed by `tag - base_tag_count`.
    pub synthetic_names: Vec<&'static str>,

    // Attribute SoA
    pub attr_tag: Vec<u32>,
    pub attr_key_start: Vec<u32>,
    pub attr_key_len: Vec<u16>,
    pub attr_value_start: Vec<u32>,
    pub attr_value_len: Vec<u32>,
    pub attr_quote: Vec<Quote>,

    /// Root node (always 0).
    pub root: NodeId,

    /// Document diagnostics.
    pub flags: DocFlags,

    /// 可恢复错误警报
    pub warnings: Vec<Warning>,
}

/// One node in the preorder array.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Node {
    pub kind: NodeKind,

    // PERF: it seems that we do not need parent since we already adopt a preorder nodes array and
    // use ltr search rather than rtl backtrace. Selector should at build time knows that they
    // need to keep parent id to backtrace rather than being a global memory burden.
    // NOTE: 对于 root, parent 或许应该是自己（不动点），通过别的来区分？
    /// 指向 parent id, 或者 NO_NODE
    pub parent: NodeId,

    // PERF: first_child and has_child can be inferred from subtree_end.
    // TODO: NO_NODE can be Option<NonZero<NodeId>>, this avoid a potential subtract instruction before
    // cmp. since we no longer need parent here. NodeId(0) can be reused for None.
    /// 第一个子元素, or NO_NODE
    pub first_child: NodeId,

    /// 下一 sibling, or NO_NODE
    pub next_sibling: NodeId,

    /// 前一元素 sibling, or NO_NODE
    pub prev_element_sibling: NodeId,

    /// 子树 last node + 1
    pub subtree_end: NodeId,

    // TODO: think do we really need depth?
    /// 从 root 到这的深度 (root = 0)
    pub depth: u16,

    /// 元素 start-tag 范围，root 是 0..0
    pub span: Range<u32>,

    /// Tag token index for start tags; `NO_TAG` otherwise.
    pub tag_token: u32,

    /// outer html 的 node_id + 1
    pub outer_end: u32,

    /// 文字是否来自 rawtext/RCDATA
    pub raw: bool,
}

/// Broad node category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeKind {
    /// The document root.  It has no source span of its own.
    Root,
    /// An element, including void and rawtext elements.
    Element,
    /// A text run.
    Text,
}

/// Tag token kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TagKind {
    /// `<name ...>`
    Start,
    /// `</name>`
    End,
}

/// A parsed tag.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TagToken {
    /// `<` through `>` inclusive (`end` is one past `>`).
    pub span: Range<u32>,

    /// Start or end.
    pub kind: TagKind,

    /// Attribute index range owned by this tag.
    pub attrs: Range<u32>,

    /// Element node for a start tag; `NO_NODE` for an end tag.
    pub node: NodeId,

    /// Whether the source had a self-closing slash.
    pub self_closing: bool,
}

/// Attribute value quoting style.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Quote {
    /// `"value"`
    Double,
    /// `'value'`
    Single,
    /// `value`
    Unquoted,
    /// Attribute with no `=`.
    Boolean,
}

/// One attribute cut from a raw start-tag span.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttrRef {
    /// Raw attribute-name span.
    pub name: Range<u32>,
    /// Raw value span, or `None` for a boolean attribute.
    pub value: Option<Range<u32>>,
    /// Quoting style.
    pub quote: Quote,
}

impl AttrRef {
    /// Whether this is a boolean attribute (no `=` in the source).
    #[must_use]
    pub const fn is_boolean(&self) -> bool {
        matches!(self.quote, Quote::Boolean)
    }
}

/// Structural attribute iterator over a raw start-tag span.
///
/// This is the structural cut: it finds names and values from whitespace,
/// `=`, quotes, `/` and `>` without decoding, lowercasing or interning.
/// Duplicate attributes are yielded in source order; HTML's "first wins" rule
/// is applied by lookups such as [`FlatDoc::find_attr`].
#[derive(Clone, Debug)]
pub struct AttrIter<'a> {
    input: &'a [u8],
    pos: usize,
    end: usize,
}

impl<'a> AttrIter<'a> {
    fn new(input: &'a [u8], span: &Range<u32>) -> Self {
        let start = span.start as usize;
        let end = (span.end as usize).min(input.len());
        let mut pos = start;
        if input.get(pos) == Some(&b'<') {
            pos += 1;
        }
        if input.get(pos) == Some(&b'/') {
            // End tags carry no attributes.
            return Self {
                input,
                pos: end,
                end,
            };
        }
        while pos < end && !is_whitespace(input[pos]) && !matches!(input[pos], b'/' | b'>') {
            pos += 1;
        }
        Self { input, pos, end }
    }
}

impl Iterator for AttrIter<'_> {
    type Item = AttrRef;

    fn next(&mut self) -> Option<Self::Item> {
        let input = self.input;
        let end = self.end;
        while self.pos < end && is_whitespace(input[self.pos]) {
            self.pos += 1;
        }
        if self.pos >= end || matches!(input[self.pos], b'>' | b'/') {
            return None;
        }

        let name_start = self.pos;
        while self.pos < end
            && !is_whitespace(input[self.pos])
            && !matches!(input[self.pos], b'=' | b'/' | b'>')
        {
            self.pos += 1;
        }
        let name_end = self.pos;
        if name_start == name_end {
            self.pos += 1;
            return self.next();
        }

        while self.pos < end && is_whitespace(input[self.pos]) {
            self.pos += 1;
        }

        let (value, quote) = if self.pos < end && input[self.pos] == b'=' {
            self.pos += 1;
            while self.pos < end && is_whitespace(input[self.pos]) {
                self.pos += 1;
            }
            if self.pos < end && matches!(input[self.pos], b'"' | b'\'') {
                let q = input[self.pos];
                self.pos += 1;
                let value_start = self.pos;
                while self.pos < end && input[self.pos] != q {
                    self.pos += 1;
                }
                let value_end = self.pos;
                if self.pos < end {
                    self.pos += 1;
                }
                (
                    Some((value_start as u32)..(value_end as u32)),
                    if q == b'"' {
                        Quote::Double
                    } else {
                        Quote::Single
                    },
                )
            } else {
                let value_start = self.pos;
                while self.pos < end && !is_whitespace(input[self.pos]) && input[self.pos] != b'>' {
                    self.pos += 1;
                }
                (
                    Some((value_start as u32)..(self.pos as u32)),
                    Quote::Unquoted,
                )
            }
        } else {
            (None, Quote::Boolean)
        };

        Some(AttrRef {
            name: (name_start as u32)..(name_end as u32),
            value,
            quote,
        })
    }
}

fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r')
}

bitflags::bitflags! {
    /// Document-level diagnostics.
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
    pub struct DocFlags: u32 {
        /// An end tag did not match any open element, or an element was
        /// implicitly closed by the normalizer.
        const MISNESTED = 1 << 0;

        /// Stage-1 reported unbalanced tag starts/ends.
        const UNBALANCED_TAG = 1 << 1;

        /// The open-element stack exceeded the configured limit.
        const DEPTH_LIMIT = 1 << 2;

        /// The document uses a construct outside the fast path.
        const NEEDS_FALLBACK = 1 << 3;

        /// A synthetic end tag was emitted for a void element.
        const SYNTHETIC_END = 1 << 4;
    }
}

/// Warning category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WarningKind {
    /// An end tag had no matching open element.
    UnmatchedEndTag,
    /// An open element was closed implicitly by the normalizer.
    ImpliedEndTag,
    /// A duplicate attribute was ignored.
    DuplicateAttribute,
    /// An attribute or tag name was empty/malformed.
    MalformedTag,
}

/// A non-fatal parse warning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Warning {
    /// What happened.
    pub kind: WarningKind,
    /// Where it happened.
    pub span: Range<u32>,
}

impl FlatDoc<'_> {
    /// Returns a node.
    // PERF: Node 绝对应该 SoA, 而不是为了这里做成 AoS.
    #[must_use]
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    /// Returns a tag name by tag-token index.
    #[must_use]
    pub fn tag_name(&self, tag: u32) -> &str {
        if tag >= self.base_tag_count {
            return self.synthetic_names[(tag - self.base_tag_count) as usize];
        }
        let start = self.tag_name_start[tag as usize] as usize;
        let len = self.tag_name_len[tag as usize] as usize;
        std::str::from_utf8(&self.input[start..start + len]).expect("FlatDoc input is valid UTF-8")
    }

    /// Returns an element's tag name, or `None` for non-elements.
    #[must_use]
    pub fn node_tag_name(&self, id: NodeId) -> Option<&str> {
        let node = self.node(id);
        node.is_element().then(|| self.tag_name(node.tag_token))
    }

    /// Iterates the structural attributes of an element without materializing
    /// an attribute table.
    #[must_use]
    pub fn attr_iter(&self, id: NodeId) -> AttrIter<'_> {
        AttrIter::new(self.input, &self.node(id).span)
    }

    /// Finds the first attribute with `name` (HTML names are case-insensitive).
    #[must_use]
    pub fn find_attr(&self, id: NodeId, name: &str) -> Option<AttrRef> {
        self.attr_iter(id)
            .find(|attr| self.attr_name_is(attr, name))
    }

    /// Returns whether an attribute's raw name matches `name`.
    #[must_use]
    pub fn attr_name_is(&self, attr: &AttrRef, name: &str) -> bool {
        let raw = self.raw_attr_name(attr);
        raw.eq_ignore_ascii_case(name.as_bytes())
    }

    /// Returns the attribute range owned by a tag token.
    ///
    /// Duplicates are kept in source order; this is a structural range.  Use
    /// [`Self::find_attr_index`] or [`Self::find_attr`] for HTML's first-wins
    /// lookup.
    #[must_use]
    pub fn tag_attrs(&self, tag: u32) -> Range<u32> {
        self.tags[tag as usize].attrs.clone()
    }

    /// Returns an attribute key's raw bytes.
    #[must_use]
    pub fn attr_key(&self, attr: u32) -> &[u8] {
        let a = attr as usize;
        &self.input[self.attr_key_start[a] as usize
            ..self.attr_key_start[a] as usize + self.attr_key_len[a] as usize]
    }

    /// Returns an attribute value's raw bytes.
    #[must_use]
    pub fn attr_raw_value(&self, attr: u32) -> &[u8] {
        let a = attr as usize;
        &self.input[self.attr_value_start[a] as usize
            ..self.attr_value_start[a] as usize + self.attr_value_len[a] as usize]
    }

    /// Returns an attribute value as decoded text.
    #[must_use]
    pub fn attr_decoded_value(&self, attr: u32) -> Cow<'_, str> {
        let raw = self.attr_raw_value(attr);
        let text = std::str::from_utf8(raw).expect("FlatDoc input is valid UTF-8");
        decode_html_entities(text, true)
    }

    /// Finds the first attribute index by key name.
    ///
    /// HTML keeps the first duplicate attribute, so the scan stops at the first
    /// case-insensitive key match.
    #[must_use]
    pub fn find_attr_index(&self, node: NodeId, name: &str) -> Option<u32> {
        let tag_token = self.node(node).tag_token;
        if tag_token == crate::NO_TAG {
            return None;
        }
        self.tag_attrs(tag_token)
            .find(|&attr| self.attr_key(attr).eq_ignore_ascii_case(name.as_bytes()))
    }

    /// Returns the raw attribute-name bytes.
    #[must_use]
    pub fn raw_attr_name(&self, attr: &AttrRef) -> &[u8] {
        self.raw_attr_name_span(&attr.name)
    }

    /// Returns raw attribute-name bytes for a name span.
    #[must_use]
    pub fn raw_attr_name_span(&self, span: &Range<u32>) -> &[u8] {
        &self.input[span.start as usize..span.end as usize]
    }

    /// Returns the raw attribute-value bytes (empty for boolean attributes).
    #[must_use]
    pub fn raw_attr_value(&self, attr: &AttrRef) -> &[u8] {
        match attr.value {
            Some(ref span) => &self.input[span.start as usize..span.end as usize],
            None => &[],
        }
    }

    /// Decodes an attribute value span.
    ///
    /// # Panics
    ///
    /// Panics if the document input is not valid UTF-8.  The parser only
    /// accepts UTF-8, so this is an invariant.
    #[must_use]
    pub fn attr_value_span(&self, span: &Range<u32>) -> Cow<'_, str> {
        let raw = &self.input[span.start as usize..span.end as usize];
        let text = std::str::from_utf8(raw).expect("FlatDoc input is valid UTF-8");
        decode_html_entities(text, true)
    }

    /// Decodes an attribute value.
    #[must_use]
    pub fn attr_value(&self, attr: &AttrRef) -> Cow<'_, str> {
        match attr.value {
            Some(ref span) => self.attr_value_span(span),
            None => Cow::Borrowed(""),
        }
    }

    /// Returns a text node's decoded text.
    ///
    /// Rawtext nodes are returned verbatim.
    #[must_use]
    pub fn text_of(&self, id: NodeId) -> Cow<'_, str> {
        let node = self.node(id);
        debug_assert!(node.is_text());
        let raw =
            std::str::from_utf8(&self.input[node.span.start as usize..node.span.end as usize])
                .expect("FlatDoc input is valid UTF-8");
        if node.raw {
            Cow::Borrowed(raw)
        } else {
            decode_html_entities(raw, false)
        }
    }

    /// Concatenates all descendant text of `id` (including `id` if it is a
    /// text node), matching `lxml`'s `text_content()` for the supported subset.
    #[must_use]
    /// Concatenates all descendant text, matching `lxml.text_content()`.
    pub fn text_content(&self, id: NodeId) -> String {
        let node = self.node(id);
        if node.is_text() {
            return self.text_of(id).into_owned();
        }
        let mut out = String::new();
        for child_id in id..node.subtree_end {
            if self.nodes[child_id as usize].is_text() {
                out.push_str(&self.text_of(child_id));
            }
        }
        out
    }

    /// Returns the original bytes for `get()`.
    ///
    /// Elements return their outer HTML, text nodes their text, and the root
    /// the whole input.
    #[must_use]
    pub fn outer_html(&self, id: NodeId) -> &[u8] {
        let node = self.node(id);
        match node.kind {
            NodeKind::Root => self.input,
            NodeKind::Element | NodeKind::Text => {
                &self.input[node.span.start as usize..node.outer_end as usize]
            }
        }
    }

    /// Returns the raw inner text span of a text node.
    #[must_use]
    pub fn text_span(&self, id: NodeId) -> Range<u32> {
        self.node(id).span.clone()
    }
}

impl Node {
    /// Creates the root node.
    #[must_use]
    pub fn root(input_len: u32) -> Self {
        Self {
            kind: NodeKind::Root,
            parent: NO_NODE,
            first_child: NO_NODE,
            next_sibling: NO_NODE,
            prev_element_sibling: NO_NODE,
            subtree_end: NO_NODE,
            depth: 0,
            span: 0..0,
            tag_token: crate::NO_TAG,
            outer_end: input_len,
            raw: false,
        }
    }

    /// Whether this is an element node.
    #[must_use]
    pub const fn is_element(&self) -> bool {
        matches!(self.kind, NodeKind::Element)
    }

    /// Whether this is a text node.
    #[must_use]
    pub const fn is_text(&self) -> bool {
        matches!(self.kind, NodeKind::Text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_predicates() {
        let root = Node::root(10);
        assert!(!root.is_element());
        assert!(!root.is_text());
        assert_eq!(root.subtree_end, NO_NODE);
        assert_eq!(root.outer_end, 10);
    }
}
