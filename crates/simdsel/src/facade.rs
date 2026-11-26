//! The parsel-shaped [`Selector`].

use std::borrow::Cow;
use std::rc::Rc;

use simdhtml::document::Document;
use simdhtml::document::id::{DOC_ROOT, NodeId};
use simdhtml::parse::{EncodingMode, ParseOptions};

use crate::css::ast::SelectorList;
use crate::css::cache::QueryCache;
use crate::css::eval::root_element;
use crate::css::parser::parse_selector;
use crate::css::select::{Count, Projection, extract, select_as_within_cache, select_within_cache};
use crate::error::Result;
use crate::record::{MatchKind, MatchRecord};
use crate::xpath::XPath;
use crate::xpath::value::{NodeRef, RefKind, Value};

/// One result of a selector chain: an element/text/attribute match or a
/// scalar XPath string.
#[derive(Clone, Debug)]
enum Item {
    Record(MatchRecord),
    Text(String),
}

/// An owned document or a result set, like parsel's `Selector`/`SelectorList`.
///
/// ```
/// use simdsel::Selector;
///
/// let sel = Selector::from_text(r#"<div class=item><b>hi</b></div>"#)?;
/// assert_eq!(sel.css(".item b::text")?.get(), Some("hi".into()));
/// assert_eq!(sel.xpath("//b/text()")?.get(), Some("hi".into()));
/// # Ok::<(), simdsel::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct Selector<'a> {
    doc: Rc<Document<'a>>,
    roots: Vec<NodeId>,
    items: Vec<Item>,
    /// Derived query data (masks, child index) shared by every query from this
    /// selector.  A function of the document, not of `roots`, so it stays valid
    /// as queries narrow; built on demand.
    cache: Rc<QueryCache>,
}

impl<'a> Selector<'a> {
    /// Wraps an already parsed document and selects its root element.
    ///
    /// ```
    /// use simdsel::{Document, Selector};
    ///
    /// let doc = Document::parse(b"<p>hi</p>")?;
    /// let sel = Selector::new(doc);
    /// assert_eq!(sel.css("p::text")?.get(), Some("hi".into()));
    /// # Ok::<(), simdsel::Error>(())
    /// ```
    pub fn new(doc: Document<'a>) -> Self {
        let root = Item::Record(document_record(&doc));
        Self {
            doc: Rc::new(doc),
            roots: vec![DOC_ROOT],
            items: vec![root],
            cache: Rc::new(QueryCache::root()),
        }
    }

    /// Parses `input` as bytes (parsel's `body=`) and selects from its root.
    /// The charset is sniffed from a BOM or `<meta charset>`.
    pub fn from_bytes(input: &'a [u8]) -> Result<Self> {
        Ok(Self::new(Document::parse(input)?))
    }

    /// [`Self::from_bytes`] with explicit [`ParseOptions`].
    pub fn from_bytes_with(input: &'a [u8], options: ParseOptions) -> Result<Self> {
        Ok(Self::new(Document::parse_with(input, options)?))
    }

    /// Parses already decoded `text` (parsel's `text=`) and selects from its
    /// root. No BOM or `<meta charset>` sniffing: a `&str` is already text.
    pub fn from_text(text: &'a str) -> Result<Self> {
        Self::from_bytes_with(
            text.as_bytes(),
            ParseOptions {
                encoding: EncodingMode::Utf8,
                ..Default::default()
            },
        )
    }

    /// Splits a result set into one single-result selector per item; the
    /// derived selectors share the document and the query cache.
    ///
    /// ```
    /// use simdsel::Selector;
    ///
    /// let sel = Selector::from_text("<b>a</b><b>b</b>")?;
    /// let items = sel.css("b")?.split();
    /// let texts = items.iter().filter_map(|b| b.text()).collect::<Vec<_>>();
    /// assert_eq!(texts, vec!["a", "b"]);
    /// # Ok::<(), simdsel::Error>(())
    /// ```
    pub fn split(&self) -> Vec<Self> {
        self.items
            .iter()
            .map(|item| Self {
                doc: Rc::clone(&self.doc),
                roots: match item {
                    Item::Record(record) => vec![record.node],
                    Item::Text(_) => Vec::new(),
                },
                items: vec![item.clone()],
                cache: Rc::clone(&self.cache),
            })
            .collect()
    }

