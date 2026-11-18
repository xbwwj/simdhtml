//! 高层查询门面 API
//! parsel-shaped high-level selector facade.
//!
//! [`Selector`] borrows an immutable [`FlatDoc`] plus the current result
//! records. It is the thin layer used by the Python binding and the examples:
//! parse once, then run repeated CSS queries over the same document.
//!
//! A `Selector` can represent either a document root (no records) or a result
//! set. Chained `css` calls evaluate inside the current roots.

use std::borrow::Cow;

use simdhtml::{FlatDoc, NodeId};

use crate::record::{MatchKind, MatchRecord};
use crate::{
    Count, Error, Projection, QueryCache, parse_selector, select_as_within_cache,
    select_within_cache,
};

/// A borrowed document or a result set, like parsel's
/// `Selector`/`SelectorList`.
#[derive(Clone, Debug)]
pub struct Selector<'a> {
    doc: &'a FlatDoc<'a>,
    roots: Vec<NodeId>,
    records: Vec<MatchRecord>,
}

impl<'a> Selector<'a> {
    /// Wraps an existing flat document.
    #[must_use]
    pub fn new(doc: &'a FlatDoc<'a>) -> Self {
        Self {
            doc,
            roots: vec![doc.root],
            records: Vec::new(),
        }
    }

    /// Reconstructs a selector from its parts (used by language bindings).
    #[must_use]
    pub fn from_parts(doc: &'a FlatDoc<'a>, roots: Vec<NodeId>, records: Vec<MatchRecord>) -> Self {
        Self {
            doc,
            roots,
            records,
        }
    }

    /// Splits a result set into one single-result selector per record.
    #[must_use]
    pub fn split(&self) -> Vec<Self> {
        self.records
            .iter()
            .map(|record| Self {
                doc: self.doc,
                roots: vec![record.node],
                records: vec![record.clone()],
            })
            .collect()
    }

    /// The underlying borrowed document.
    #[must_use]
    pub fn doc(&self) -> &'a FlatDoc<'a> {
        self.doc
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

    /// Compiles and evaluates a CSS selector inside the current roots.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Selector`] for an invalid selector and [`Error::Match`]
    /// for evaluation limits.
    pub fn css(&self, query: &str) -> Result<Self, Error> {
        let parsed = parse_selector(query)?;
        let cache = QueryCache::root();
        let records = select_within_cache(self.doc, &parsed, &self.roots, &cache)?;
        let roots = records.iter().map(|record| record.node).collect::<Vec<_>>();
        Ok(Self {
            doc: self.doc,
            roots,
            records,
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
        let parsed = parse_selector(query)?;
        let cache = QueryCache::root();
        Ok(select_as_within_cache::<P>(
            self.doc,
            &parsed,
            &self.roots,
            &cache,
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
            .map(|record| crate::extract(self.doc, record))
    }

    /// All result strings.
    #[must_use]
    pub fn getall(&self) -> Vec<Cow<'_, str>> {
        self.records
            .iter()
            .map(|record| crate::extract(self.doc, record))
            .collect()
    }

    /// First result's text content.
    #[must_use]
    pub fn text(&self) -> Option<Cow<'_, str>> {
        self.records.first().and_then(|record| self.text_of(record))
    }

    /// All result text contents.
    #[must_use]
    pub fn texts(&self) -> Vec<Cow<'_, str>> {
        self.records
            .iter()
            .filter_map(|record| self.text_of(record))
            .collect()
    }

    /// Attributes of the first element result.
    #[must_use]
    pub fn attrib(&self) -> Option<Vec<(String, String)>> {
        let record = self.records.first()?;
        if record.kind != MatchKind::Element {
            return None;
        }
        let mut out = Vec::new();
        let mut seen: Vec<std::ops::Range<u32>> = Vec::new();
        for attr in self.doc.attr_iter(record.node) {
            let name = self.doc.raw_attr_name(&attr);
            if seen
                .iter()
                .any(|span| self.doc.raw_attr_name_span(span).eq_ignore_ascii_case(name))
            {
                continue;
            }
            seen.push(attr.name.clone());
            out.push((
                String::from_utf8_lossy(self.doc.raw_attr_name(&attr)).into_owned(),
                self.doc.attr_value(&attr).into_owned(),
            ));
        }
        Some(out)
    }

    fn text_of(&self, record: &MatchRecord) -> Option<Cow<'_, str>> {
        match record.kind {
            MatchKind::Element => Some(Cow::Owned(self.doc.text_content(record.node))),
            MatchKind::Text => Some(self.doc.text_of(record.node)),
            MatchKind::Attr => None,
        }
    }
}
