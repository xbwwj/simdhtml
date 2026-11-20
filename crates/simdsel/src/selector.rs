//! 高层查询门面 API
//! parsel-shaped high-level selector facade.
//!
//! [`Selector`] *owns* its [`Document`], like parsel's `Selector` owns its
//! text: the caller parses once and keeps querying, without threading a
//! document reference through every call.
//!
//! ```no_run
//! use simdsel::Selector;
//!
//! let sel = Selector::from_text("<div class=item><a href=/x>hi</a></div>").unwrap();
//! assert_eq!(sel.css(".item a::attr(href)").unwrap().get(), Some("/x".into()));
//! ```
//!
//! Ownership follows the caller's buffer, and the zero-copy choice is per
//! entry point rather than a global policy:
//!
//! | caller has | constructor | document holds | cost |
//! | --- | --- | --- | --- |
//! | `Vec<u8>` (a response body) | [`Selector::from_vec`] | the buffer | none |
//! | `String` (decoded text) | [`Selector::from_string`] | the buffer | none |
//! | `&[u8]` / `&str` | [`Selector::from_bytes`] / [`Selector::from_text`] | a borrow | none |
//! | a parsed [`Document`] | [`Selector::new`] | the document | none |
//!
//! The owned constructors yield a `Selector<'static>`: no lifetime to thread
//! through callers.  The borrowed ones keep the lifetime of their input, which
//! is what a same-scope parse-select-throw-away loop wants.
//!
//! A `Selector` can represent either a document root (no records) or a result
//! set. Chained `css` calls evaluate inside the current roots.  The document is
//! held through an `Rc` so that deriving a result selector (or [`Selector::split`])
//! shares it instead of cloning the whole node tape.
//!
//! This is the crate's front door: `simdhtml` stays the parser, and its types
//! ([`Document`], [`ParseOptions`], ...) are re-exported here so a caller only
//! needs `simdsel`.

use std::borrow::Cow;
use std::rc::Rc;

use simdhtml::{Document, EncodingMode, NodeId, ParseOptions};

use crate::parser::Selector as CssSelector;
use crate::record::{MatchKind, MatchRecord};
use crate::{
    Count, Error, Projection, QueryCache, parse_selector, select_as_within_cache,
    select_within_cache,
};

/// An owned document or a result set, like parsel's
/// `Selector`/`SelectorList`.
#[derive(Clone, Debug)]
pub struct Selector<'a> {
    doc: Rc<Document<'a>>,
    roots: Vec<NodeId>,
    records: Vec<MatchRecord>,
    /// Derived query data (tag/attr masks, bare-compound masks, child index)
    /// shared by every query derived from this selector.
    ///
    /// Everything in it is a function of the (immutable) document, not of
    /// `roots`, so it stays valid as queries narrow the result set: it is built
    /// on demand by the first query that needs each piece and reused by the
    /// rest.  `Rc` because a selector is single-threaded by design.
    cache: Rc<QueryCache>,
}

impl<'a> Selector<'a> {
    /// Wraps an already parsed document and selects from its root.
    ///
    /// This is the ownership-taking constructor: the document becomes the
    /// selector's, so a caller that already parsed (or that needs an owned
    /// document - see [`Document::parse_owned`]) does not throw it away.  The
    /// borrowed-input constructors ([`Self::from_bytes`], [`Self::from_text`])
    /// are one-liners over this one.
    ///
    /// ```no_run
    /// use simdsel::{Document, Selector};
    ///
    /// let body: Vec<u8> = Vec::new(); // a fetched response body
    /// let sel = Selector::new(Document::parse_owned(body)?);
    /// # Ok::<(), simdsel::Error>(())
    /// ```
    #[must_use]
    pub fn new(doc: Document<'a>) -> Self {
        let root = doc.root;
        Self {
            doc: Rc::new(doc),
            roots: vec![root],
            records: Vec::new(),
            cache: Rc::new(QueryCache::root()),
        }
    }

    /// Parses `input` (default [`ParseOptions`]) and selects from its root.
    ///
    /// The input is bytes, like parsel's `body=`: the encoding is sniffed per
    /// the HTML specification.  Use [`Self::from_text`] when the caller already
    /// has a `&str`, and [`Selector::from_vec`] when the buffer is owned and the
    /// selector should outlive it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parse`] if the input is too large or the encoding
    /// could not be decoded.
    pub fn from_bytes(input: &'a [u8]) -> Result<Self, Error> {
        Ok(Self::new(Document::parse(input)?))
    }

