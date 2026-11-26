use std::borrow::Cow;

use simdhtml::document::Document;
use simdhtml::document::id::NodeId;

use crate::bitset::BitSet;
use crate::error::Error;
use crate::record::{MatchKind, MatchRecord};

use super::ast::SelectorList;
use super::cache::QueryCache;
use super::eval::{Eval, common_pseudo};

/// A typed projection of a selector result: the same match mask materialized as
/// records, strings, text or a count.  The lifetime lets borrowed results point
/// into the document input.
pub trait Projection {
    type Output<'a>;

    /// Projects an already-computed match mask.
    fn finish<'a>(
        doc: &'a Document<'_>,
        selector: &SelectorList,
        mask: BitSet,
    ) -> Result<Self::Output<'a>, Error>;
}

/// Raw `MatchRecord` projection.
pub struct Records;

impl Projection for Records {
    type Output<'a> = Vec<MatchRecord>;

    fn finish<'a>(
        doc: &'a Document<'_>,
        selector: &SelectorList,
        mask: BitSet,
    ) -> Result<Self::Output<'a>, Error> {
        {
            let cache = QueryCache::root();
            Ok(Eval::new(doc, &cache).records_for(selector, &mask))
        }
    }
}

/// Count-only projection: no records allocated.
pub struct Count;

impl Projection for Count {
    type Output<'a> = usize;

    fn finish<'a>(
        doc: &'a Document<'_>,
        selector: &SelectorList,
        mask: BitSet,
    ) -> Result<Self::Output<'a>, Error> {
        if common_pseudo(selector).is_some() {
            {
                let cache = QueryCache::root();
                Ok(Eval::new(doc, &cache).records_for(selector, &mask).len())
            }
        } else {
            Ok(mask.count_ones() as usize)
        }
    }
}

/// `getall()`-style string projection.
pub struct Html;

impl Projection for Html {
    type Output<'a> = Vec<Cow<'a, str>>;

    fn finish<'a>(
        doc: &'a Document<'_>,
        selector: &SelectorList,
        mask: BitSet,
    ) -> Result<Self::Output<'a>, Error> {
        let cache = QueryCache::root();
        Ok(Eval::new(doc, &cache)
            .records_for(selector, &mask)
            .into_iter()
            .map(|record| extract(doc, &record))
            .collect())
    }
}

/// Projection to text content / text nodes / attribute values.
pub struct Texts;

impl Projection for Texts {
    type Output<'a> = Vec<Cow<'a, str>>;

    fn finish<'a>(
        doc: &'a Document<'_>,
        selector: &SelectorList,
        mask: BitSet,
    ) -> Result<Self::Output<'a>, Error> {
        let cache = QueryCache::root();
        Ok(Eval::new(doc, &cache)
            .records_for(selector, &mask)
            .into_iter()
            .map(|record| match record.kind {
                MatchKind::Element => Cow::Owned(crate::dom::text_content(doc, record.node)),
                MatchKind::Text => doc.text(record.node),
                MatchKind::Attr => {
                    let span = record.attr.as_ref().expect("attr record");
                    crate::dom::attr_value_span(doc, span)
                }
            })
            .collect())
    }
}

/// Selects all elements matching `selector`, in document order.
pub fn select(doc: &Document<'_>, selector: &SelectorList) -> Result<Vec<MatchRecord>, Error> {
    let cache = QueryCache::root();
    let ctx = Eval::new(doc, &cache);
    ctx.select(selector, None)
}

/// Selects matching elements inside the subtrees of `roots`: what chained
/// `SelectorList.css()` uses.
pub fn select_within(
    doc: &Document<'_>,
    selector: &SelectorList,
    roots: &[NodeId],
) -> Result<Vec<MatchRecord>, Error> {
    let cache = QueryCache::root();
    let ctx = Eval::new(doc, &cache);
    ctx.select(selector, Some(roots))
}

/// Selects within `roots` using a chain-local cache.
pub fn select_within_cache(
    doc: &Document<'_>,
    selector: &SelectorList,
    roots: &[NodeId],
    cache: &QueryCache,
) -> Result<Vec<MatchRecord>, Error> {
    Eval::new(doc, cache).select(selector, Some(roots))
}

/// Evaluates a projection within `roots` using a chain-local cache.
pub fn select_as_within_cache<'a, P: Projection>(
    doc: &'a Document<'_>,
    selector: &SelectorList,
    roots: &[NodeId],
    cache: &QueryCache,
) -> Result<P::Output<'a>, Error> {
    let ctx = Eval::new(doc, cache);
    let mask = ctx.mask_for(selector, Some(roots))?;
    P::finish(doc, selector, mask)
}

/// Counts matches without materializing records.  Pseudo-elements fall back to
/// record projection so `::text` / `::attr()` counts stay correct.
pub fn select_count(doc: &Document<'_>, selector: &SelectorList) -> Result<usize, Error> {
    {
        let cache = QueryCache::root();
        Eval::new(doc, &cache).count(selector, None)
    }
}

/// Like [`select_count`], but restricted to the subtrees of `roots`.
pub fn select_count_within(
    doc: &Document<'_>,
    selector: &SelectorList,
    roots: &[NodeId],
) -> Result<usize, Error> {
    {
        let cache = QueryCache::root();
        Eval::new(doc, &cache).count(selector, Some(roots))
    }
}

/// Evaluates `selector` and projects the result with `P`.
pub fn select_as<'a, P: Projection>(
    doc: &'a Document<'_>,
    selector: &SelectorList,
) -> Result<P::Output<'a>, Error> {
    let cache = QueryCache::root();
    let ctx = Eval::new(doc, &cache);
    let mask = ctx.mask_for(selector, None)?;
    P::finish(doc, selector, mask)
}

/// Like [`select_as`], but restricted to the subtrees of `roots`.
pub fn select_as_within<'a, P: Projection>(
    doc: &'a Document<'_>,
    selector: &SelectorList,
    roots: &[NodeId],
) -> Result<P::Output<'a>, Error> {
    let cache = QueryCache::root();
    let ctx = Eval::new(doc, &cache);
    let mask = ctx.mask_for(selector, Some(roots))?;
    P::finish(doc, selector, mask)
}

/// Extracts the text/HTML represented by a match record.
pub fn extract<'a>(doc: &'a Document<'_>, record: &MatchRecord) -> std::borrow::Cow<'a, str> {
    match record.kind {
        MatchKind::Element => {
            let bytes = crate::dom::outer_html(doc, record.node);
            std::borrow::Cow::Borrowed(
                std::str::from_utf8(bytes).expect("Document input is valid UTF-8"),
            )
        }
        MatchKind::Text => doc.text(record.node),
        MatchKind::Attr => {
            let span = record.attr.as_ref().expect("attr record has a value span");
            crate::dom::attr_value_span(doc, span)
        }
    }
}
