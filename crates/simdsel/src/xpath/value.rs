use std::borrow::Cow;

use simdhtml::{Document, NO_TAG, NodeId};

use crate::raw::{attr_name, attr_value};

/// What a [`NodeRef`] points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    /// The document node (the parent of the root element).
    Document,
    /// An element.
    Element,
    /// A text node.
    Text,
    /// An attribute node; the [`NodeRef`]'s node id is the owning element.
    Attribute,
}

/// A node in an XPath node-set: elements and text by preorder [`NodeId`],
/// attributes by owning element plus flat attribute index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeRef {
    kind: RefKind,
    id: NodeId,
    attr: u32,
}

impl NodeRef {
    pub(crate) fn new(kind: RefKind, id: NodeId, attr: u32) -> Self {
        Self { kind, id, attr }
    }

    /// The document node (for callers that need a context node).
    pub fn document() -> Self {
        Self::new(RefKind::Document, 0, 0)
    }

    /// An element reference.
    pub fn element(id: NodeId) -> Self {
        Self::new(RefKind::Element, id, 0)
    }

    /// A text-node reference.
    pub fn text(id: NodeId) -> Self {
        Self::new(RefKind::Text, id, 0)
    }

    /// An attribute-node reference (owned by element `id`, flat index `attr`).
    pub fn attribute(id: NodeId, attr: u32) -> Self {
        Self::new(RefKind::Attribute, id, attr)
    }

    /// What this reference points at.
    pub fn kind(self) -> RefKind {
        self.kind
    }

    /// The preorder id of the node, or of the owning element for attributes.
    pub fn node_id(self) -> NodeId {
        self.id
    }

    /// The flat attribute index when this is an attribute node.
    pub fn attr_index(self) -> Option<u32> {
        (self.kind == RefKind::Attribute).then_some(self.attr)
    }

    /// Document-order key: preorder id, with attributes sorting right after
    /// their element.
    pub(crate) fn order_key(self) -> (NodeId, u32) {
        match self.kind {
            RefKind::Attribute => (self.id, self.attr + 1),
            _ => (self.id, 0),
        }
    }

    /// XPath `string()` of the node.  Adjacent text siblings are merged: the
    /// XPath data model has
    /// one text node per maximal run, while the parser's tape may split a run
    /// wherever the markup scan ended one (a comment between two runs is not
    /// adjacent, so this only merges true siblings).
    pub fn string_value<'a>(self, doc: &'a Document<'_>) -> Cow<'a, str> {
        match self.kind {
            RefKind::Attribute => attr_value(doc, self.attr),
            RefKind::Text => match crate::xpath::eval::text_run(doc, self.id) {
                None => doc.text(self.id),
                Some(last) => {
                    let mut out = String::new();
                    for id in self.id..=last {
                        out.push_str(&doc.text(id));
                    }
                    Cow::Owned(out)
                }
            },
            RefKind::Element => Cow::Owned(crate::raw::text_content(doc, self.id)),
            // The root node's string-value is that of its element child: text
            // before/after the root element is not in the XPath data model.
            RefKind::Document => {
                let root = doc.node(self.id).first_child();
                if root == simdhtml::NO_NODE {
                    Cow::Borrowed("")
                } else {
                    Cow::Owned(crate::raw::text_content(doc, root))
                }
            }
        }
    }

    /// XPath ``name()``/``local-name()`` of the node ("" for text and document
    /// nodes).
    pub fn name<'a>(self, doc: &'a Document<'_>) -> &'a str {
        match self.kind {
            RefKind::Element => {
                let tag = doc.node(self.id).tag_token();
                if tag == NO_TAG { "" } else { doc.tag_name(tag) }
            }
            RefKind::Attribute => std::str::from_utf8(attr_name(doc, self.attr))
                .expect("Document input is valid UTF-8"),
            _ => "",
        }
    }
}

/// The result of evaluating an XPath expression.
#[derive(Clone, Debug)]
pub enum Value {
    /// A node-set, in document order and without duplicates.
    Nodes(Vec<NodeRef>),
    /// A string.
    Str(String),
    /// A number (IEEE-754 double, as XPath 1.0 requires).
    Number(f64),
    /// A boolean.
    Bool(bool),
}

impl Value {
    /// XPath ``boolean()``.
    pub(crate) fn to_boolean(&self) -> bool {
        match self {
            Self::Nodes(nodes) => !nodes.is_empty(),
            Self::Str(s) => !s.is_empty(),
            Self::Number(n) => *n != 0.0 && !n.is_nan(),
            Self::Bool(b) => *b,
        }
    }

    /// XPath ``number()``.
    pub(crate) fn to_number(&self, doc: &Document<'_>) -> f64 {
        match self {
            Self::Number(n) => *n,
            Self::Bool(b) => f64::from(u8::from(*b)),
            Self::Str(s) => parse_number(s),
            Self::Nodes(nodes) => match nodes.first() {
                Some(node) => parse_number(&node.string_value(doc)),
                None => f64::NAN,
            },
        }
    }

    /// XPath ``string()``.
    pub(crate) fn to_string(&self, doc: &Document<'_>) -> String {
        match self {
            Self::Str(s) => s.clone(),
            Self::Bool(true) => "true".to_owned(),
            Self::Bool(false) => "false".to_owned(),
            Self::Number(n) => format_number(*n),
            Self::Nodes(nodes) => match nodes.first() {
                Some(node) => node.string_value(doc).into_owned(),
                None => String::new(),
            },
        }
    }
}

/// XPath number parsing: optional surrounding whitespace, optional sign,
/// digits with at most one dot.  Anything else is NaN.
pub(crate) fn parse_number(text: &str) -> f64 {
    let trimmed = text.trim_matches(|c: char| c == ' ' || c == '\t' || c == '\n' || c == '\r');
    if trimmed.is_empty() {
        return f64::NAN;
    }
    let mut seen_dot = false;
    let mut seen_digit = false;
    for (i, byte) in trimmed.bytes().enumerate() {
        match byte {
            b'0'..=b'9' => seen_digit = true,
            b'.' if !seen_dot => seen_dot = true,
            b'-' | b'+' if i == 0 => {}
            _ => return f64::NAN,
        }
    }
    if seen_digit {
        trimmed.parse().unwrap_or(f64::NAN)
    } else {
        f64::NAN
    }
}

/// XPath number to string: integers without a decimal point, others via the
/// shortest round-tripping form.
fn format_number(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_owned()
    } else if value.is_infinite() {
        if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
    } else if value == value.trunc() && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nodes(nodes) => write!(f, "<{} nodes>", nodes.len()),
            Self::Str(s) => write!(f, "{s}"),
            Self::Number(n) => write!(f, "{}", format_number(*n)),
            Self::Bool(b) => write!(f, "{b}"),
        }
    }
}