    /// [`Self::from_bytes`] with explicit [`ParseOptions`]: repair mode and
    /// charset handling.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parse`] if parsing failed.
    pub fn from_bytes_with(input: &'a [u8], options: ParseOptions) -> Result<Self, Error> {
        Ok(Self::new(Document::parse_with(input, options)?))
    }

    /// Parses already decoded `text` and selects from its root.
    ///
    /// The bytes are taken as UTF-8 as they are: neither the BOM nor
    /// `<meta charset>` sniffing runs, because a `&str` is already text (a
    /// declared legacy charset would otherwise re-read valid UTF-8 as
    /// mojibake).  This is parsel's `text=` argument, sans the re-encoding.
    /// Use [`Selector::from_string`] when the text is owned.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parse`] if the input is too large.
    pub fn from_text(text: &'a str) -> Result<Self, Error> {
        Self::from_bytes_with(
            text.as_bytes(),
            ParseOptions::new().encoding(EncodingMode::Utf8),
        )
    }

    /// Splits a result set into one single-result selector per record.
    ///
    /// The derived selectors share the document and the query cache.
    #[must_use]
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
    #[must_use]
    pub fn doc(&self) -> &Document<'a> {
        &self.doc
    }

    /// Current search roots.
    #[must_use]
    pub fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    /// Current records.
    #[must_use]
    pub fn records(&self) -> &[MatchRecord] {
        &self.records
    }

    /// Number of records.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether there are no records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Compiled-selector intern table.
    ///
    /// `css*` takes selector *text*, like parsel, so the same query arrives as a
    /// string on every call.  Parsing it costs ~11% of a warm query (perf on a
    /// 3258-node document), and a crawl loop repeats a handful of selectors over
    /// and over, so the compiled form is interned per thread.  The table is
    /// bounded: a crawler that generates queries dynamically clears it rather
    /// than growing without limit.
    fn compiled(query: &str) -> Result<Rc<CssSelector>, Error> {
        use std::cell::RefCell;
        use std::collections::HashMap;

        thread_local! {
            static CACHE: RefCell<HashMap<Box<str>, Rc<CssSelector>>> = RefCell::new(HashMap::new());
        }
        /// Selectors are small; a few hundred is plenty and bounds memory.
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
    ///
    /// # Errors
    ///
    /// Returns [`Error::Selector`] for an invalid selector and [`Error::Match`]
    /// for evaluation limits.
    pub fn css(&self, query: &str) -> Result<Self, Error> {
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

    /// Evaluates CSS and projects the result with a [`Projection`].
    ///
    /// This is the GAT-based output mode: `Count` avoids materializing
    /// records; `Texts` borrows `Cow<str>` from the document input.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Selector`] for an invalid selector and [`Error::Match`]
    /// for evaluation limits.
    pub fn css_as<'b, P: Projection>(&'b self, query: &str) -> Result<P::Output<'b>, Error> {
        let parsed = Self::compiled(query)?;
        Ok(select_as_within_cache::<P>(
            &self.doc,
            &parsed,
            &self.roots,
            &self.cache,
        )?)
    }

    /// Counts CSS matches without materializing result records.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Selector`] for an invalid selector and [`Error::Match`]
    /// for evaluation limits.
    pub fn css_count(&self, query: &str) -> Result<usize, Error> {
        self.css_as::<Count>(query)
    }

    /// First result's extracted string.
    ///
    /// Element results are the original source slice, not a parsel/lxml
    /// re-serialization: quotes, entities, optional end tags and attribute
    /// spelling are preserved. Text/attribute results are entity-decoded.
    #[must_use]
    pub fn get(&self) -> Option<Cow<'_, str>> {
        self.records
            .first()
            .map(|record| crate::extract(&self.doc, record))
    }

    /// All result strings.
    #[must_use]
    pub fn getall(&self) -> Vec<Cow<'_, str>> {
        self.records
            .iter()
            .map(|record| crate::extract(&self.doc, record))
            .collect()
    }

    /// First result's text content.
    #[must_use]
    pub fn text(&self) -> Option<Cow<'_, str>> {
        self.records
            .first()
            .and_then(|record| record_text(&self.doc, record))
    }

    /// All result text contents.
    #[must_use]
    pub fn texts(&self) -> Vec<Cow<'_, str>> {
        self.records
            .iter()
            .filter_map(|record| record_text(&self.doc, record))
            .collect()
    }

    /// Attributes of the first element result.
    #[must_use]
    pub fn attrib(&self) -> Option<Vec<(String, String)>> {
        record_attrib(&self.doc, self.records.first()?)
    }
}

