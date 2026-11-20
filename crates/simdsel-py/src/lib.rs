//! PyO3 bindings for simdsel.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use pyo3::exceptions::{PyIndexError, PyNotImplementedError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyAnyMethods, PySlice, PySliceMethods};
use regex_lite::Regex;

use encoding_rs::Encoding;
use simdhtml::{Document, EncodingMode, ParseOptions};
use simdsel::{
    MatchRecord, NodeId, QueryCache, extract, record_attrib, record_text, select_query_within_cache,
};

// The document owns its bytes (`Document::parse_owned_with`), so the binding
// needs no owner/dependent pair: `Rc<Document>` is the whole sharing story, and
// text/attribute values are decoded lazily from the document's source spans.

/// Parsel-shaped single selector.
#[pyclass(name = "Selector", module = "simdsel._simdsel", unsendable)]
#[derive(Clone)]
pub struct PySelector {
    doc: Rc<Document<'static>>,
    roots: Vec<NodeId>,
    records: Vec<MatchRecord>,
    cache: Rc<QueryCache>,
    base_url: Option<String>,
}

/// Parsel-shaped selector list.
#[pyclass(
    name = "SelectorList",
    module = "simdsel._simdsel",
    sequence,
    unsendable
)]
pub struct PySelectorList {
    doc: Rc<Document<'static>>,
    records: Vec<MatchRecord>,
    cache: Rc<QueryCache>,
    base_url: Option<String>,
}

impl PySelector {
    /// Parses `bytes`, taking ownership of them, with options already resolved.
    fn parse_owned(
        bytes: Vec<u8>,
        base_url: Option<String>,
        options: ParseOptions,
    ) -> PyResult<Self> {
        let doc = Document::parse_owned_with(bytes, options)
            .map_err(|err| PyValueError::new_err(err.to_string()))?;
        let root = doc.root;

        Ok(Self {
            doc: Rc::new(doc),
            roots: vec![root],
            records: Vec::new(),
            cache: Rc::new(QueryCache::root()),
            base_url,
        })
    }

    fn from_record(
        doc: Rc<Document<'static>>,
        cache: Rc<QueryCache>,
        record: &MatchRecord,
        base_url: Option<String>,
    ) -> Self {
        Self {
            doc,
            roots: vec![record.node],
            records: vec![record.clone()],
            cache,
            base_url,
        }
    }
}

#[pymethods]
impl PySelector {
    #[new]
    #[pyo3(signature = (text=None, body=None, encoding=None, base_url=None, r#type=None, mode=None))]
    fn new(
        text: Option<&str>,
        body: Option<&[u8]>,
        encoding: Option<String>,
        base_url: Option<String>,
        r#type: Option<String>,
        mode: Option<&str>,
    ) -> PyResult<Self> {
        if let Some(kind) = r#type.as_deref()
            && kind.eq_ignore_ascii_case("xml")
        {
            return Err(PyNotImplementedError::new_err(
                "simdsel v1 only supports type='html'",
            ));
        }

        let encoding = explicit_encoding(encoding.as_deref())?;
        let repair = repair_mode(mode)?;
        let (bytes, options) = match (text, body) {
            // `text` is already Unicode.  Like parsel, an explicit `encoding=`
            // round-trips it through that encoding before parsing (characters
            // the encoding cannot represent end up as character references
            // instead of being lost), and an absent one leaves the bytes as the
            // UTF-8 they are: `<meta charset>` in already decoded text is
            // document data, not a hint that should re-read it as legacy bytes.
            (Some(text), _) => match encoding {
                Some(encoding) => (
                    encoding.encode(text).0.into_owned(),
                    ParseOptions::new()
                        .repair(repair)
                        .encoding(EncodingMode::Explicit(encoding)),
                ),
                None => (
                    text.as_bytes().to_vec(),
                    ParseOptions::new()
                        .repair(repair)
                        .encoding(EncodingMode::Utf8),
                ),
            },
            // `body=` is bytes off the wire, so the charset comes from `encoding=`
            // or, absent that, from the spec's sniffing algorithm (the
            // documented deviation from parsel's hard-coded utf-8).
            (None, Some(body)) => (
                body.to_vec(),
                match encoding {
                    Some(encoding) => ParseOptions::new()
                        .repair(repair)
                        .encoding(EncodingMode::Explicit(encoding)),
                    None => ParseOptions::new().repair(repair),
                },
            ),
            (None, None) => (Vec::new(), ParseOptions::new().repair(repair)),
        };

        Self::parse_owned(bytes, base_url, options)
    }