    /// The document this selector owns.
    pub fn doc(&self) -> &Document<'a> {
        &self.doc
    }

    /// Current search roots.
    pub fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    /// Whether every result is a string: an XPath scalar or `::text`/`::attr`
    /// output rather than an element.
    pub fn is_text(&self) -> bool {
        !self.items.is_empty()
            && self.items.iter().all(|item| match item {
                Item::Text(_) => true,
                Item::Record(record) => record.kind != MatchKind::Element,
            })
    }

    /// Number of results.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the selection is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Per-thread compiled-selector intern table: `css*` takes selector text on
    /// every call, and parsing it costs ~11% of a warm query.  Bounded, so a
    /// query-generating crawler clears it instead of growing it.
    fn compiled(query: &str) -> Result<Rc<SelectorList>> {
        use std::cell::RefCell;
        use std::collections::HashMap;

        thread_local! {
            static CACHE: RefCell<HashMap<Box<str>, Rc<SelectorList>>> = RefCell::new(HashMap::new());
        }
        // A few hundred selectors bound the memory.
        const MAX_ENTRIES: usize = 256;

        CACHE.with(|cache| {
            if let Some(parsed) = cache.borrow().get(query) {
                return Ok(Rc::clone(parsed));
            }
            let parsed = Rc::new(parse_selector(query)?);
            let mut cache = cache.borrow_mut();
            if cache.len() >= MAX_ENTRIES {
                cache.clear();
            }
            cache.insert(query.into(), Rc::clone(&parsed));
            Ok(parsed)
        })
    }

    /// Compiles and evaluates a CSS selector inside the current results.
    ///
    /// The current results are not part of the search (use `:scope` to include
    /// them). `::text` and `::attr(name)` select text and attribute values.
    pub fn css(&self, query: &str) -> Result<Self> {
        let parsed = Self::compiled(query)?;
        let records = select_within_cache(&self.doc, &parsed, &self.roots, &self.cache)?;
        let roots = records.iter().map(|record| record.node).collect::<Vec<_>>();
        Ok(Self {
            doc: Rc::clone(&self.doc),
            roots,
            items: records.into_iter().map(Item::Record).collect(),
            cache: Rc::clone(&self.cache),
        })
    }

    /// Evaluates an XPath 1.0 expression at every element result, flattening the
    /// node-sets into parsel-shaped items: elements stay elements, text and
    /// attribute nodes become strings, scalars become one string.
    ///
    /// Text results cannot be queried further, as in parsel.
    pub fn xpath(&self, expr: &str) -> Result<Self> {
        let path = XPath::new(expr)?;
        let mut items = Vec::new();
        let mut roots = Vec::new();
        for item in &self.items {
            let Item::Record(record) = item else {
                continue;
            };
            if record.kind != MatchKind::Element {
                continue;
            }
            let context = NodeRef::element(record.node);
            match path.eval_at(&self.doc, context)? {
                Value::Nodes(nodes) => {
                    for node in nodes {
                        match node.kind() {
                            RefKind::Element => {
                                let record = element_record(&self.doc, node.node_id());
                                roots.push(record.node);
                                items.push(Item::Record(record));
                            }
                            RefKind::Text | RefKind::Attribute => {
                                items.push(Item::Text(node.string_value(&self.doc).into_owned()))
                            }
                            // parsel never sees the document node (lxml has none).
                            RefKind::Document => {}
                        }
                    }
                }
                Value::Str(text) => items.push(Item::Text(text)),
                Value::Number(number) => items.push(Item::Text(format_number(number))),
                Value::Bool(value) => {
                    items.push(Item::Text(if value { "1" } else { "0" }.to_string()));
                }
            }
        }
        Ok(Self {
            doc: Rc::clone(&self.doc),
            roots,
            items,
            cache: Rc::clone(&self.cache),
        })
    }

    /// Evaluates CSS and projects the result: `Count` avoids materializing
    /// records, `Texts` borrows from the document input.
    pub fn css_as<'b, P: Projection>(&'b self, query: &str) -> Result<P::Output<'b>> {
        let parsed = Self::compiled(query)?;
        select_as_within_cache::<P>(&self.doc, &parsed, &self.roots, &self.cache)
    }

    /// Counts CSS matches without materializing records.
    pub fn css_count(&self, query: &str) -> Result<usize> {
        self.css_as::<Count>(query)
    }

    /// First result's extracted string.  Element results are the original source
    /// slice, not a re-serialization; text and attribute results are decoded.
    pub fn get(&self) -> Option<Cow<'_, str>> {
        self.items.first().map(|item| self.item_value(item))
    }

    /// All result strings.
    pub fn getall(&self) -> Vec<Cow<'_, str>> {
        self.items
            .iter()
            .map(|item| self.item_value(item))
            .collect()
    }

    /// First result's text content.
    pub fn text(&self) -> Option<Cow<'_, str>> {
        match self.items.first()? {
            Item::Record(record) => record_text(&self.doc, record),
            Item::Text(text) => Some(Cow::Borrowed(text)),
        }
    }

    /// All result text contents.
    pub fn texts(&self) -> Vec<Cow<'_, str>> {
        self.items
            .iter()
            .filter_map(|item| match item {
                Item::Record(record) => record_text(&self.doc, record),
                Item::Text(text) => Some(Cow::Borrowed(text.as_str())),
            })
            .collect()
    }

    /// Attributes of the first element result.
    pub fn attrib(&self) -> Option<Vec<(String, String)>> {
        match self.items.first()? {
            Item::Record(record) => record_attrib(&self.doc, record),
            Item::Text(_) => None,
        }
    }

    fn item_value<'s>(&'s self, item: &'s Item) -> Cow<'s, str> {
        match item {
            Item::Record(record) => extract(&self.doc, record),
            Item::Text(text) => Cow::Borrowed(text),
        }
    }
}

