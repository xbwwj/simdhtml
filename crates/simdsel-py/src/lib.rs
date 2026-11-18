//! PyO3 bindings for simdsel.
//!
//! The public Python module is `simdsel`, with a thin pure-Python `__init__.py`
//! re-exporting the classes from `simdsel._simdsel`.  The binding is intentionally
//! a facade over the Rust [`simdsel::Selector`] type: parsing/matching happen in
//! Rust, and `get()`/`getall()` slice the original input.
//!
//! `PySelector` owns the input bytes inside a [`self_cell`] and stores a
//! [`simdhtml::FlatDoc`] that borrows those bytes.  Rust-side `Selector`s are
//! constructed only for the duration of a method call.

#![warn(missing_docs)]

use std::collections::HashMap;
use std::rc::Rc;

use pyo3::exceptions::{PyIndexError, PyNotImplementedError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyAnyMethods, PySlice, PySliceMethods};
use regex_lite::Regex;
use self_cell::self_cell;

use simdhtml::{FlatDoc, ParseMode};
use simdsel::{MatchRecord, NodeId, Selector as CoreSelector};

self_cell!(
    struct PyDoc {
        owner: Box<[u8]>,

        #[covariant]
        dependent: FlatDoc,
    }
);

/// Parsel-shaped single selector.
#[pyclass(name = "Selector", module = "simdsel._simdsel", unsendable)]
#[derive(Clone)]
pub struct PySelector {
    doc: Rc<PyDoc>,
    roots: Vec<NodeId>,
    records: Vec<MatchRecord>,
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
    items: Vec<PySelector>,
}

impl PySelector {
    fn from_bytes(bytes: Box<[u8]>, base_url: Option<String>) -> PyResult<Self> {
        let doc = PyDoc::try_new(bytes, |owner| simdhtml::parse(owner, ParseMode::Fast))
            .map_err(|err| PyValueError::new_err(err.to_string()))?;
        let root = doc.borrow_dependent().root;
        Ok(Self {
            doc: Rc::new(doc),
            roots: vec![root],
            records: Vec::new(),
            base_url,
        })
    }

    fn with_core<R>(&self, f: impl for<'a> FnOnce(CoreSelector<'a>) -> R) -> R {
        self.doc.with_dependent(|_, doc| {
            f(CoreSelector::from_parts(
                doc,
                self.roots.clone(),
                self.records.clone(),
            ))
        })
    }

    fn from_core(doc: Rc<PyDoc>, result: CoreSelector<'_>, base_url: Option<String>) -> Self {
        Self {
            doc,
            roots: result.roots().to_vec(),
            records: result.records().to_vec(),
            base_url,
        }
    }
}

#[pymethods]
impl PySelector {
    #[new]
    #[pyo3(signature = (text=None, body=None, encoding=None, base_url=None, r#type=None))]
    fn new(
        text: Option<&str>,
        body: Option<&[u8]>,
        encoding: Option<String>,
        base_url: Option<String>,
        r#type: Option<String>,
    ) -> PyResult<Self> {
        if let Some(kind) = r#type.as_deref()
            && kind.eq_ignore_ascii_case("xml")
        {
            return Err(PyNotImplementedError::new_err(
                "simdsel v1 only supports type='html'",
            ));
        }
        let _ = encoding;
        let bytes: Vec<u8> = match (text, body) {
            (Some(text), _) => text.as_bytes().to_vec(),
            (None, Some(body)) => body.to_vec(),
            (None, None) => Vec::new(),
        };
        Self::from_bytes(bytes.into_boxed_slice(), base_url)
    }

    /// Parses a list of byte buffers sequentially.
    ///
    /// This is the optional `parse_many` batch API; it has the same semantics
    /// as calling `Selector(body=...)` for each item.
    #[staticmethod]
    fn parse_many(bodies: Vec<Vec<u8>>) -> PyResult<Vec<PySelector>> {
        bodies
            .into_iter()
            .map(|body| PySelector::from_bytes(body.into_boxed_slice(), None))
            .collect()
    }

    /// Base URL (stored only; simdsel never fetches).
    #[getter]
    fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    /// Compiles and evaluates a CSS selector.
    fn css(&self, query: &str) -> PyResult<PySelectorList> {
        self.with_core(|core| {
            let result = core.css(query).map_err(to_py_err)?;
            Ok(PySelectorList::from_core(
                Rc::clone(&self.doc),
                result,
                self.base_url.clone(),
            ))
        })
    }

    /// Serialized current value.
    fn get(&self) -> Option<String> {
        self.with_core(|core| core.get().map(|value| value.into_owned()))
    }