impl Selector<'static> {
    /// Parses an owned buffer and selects from its root.
    ///
    /// This is the *zero-copy owned* entry point: the bytes move into the
    /// document instead of being borrowed or copied, and the selector carries no
    /// lifetime, so it can be returned from the function that fetched the body
    /// and stored next to its results.  Use [`Selector::from_bytes`] to borrow a
    /// buffer that outlives the selector instead.
    ///
    /// ```no_run
    /// use simdsel::Selector;
    ///
    /// let body: Vec<u8> = Vec::new(); // a fetched response body
    /// let sel = Selector::from_vec(body)?;
    /// # Ok::<(), simdsel::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parse`] if the input is too large or the encoding could
    /// not be decoded.
    pub fn from_vec(input: Vec<u8>) -> Result<Self, Error> {
        Ok(Self::new(Document::parse_owned(input)?))
    }

    /// [`Self::from_vec`] with explicit [`ParseOptions`]: repair mode and charset
    /// handling.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parse`] if parsing failed.
    pub fn from_vec_with(input: Vec<u8>, options: ParseOptions) -> Result<Self, Error> {
        Ok(Self::new(Document::parse_owned_with(input, options)?))
    }

    /// Parses an owned, already decoded HTML string and selects from its root.
    ///
    /// Like [`Selector::from_text`], the text is taken as UTF-8 as it is: no BOM
    /// or `<meta charset>` sniffing, because a `String` is already text.  Like
    /// [`Self::from_vec`], it neither borrows nor copies.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Parse`] if the input is too large.
    pub fn from_string(text: String) -> Result<Self, Error> {
        Self::from_vec_with(
            text.into_bytes(),
            ParseOptions::new().encoding(EncodingMode::Utf8),
        )
    }
}

impl<'a> From<Document<'a>> for Selector<'a> {
    fn from(doc: Document<'a>) -> Self {
        Self::new(doc)
    }
}

/// Compiles `query` (through the per-thread intern table) and evaluates it
/// inside `roots`, reusing the chain-local [`QueryCache`].
///
/// This is the borrowed-parts entry point used by language bindings: it avoids
/// reconstructing a [`Selector`] just to run one query.
///
/// # Errors
///
/// Returns [`Error::Selector`] for an invalid selector and [`Error::Match`]
/// for evaluation limits.
pub fn select_query_within_cache(
    doc: &Document<'_>,
    roots: &[NodeId],
    cache: &QueryCache,
    query: &str,
) -> Result<Vec<MatchRecord>, Error> {
    let parsed = Selector::compiled(query)?;
    Ok(select_within_cache(doc, &parsed, roots, cache)?)
}

/// Text content represented by one match record.
///
/// Element records use the concatenated text of the subtree; text records use
/// the decoded text node; attribute records have no text projection.
#[must_use]
pub fn record_text<'a>(doc: &'a Document<'_>, record: &MatchRecord) -> Option<Cow<'a, str>> {
    match record.kind {
        MatchKind::Element => Some(Cow::Owned(crate::raw::text_content(doc, record.node))),
        MatchKind::Text => Some(doc.text(record.node)),
        MatchKind::Attr => None,
    }
}

/// Attributes of one element match record.
///
/// Returns `None` for text/attribute records. Duplicate attribute names are
/// reported once, matching HTML's first-attribute-wins rule.
#[must_use]
pub fn record_attrib(doc: &Document<'_>, record: &MatchRecord) -> Option<Vec<(String, String)>> {
    if record.kind != MatchKind::Element {
        return None;
    }
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for attr in crate::raw::attr_range(doc, record.node) {
        let name = crate::raw::attr_name(doc, attr);
        if seen
            .iter()
            .any(|s| s.eq_ignore_ascii_case(std::str::from_utf8(name).unwrap_or("")))
        {
            continue;
        }
        let name = String::from_utf8_lossy(name).into_owned();
        seen.push(name.clone());
        out.push((name, crate::raw::attr_value(doc, attr).into_owned()));
    }
    Some(out)
}