impl Selector<'static> {
    /// Parses an owned buffer and selects from its root: the bytes move into the
    /// document and the selector carries no lifetime.
    ///
    /// ```
    /// use simdsel::Selector;
    ///
    /// let body = b"<p>hi</p>".to_vec();
    /// let sel = Selector::from_vec(body)?;
    /// assert_eq!(sel.css("p::text")?.get(), Some("hi".into()));
    /// # Ok::<(), simdsel::Error>(())
    /// ```
    pub fn from_vec(input: Vec<u8>) -> Result<Self> {
        Ok(Self::new(Document::parse_owned(input)?))
    }

    /// [`Self::from_vec`] with explicit [`ParseOptions`].
    pub fn from_vec_with(input: Vec<u8>, options: ParseOptions) -> Result<Self> {
        Ok(Self::new(Document::parse_owned_with(input, options)?))
    }

    /// [`Selector::from_text`] for an owned string: it neither borrows nor
    /// copies, and no charset sniffing runs.
    pub fn from_string(text: String) -> Result<Self> {
        Self::from_vec_with(
            text.into_bytes(),
            ParseOptions {
                encoding: EncodingMode::Utf8,
                ..Default::default()
            },
        )
    }
}

impl<'a> From<Document<'a>> for Selector<'a> {
    fn from(doc: Document<'a>) -> Self {
        Self::new(doc)
    }
}

/// One match record for an element node.
fn element_record(doc: &Document<'_>, id: NodeId) -> MatchRecord {
    let node = doc.node(id);
    MatchRecord {
        node: id,
        kind: MatchKind::Element,
        span: node.span(),
        outer_end: node.outer_end(),
        attr: None,
    }
}

/// The record a document selector starts with: the root element, or the
/// document node when the parse produced no element.
fn document_record(doc: &Document<'_>) -> MatchRecord {
    match root_element(doc) {
        Some(id) => element_record(doc, id),
        None => MatchRecord {
            node: DOC_ROOT,
            kind: MatchKind::Element,
            span: 0..0,
            outer_end: 0,
            attr: None,
        },
    }
}

/// Python's `repr(float)`, which is what parsel stringifies XPath numbers with.
fn format_number(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    format!("{value:?}")
}

/// Text content of one match record: subtree text for elements, the decoded
/// node for text records, nothing for attributes.
pub(crate) fn record_text<'a>(doc: &'a Document<'_>, record: &MatchRecord) -> Option<Cow<'a, str>> {
    match record.kind {
        MatchKind::Element => Some(Cow::Owned(crate::dom::text_content(doc, record.node))),
        MatchKind::Text => Some(doc.text(record.node)),
        MatchKind::Attr => None,
    }
}

/// Attributes of one element match record, deduplicated first-wins.  `None` for
/// text and attribute records.  Names are lowercased like parsel/lxml: HTML
/// attribute names are case-insensitive.
pub(crate) fn record_attrib(
    doc: &Document<'_>,
    record: &MatchRecord,
) -> Option<Vec<(String, String)>> {
    if record.kind != MatchKind::Element {
        return None;
    }
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for attr in crate::dom::attr_range(doc, record.node) {
        let name = crate::dom::attr_name(doc, attr);
        if seen
            .iter()
            .any(|s| s.eq_ignore_ascii_case(std::str::from_utf8(name).unwrap_or("")))
        {
            continue;
        }
        let name = String::from_utf8_lossy(name).to_ascii_lowercase();
        seen.push(name.clone());
        out.push((name, crate::dom::attr_value(doc, attr).into_owned()));
    }
    Some(out)
}