    /// Serialized values (a one-element list for a Selector).
    fn getall(&self) -> Vec<String> {
        self.with_core(|core| {
            core.getall()
                .into_iter()
                .map(|value| value.into_owned())
                .collect()
        })
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
        self.with_core(|core| core.text().map(|value| value.into_owned()))
    }

    /// Attributes of the first element result.
    #[getter]
    fn attrib(&self) -> HashMap<String, String> {
        self.with_core(|core| attributes(&core))
    }

    /// Apply a regex to `get()`.
    #[pyo3(signature = (pattern, replace_entities=true))]
    fn re(&self, pattern: &str, replace_entities: bool) -> PyResult<Vec<String>> {
        let _ = replace_entities;
        let Some(text) = self.get() else {
            return Ok(Vec::new());
        };
        extract_regex(&text, pattern)
    }

    /// First regex match or `default`.
    #[pyo3(signature = (pattern, default=None, replace_entities=true))]
    fn re_first(
        &self,
        pattern: &str,
        default: Option<String>,
        replace_entities: bool,
    ) -> PyResult<Option<String>> {
        let mut matches = self.re(pattern, replace_entities)?;
        if matches.is_empty() {
            Ok(default)
        } else {
            Ok(Some(matches.remove(0)))
        }
    }

    fn __bool__(&self) -> bool {
        self.get().is_some_and(|value| !value.is_empty())
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
        self.items.len()
    }

    /// Integer index or Python slice access.
    fn __getitem__(&self, py: Python<'_>, index: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        if let Ok(index) = index.extract::<isize>() {
            return Ok(Py::new(py, self.item_at(index)?)?.into_any());
        }
        if let Ok(slice) = index.cast::<PySlice>() {
            let indices = slice.indices(self.items.len() as isize)?;
            let mut out = Vec::with_capacity(indices.slicelength);
            for k in 0..indices.slicelength {
                let idx = indices.start + k as isize * indices.step;
                out.push(self.items[idx as usize].clone());
            }
            return Ok(Py::new(py, PySelectorList { items: out })?.into_any());
        }
        Err(PyTypeError::new_err(
            "SelectorList index must be int or slice",
        ))
    }

    /// Applies CSS to every item and flattens the result.
    fn css(&self, query: &str) -> PyResult<PySelectorList> {
        let mut out = Vec::new();
        for item in &self.items {
            out.extend(item.css(query)?.items);
        }
        Ok(PySelectorList { items: out })
    }

    /// First item's value, or `default` when empty.
    #[pyo3(signature = (default=None))]
    fn get(&self, default: Option<String>) -> Option<String> {
        self.items.first().and_then(PySelector::get).or(default)
    }

    /// All items' values.
    fn getall(&self) -> Vec<String> {
        self.items.iter().flat_map(PySelector::getall).collect()
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
        self.items.iter().filter_map(PySelector::text).collect()
    }

    /// First item's attributes.
    #[getter]
    fn attrib(&self) -> HashMap<String, String> {
        self.items
            .first()
            .map(PySelector::attrib)
            .unwrap_or_default()
    }

    /// Apply a regex to every item and flatten.
    #[pyo3(signature = (pattern, replace_entities=true))]
    fn re(&self, pattern: &str, replace_entities: bool) -> PyResult<Vec<String>> {
        let mut out = Vec::new();
        for item in &self.items {
            out.extend(item.re(pattern, replace_entities)?);
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
        for item in &self.items {
            let mut matches = item.re(pattern, replace_entities)?;
            if !matches.is_empty() {
                return Ok(Some(matches.remove(0)));
            }
        }
        Ok(default)
    }

    fn __bool__(&self) -> bool {
        !self.items.is_empty()
    }

    fn __repr__(&self) -> String {
        format!("<SelectorList len={}>", self.items.len())
    }
}

impl PySelectorList {
    fn item_at(&self, index: isize) -> PyResult<PySelector> {
        let len = self.items.len() as isize;
        let index = if index < 0 { len + index } else { index };
        if index < 0 || index >= len {
            return Err(PyIndexError::new_err("SelectorList index out of range"));
        }
        Ok(self.items[index as usize].clone())
    }

    fn from_core(doc: Rc<PyDoc>, result: CoreSelector<'_>, base_url: Option<String>) -> Self {
        let items = result
            .split()
            .into_iter()
            .map(|inner| PySelector::from_core(Rc::clone(&doc), inner, base_url.clone()))
            .collect();
        Self { items }
    }
}

fn attributes(selector: &CoreSelector<'_>) -> HashMap<String, String> {
    selector.attrib().unwrap_or_default().into_iter().collect()
}

fn extract_regex(text: &str, pattern: &str) -> PyResult<Vec<String>> {
    let regex = Regex::new(pattern)
        .map_err(|err| PyValueError::new_err(format!("invalid regex: {err}")))?;
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