    /// Parses a list of byte buffers sequentially.
    ///
    /// This is the optional `parse_many` batch API; it has the same semantics
    /// as calling `Selector(body=...)` for each item.
    #[staticmethod]
    fn parse_many(bodies: Vec<Vec<u8>>) -> PyResult<Vec<PySelector>> {
        bodies
            .into_iter()
            .map(|body| PySelector::parse_owned(body, None, ParseOptions::new()))
            .collect()
    }

    /// Base URL (stored only; simdsel never fetches).
    #[getter]
    fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    /// Compiles and evaluates a CSS selector.
    fn css(&self, query: &str) -> PyResult<PySelectorList> {
        let records = select_query_within_cache(&self.doc, &self.roots, &self.cache, query)
            .map_err(to_py_err)?;
        Ok(PySelectorList {
            doc: Rc::clone(&self.doc),
            records,
            cache: Rc::clone(&self.cache),
            base_url: self.base_url.clone(),
        })
    }

    /// Serialized current value.
    fn get(&self) -> Option<String> {
        self.records
            .first()
            .map(|record| extract(&self.doc, record).into_owned())
    }

    /// Serialized values (a one-element list for a Selector).
    fn getall(&self) -> Vec<String> {
        self.records
            .iter()
            .map(|record| extract(&self.doc, record).into_owned())
            .collect()
    }

    /// `extract()` alias for `get()`.
    fn extract(&self) -> Option<String> {
        self.get()
    }

    /// `extract_first()` alias for `get()`.
    fn extract_first(&self) -> Option<String> {
        self.get()
    }

    /// Text content of the current result.
    #[getter]
    fn text(&self) -> Option<String> {
        self.records
            .first()
            .and_then(|record| record_text(&self.doc, record))
            .map(|value| value.into_owned())
    }

    /// Attributes of the first element result.
    #[getter]
    fn attrib(&self) -> HashMap<String, String> {
        self.records
            .first()
            .and_then(|record| record_attrib(&self.doc, record))
            .unwrap_or_default()
            .into_iter()
            .collect()
    }

    /// Apply a regex to `get()`.
    #[pyo3(signature = (pattern, replace_entities=true))]
    fn re(&self, pattern: &str, replace_entities: bool) -> PyResult<Vec<String>> {
        let Some(record) = self.records.first() else {
            return Ok(Vec::new());
        };
        extract_regex(
            &with_entities(extract(&self.doc, record), replace_entities),
            pattern,
        )
    }

    /// First regex match or `default`.
    #[pyo3(signature = (pattern, default=None, replace_entities=true))]
    fn re_first(
        &self,
        pattern: &str,
        default: Option<String>,
        replace_entities: bool,
    ) -> PyResult<Option<String>> {
        match self.records.first() {
            Some(record) => Ok(extract_regex_first(
                &with_entities(extract(&self.doc, record), replace_entities),
                pattern,
            )?
            .or(default)),
            None => Ok(default),
        }
    }

    fn __bool__(&self) -> bool {
        self.records
            .first()
            .is_some_and(|record| !extract(&self.doc, record).is_empty())
    }

    fn __str__(&self) -> String {
        self.get().unwrap_or_default()
    }

    fn __repr__(&self) -> String {
        format!("<Selector data={:?}>", self.get().unwrap_or_default())
    }
}

#[pymethods]
impl PySelectorList {
    /// Number of selected nodes.
    fn __len__(&self) -> usize {
        self.records.len()
    }

