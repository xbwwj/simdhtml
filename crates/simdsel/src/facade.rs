use std::borrow::Cow;
use std::rc::Rc;

use simdhtml::document::Document;
use simdhtml::document::id::{DOC_ROOT, NodeId};
use simdhtml::parse::{EncodingMode, ParseOptions};

use crate::css::ast::SelectorList;
use crate::css::cache::QueryCache;
use crate::css::parser::parse_selector;
use crate::css::select::{Count, Projection, extract, select_as_within_cache, select_within_cache};
use crate::error::Result;
use crate::record::{MatchKind, MatchRecord};

/// An owned document or a result set, like parsel's `Selector`/`SelectorList`.
#[derive(Clone, Debug)]
pub struct Selector<'a> {
    doc: Rc<Document<'a>>,
    roots: Vec<NodeId>,
    records: Vec<MatchRecord>,
    /// Derived query data (masks, child index) shared by every query from this
    /// selector.  A function of the document, not of `roots`, so it stays valid
    /// as queries narrow; built on demand.
    cache: Rc<QueryCache>,
}

impl<'a> Selector<'a> {
    /// Wraps an already parsed document and selects from its root.
    ///
    /// ```no_run
    /// use simdsel::prelude::{Document, Error, Selector};
    ///
    /// let body: Vec<u8> = Vec::new(); // a fetched response body
    /// let sel = Selector::new(Document::parse_owned(body)?);
    /// # Ok::<(), Error>(())
    /// ```
    pub fn new(doc: Document<'a>) -> Self {
        Self {
            doc: Rc::new(doc),
            roots: vec![DOC_ROOT],
            records: Vec::new(),
            cache: Rc::new(QueryCache::root()),
        }
    }

    /// Parses `input` as bytes (parsel's `body=`) and selects from its root.
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

    /// Splits a result set into one single-result selector per record; the
    /// derived selectors share the document and the query cache.
    pub fn split(&self) -> Vec<Self> {
        self.records
            .iter()
            .map(|record| Self {
                doc: Rc::clone(&self.doc),
                roots: vec![record.node],
                records: vec![record.clone()],
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

    /// Current records.
    pub fn records(&self) -> &[MatchRecord] {
        &self.records
    }

    /// Number of records.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether there are no records.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
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

    /// Compiles and evaluates a CSS selector inside the current roots.
    pub fn css(&self, query: &str) -> Result<Self> {
        let parsed = Self::compiled(query)?;
        let records = select_within_cache(&self.doc, &parsed, &self.roots, &self.cache)?;
        let roots = records.iter().map(|record| record.node).collect::<Vec<_>>();
        Ok(Self {
            doc: Rc::clone(&self.doc),
            roots,
            records,
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
        self.records
            .first()
            .map(|record| extract(&self.doc, record))
    }

    /// All result strings.
    pub fn getall(&self) -> Vec<Cow<'_, str>> {
        self.records
            .iter()
            .map(|record| extract(&self.doc, record))
            .collect()
    }

    /// First result's text content.
    pub fn text(&self) -> Option<Cow<'_, str>> {
        self.records
            .first()
            .and_then(|record| record_text(&self.doc, record))
    }

    /// All result text contents.
    pub fn texts(&self) -> Vec<Cow<'_, str>> {
        self.records
            .iter()
            .filter_map(|record| record_text(&self.doc, record))
            .collect()
    }

    /// Attributes of the first element result.
    pub fn attrib(&self) -> Option<Vec<(String, String)>> {
        record_attrib(&self.doc, self.records.first()?)
    }
}

impl Selector<'static> {
    /// Parses an owned buffer and selects from its root: the bytes move into the
    /// document and the selector carries no lifetime.
    ///
    /// ```no_run
    /// use simdsel::prelude::{Error, Selector};
    ///
    /// let body: Vec<u8> = Vec::new(); // a fetched response body
    /// let sel = Selector::from_vec(body)?;
    /// # Ok::<(), Error>(())
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

/// Compiles `query` (through the per-thread intern table) and evaluates it
/// inside `roots`, reusing the chain-local [`QueryCache`].  Used by language
/// bindings to run one query without rebuilding a [`Selector`].
pub fn select_query_within_cache(
    doc: &Document<'_>,
    roots: &[NodeId],
    cache: &QueryCache,
    query: &str,
) -> Result<Vec<MatchRecord>> {
    let parsed = Selector::compiled(query)?;
    select_within_cache(doc, &parsed, roots, cache)
}

/// Text content of one match record: subtree text for elements, the decoded
/// node for text records, nothing for attributes.
pub fn record_text<'a>(doc: &'a Document<'_>, record: &MatchRecord) -> Option<Cow<'a, str>> {
    match record.kind {
        MatchKind::Element => Some(Cow::Owned(crate::dom::text_content(doc, record.node))),
        MatchKind::Text => Some(doc.text(record.node)),
        MatchKind::Attr => None,
    }
}

/// Attributes of one element match record, deduplicated first-wins.  `None` for
/// text and attribute records.
pub fn record_attrib(doc: &Document<'_>, record: &MatchRecord) -> Option<Vec<(String, String)>> {
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
        let name = String::from_utf8_lossy(name).into_owned();
        seen.push(name.clone());
        out.push((name, crate::dom::attr_value(doc, attr).into_owned()));
    }
    Some(out)
}