    /// Integer index or Python slice access.
    fn __getitem__(&self, py: Python<'_>, index: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        if let Ok(index) = index.extract::<isize>() {
            return Ok(Py::new(py, self.item_at(index)?)?.into_any());
        }
        if let Ok(slice) = index.cast::<PySlice>() {
            let indices = slice.indices(self.records.len() as isize)?;
            let mut records = Vec::with_capacity(indices.slicelength);
            for k in 0..indices.slicelength {
                let idx = indices.start + k as isize * indices.step;
                records.push(self.records[idx as usize].clone());
            }
            return Ok(Py::new(
                py,
                PySelectorList {
                    doc: Rc::clone(&self.doc),
                    records,
                    cache: Rc::clone(&self.cache),
                    base_url: self.base_url.clone(),
                },
            )?
            .into_any());
        }
        Err(PyTypeError::new_err(
            "SelectorList index must be int or slice",
        ))
    }

    /// Applies CSS to every item and flattens the result.
    fn css(&self, query: &str) -> PyResult<PySelectorList> {
        let mut records = Vec::new();
        for record in &self.records {
            let mut selected = select_query_within_cache(
                &self.doc,
                std::slice::from_ref(&record.node),
                &self.cache,
                query,
            )
            .map_err(to_py_err)?;
            records.append(&mut selected);
        }
        Ok(PySelectorList {
            doc: Rc::clone(&self.doc),
            records,
            cache: Rc::clone(&self.cache),
            base_url: self.base_url.clone(),
        })
    }

    /// First item's value, or `default` when empty.
    #[pyo3(signature = (default=None))]
    fn get(&self, default: Option<String>) -> Option<String> {
        self.records
            .first()
            .map(|record| extract(&self.doc, record).into_owned())
            .or(default)
    }

    /// All items' values.
    fn getall(&self) -> Vec<String> {
        self.records
            .iter()
            .map(|record| extract(&self.doc, record).into_owned())
            .collect()
    }

    /// `extract()` alias for `getall()`.
    fn extract(&self) -> Vec<String> {
        self.getall()
    }

    /// `extract_first()` alias for `get()`.
    #[pyo3(signature = (default=None))]
    fn extract_first(&self, default: Option<String>) -> Option<String> {
        self.get(default)
    }

    /// All items' text content.
    #[getter]
    fn text(&self) -> Vec<String> {
        self.records
            .iter()
            .filter_map(|record| record_text(&self.doc, record))
            .map(|value| value.into_owned())
            .collect()
    }

    /// First item's attributes.
    #[getter]
    fn attrib(&self) -> HashMap<String, String> {
        self.records
            .first()
            .and_then(|record| record_attrib(&self.doc, record))
            .unwrap_or_default()
            .into_iter()
            .collect()
    }

    /// Apply a regex to every item and flatten.
    #[pyo3(signature = (pattern, replace_entities=true))]
    fn re(&self, pattern: &str, replace_entities: bool) -> PyResult<Vec<String>> {
        let mut out = Vec::new();
        for record in &self.records {
            out.extend(extract_regex(
                &with_entities(extract(&self.doc, record), replace_entities),
                pattern,
            )?);
        }
        Ok(out)
    }

    /// First regex match or `default`.
    #[pyo3(signature = (pattern, default=None, replace_entities=true))]
    fn re_first(
        &self,
        pattern: &str,
        default: Option<String>,
        replace_entities: bool,
    ) -> PyResult<Option<String>> {
        for record in &self.records {
            if let Some(matched) = extract_regex_first(
                &with_entities(extract(&self.doc, record), replace_entities),
                pattern,
            )? {
                return Ok(Some(matched));
            }
        }
        Ok(default)
    }

    fn __bool__(&self) -> bool {
        !self.records.is_empty()
    }

    fn __repr__(&self) -> String {
        format!("<SelectorList len={}>", self.records.len())
    }
}

impl PySelectorList {
    fn item_at(&self, index: isize) -> PyResult<PySelector> {
        let len = self.records.len() as isize;
        let index = if index < 0 { len + index } else { index };
        if index < 0 || index >= len {
            return Err(PyIndexError::new_err("SelectorList index out of range"));
        }
        Ok(PySelector::from_record(
            Rc::clone(&self.doc),
            Rc::clone(&self.cache),
            &self.records[index as usize],
            self.base_url.clone(),
        ))
    }
}

/// The `encoding=` keyword: a charset label such as `"utf-8"` or `"gbk"`.
///
/// `None` (or an empty label) means "let the parser sniff", which is what
/// parsel does when no encoding is passed.
fn explicit_encoding(label: Option<&str>) -> PyResult<Option<&'static Encoding>> {
    let Some(label) = label.map(str::trim).filter(|label| !label.is_empty()) else {
        return Ok(None);
    };
    simdhtml::encoding::lookup(label)
        .map(Some)
        .ok_or_else(|| PyValueError::new_err(format!("unknown encoding {label:?}")))
}

/// The `mode=` keyword: which tree-construction mode to parse with.
///
/// `none` is the default (no repair; the tree is the flat start/end-tag
/// result), `practical` keeps structural repairs but skips the
/// formatting-element machinery, and `spec` is full WHATWG compatibility.
fn repair_mode(mode: Option<&str>) -> PyResult<simdhtml::RepairMode> {
    Ok(match mode.unwrap_or("none").to_ascii_lowercase().as_str() {
        "none" | "" => simdhtml::RepairMode::None,
        "practical" => simdhtml::RepairMode::Practical,
        "spec" => simdhtml::RepairMode::Spec,
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown parse mode {other:?}: expected \"none\", \"practical\" or \"spec\""
            )));
        }
    })
}

/// Compiled regex cache: crawl loops usually reuse a handful of patterns.
fn cached_regex(pattern: &str) -> PyResult<Rc<Regex>> {
    thread_local! {
        static CACHE: RefCell<HashMap<Box<str>, Rc<Regex>>> = RefCell::new(HashMap::new());
    }
    /// Regexes are small; a few hundred is plenty and bounds memory.
    const MAX_ENTRIES: usize = 128;

    CACHE.with(|cache| {
        if let Some(regex) = cache.borrow().get(pattern) {
            return Ok(Rc::clone(regex));
        }
        let regex = Rc::new(
            Regex::new(pattern)
                .map_err(|err| PyValueError::new_err(format!("invalid regex: {err}")))?,
        );
        let mut cache = cache.borrow_mut();
        if cache.len() >= MAX_ENTRIES {
            cache.clear();
        }
        cache.insert(pattern.into(), Rc::clone(&regex));
        Ok(regex)
    })
}

/// Applies parsel's `replace_entities` to the string a regex runs against.
///
/// Text nodes and attribute values are decoded when they are projected, so this
/// only changes anything for element results (whose serialized HTML keeps its
/// references) and for literal references in raw-text elements.  Decoding is
/// skipped when the string has no references, which is the common case.
fn with_entities<'a>(text: Cow<'a, str>, replace_entities: bool) -> Cow<'a, str> {
    if !replace_entities {
        return text;
    }
    match simdhtml::decode_character_references(&text, false) {
        // Nothing decoded: keep the original, borrowed or not.
        Cow::Borrowed(_) => text,
        Cow::Owned(decoded) => Cow::Owned(decoded),
    }
}

fn extract_regex(text: &str, pattern: &str) -> PyResult<Vec<String>> {
    let regex = cached_regex(pattern)?;
    let groups = regex.captures_len();
    let extract_group = regex
        .capture_names()
        .position(|name| name == Some("extract"));
    let mut out = Vec::new();
    for captures in regex.captures_iter(text) {
        if let Some(group) = extract_group {
            if let Some(matched) = captures.get(group) {
                out.push(matched.as_str().to_string());
            }
        } else if groups <= 1 {
            if let Some(matched) = captures.get(0) {
                out.push(matched.as_str().to_string());
            }
        } else {
            for group in 1..groups {
                if let Some(matched) = captures.get(group) {
                    out.push(matched.as_str().to_string());
                }
            }
        }
    }
    Ok(out)
}

fn extract_regex_first(text: &str, pattern: &str) -> PyResult<Option<String>> {
    let regex = cached_regex(pattern)?;
    let groups = regex.captures_len();
    let extract_group = regex
        .capture_names()
        .position(|name| name == Some("extract"));
    for captures in regex.captures_iter(text) {
        if let Some(group) = extract_group {
            if let Some(matched) = captures.get(group) {
                return Ok(Some(matched.as_str().to_string()));
            }
        } else if groups <= 1 {
            if let Some(matched) = captures.get(0) {
                return Ok(Some(matched.as_str().to_string()));
            }
        } else {
            for group in 1..groups {
                if let Some(matched) = captures.get(group) {
                    return Ok(Some(matched.as_str().to_string()));
                }
            }
        }
    }
    Ok(None)
}

fn to_py_err(error: simdsel::Error) -> PyErr {
    PyValueError::new_err(error.to_string())
}

/// Python extension module `simdsel._simdsel`.
#[pymodule]
fn _simdsel(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PySelector>()?;
    m.add_class::<PySelectorList>()?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
