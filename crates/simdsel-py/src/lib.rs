//! PyO3 binding for simdsel: a parsel-shaped `Selector`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;

use encoding_rs::Encoding;
use pyo3::exceptions::{
    PyAttributeError, PyIndexError, PyNotImplementedError, PyTypeError, PyValueError,
};
use pyo3::prelude::*;
use pyo3::types::{
    PyAny, PyAnyMethods, PyByteArray, PyBytes, PyDict, PyList, PySlice, PySliceMethods,
};
use simdhtml::{Document, EncodingMode, ParseOptions};
use simdsel::{
    MatchKind, MatchRecord, NodeId, NodeRef, QueryCache, RefKind, Value, XPath, extract,
    record_attrib, record_text, root_element, select_query_within_cache,
};

/// parsel's default XPath namespace map (EXSLT), reported by `.namespaces`.
const DEFAULT_NAMESPACES: [(&str, &str); 2] = [
    ("re", "http://exslt.org/regular-expressions"),
    ("set", "http://exslt.org/sets"),
];

/// One result item.
#[derive(Clone)]
enum Item {
    /// An element (from `css()` or an XPath node-set).
    Element(MatchRecord),
    /// A string: an XPath text node, attribute value, or scalar result.
    Text(String),
}

/// Parsel-shaped single selector.
#[pyclass(name = "Selector", module = "simdsel._simdsel", unsendable)]
#[derive(Clone)]
pub struct PySelector {
    doc: Rc<Document<'static>>,
    item: Item,
    /// Nodes a chained `css()` searches (the document node for a document
    /// selector, the element for an element selector, nothing for strings).
    scope: Vec<NodeId>,
    cache: Rc<QueryCache>,
    base_url: Option<String>,
    /// `_expr` from parsel, shown by `repr`.
    expr: Option<String>,
    namespaces: Rc<Vec<(String, String)>>,
    /// `"html"` or `"text"` (parsel's `Selector.type`).
    type_name: &'static str,
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

/// The record a document-level selector starts with: the root element, or the
/// document node when there is none (`get()` is then `""`).
fn document_record(doc: &Document<'_>) -> MatchRecord {
    match root_element(doc) {
        Some(id) => element_record(doc, id),
        None => MatchRecord {
            node: doc.root,
            kind: MatchKind::Element,
            span: 0..0,
            outer_end: 0,
            attr: None,
        },
    }
}

fn default_namespaces() -> Rc<Vec<(String, String)>> {
    Rc::new(
        DEFAULT_NAMESPACES
            .iter()
            .map(|(prefix, uri)| ((*prefix).to_string(), (*uri).to_string()))
            .collect(),
    )
}

impl PySelector {
    fn scalar(doc: Rc<Document<'static>>, value: impl Into<String>, expr: Option<String>) -> Self {
        Self {
            doc,
            item: Item::Text(value.into()),
            scope: Vec::new(),
            cache: Rc::new(QueryCache::root()),
            base_url: None,
            expr,
            namespaces: default_namespaces(),
            type_name: "text",
        }
    }

    fn from_item(
        doc: Rc<Document<'static>>,
        cache: Rc<QueryCache>,
        item: Item,
        scope: Vec<NodeId>,
        base_url: Option<String>,
        expr: Option<String>,
        namespaces: Rc<Vec<(String, String)>>,
    ) -> Self {
        Self {
            doc,
            item,
            scope,
            cache,
            base_url,
            expr,
            namespaces,
            type_name: "html",
        }
    }

    fn xpath_node_selector(&self, node: NodeRef) -> Option<Self> {
        let item = match node.kind() {
            RefKind::Element => Item::Element(element_record(&self.doc, node.node_id())),
            RefKind::Text | RefKind::Attribute => {
                Item::Text(node.string_value(&self.doc).into_owned())
            }
            // lxml has no document node, so parsel never returns one; the Rust
            // API keeps it (the XPath data model has it).
            RefKind::Document => return None,
        };
        let scope = match node.kind() {
            RefKind::Element => vec![node.node_id()],
            _ => Vec::new(),
        };
        Some(Self::from_item(
            Rc::clone(&self.doc),
            Rc::clone(&self.cache),
            item,
            scope,
            self.base_url.clone(),
            self.expr.clone(),
            Rc::clone(&self.namespaces),
        ))
    }

    /// Context node for `eval_at`; `None` for string selectors.
    fn context_node(&self) -> Option<NodeRef> {
        match &self.item {
            Item::Element(record) => Some(NodeRef::element(record.node)),
            Item::Text(_) => None,
        }
    }

    fn value(&self) -> Option<Cow<'_, str>> {
        match &self.item {
            Item::Element(record) => Some(extract(&self.doc, record)),
            Item::Text(value) => Some(Cow::Borrowed(value.as_str())),
        }
    }

    fn text_value(&self) -> Option<Cow<'_, str>> {
        match &self.item {
            Item::Element(record) => record_text(&self.doc, record),
            Item::Text(value) => Some(Cow::Borrowed(value.as_str())),
        }
    }

    fn attrib_map(&self) -> PyResult<HashMap<String, String>> {
        match &self.item {
            Item::Element(record) => Ok(record_attrib(&self.doc, record)
                .unwrap_or_default()
                .into_iter()
                .collect()),
            // parsel reaches through to the lxml node, so a string result is an
            // `AttributeError` rather than an empty dict.
            Item::Text(_) => Err(PyAttributeError::new_err(
                "'str' object has no attribute 'attrib'",
            )),
        }
    }

    fn repr_string(&self) -> String {
        let data = shorten(&self.value().unwrap_or_default(), 40);
        format!(
            "<Selector query={} data={}>",
            self.expr
                .as_deref()
                .map_or_else(|| "None".to_string(), py_repr),
            py_repr(&data)
        )
    }

    /// Builds the selector list for a chained query, per parsel: one item per
    /// matched node (the list level flattens across items).
    fn query_items(&self, query: &str) -> PyResult<Vec<PySelector>> {
        if self.scope.is_empty() {
            return Ok(Vec::new());
        }
        let records = select_query_within_cache(&self.doc, &self.scope, &self.cache, query)
            .map_err(to_py_err)?;
        Ok(records
            .into_iter()
            .map(|record| {
                // A `::text` / `::attr()` record is a string result in parsel
                // (a text selector), not an element: `.attrib` raises and
                // chained queries are empty.
                let (item, scope) = match record.kind {
                    MatchKind::Element => (Item::Element(record.clone()), vec![record.node]),
                    MatchKind::Text | MatchKind::Attr => (
                        Item::Text(extract(&self.doc, &record).into_owned()),
                        Vec::new(),
                    ),
                };
                Self::from_item(
                    Rc::clone(&self.doc),
                    Rc::clone(&self.cache),
                    item,
                    scope,
                    self.base_url.clone(),
                    self.expr.clone(),
                    Rc::clone(&self.namespaces),
                )
            })
            .collect())
    }

    /// Runs `query` from every context node and returns the flattened items.
    fn xpath_items(&self, query: &str) -> PyResult<Vec<PySelector>> {
        let Some(context) = self.context_node() else {
            // parsel re-parses a string selector's text as an HTML document;
            // for anything structural that is empty.
            return Ok(Vec::new());
        };
        let path = XPath::new(query)
            .map_err(|error| PyValueError::new_err(format!("XPath error: {error} in {query}")))?;
        let value = path
            .eval_at(&self.doc, context)
            .map_err(|error| PyValueError::new_err(format!("XPath error: {error} in {query}")))?;
        let mut items = Vec::new();
        match value {
            Value::Nodes(nodes) => {
                for node in nodes {
                    if let Some(selector) = self.xpath_node_selector(node) {
                        items.push(selector.with_expr(query));
                    }
                }
            }
            Value::Str(text) => items.push(self.scalar_like(text, query)),
            Value::Number(number) => items.push(self.scalar_like(format_number(number), query)),
            Value::Bool(value) => {
                items.push(self.scalar_like(if value { "1" } else { "0" }, query));
            }
        }
        Ok(items)
    }

    fn scalar_like(&self, value: impl Into<String>, query: &str) -> PySelector {
        Self {
            doc: Rc::clone(&self.doc),
            item: Item::Text(value.into()),
            scope: Vec::new(),
            cache: Rc::clone(&self.cache),
            base_url: self.base_url.clone(),
            expr: Some(query.to_string()),
            namespaces: Rc::clone(&self.namespaces),
            type_name: "html",
        }
    }

    fn with_expr(mut self, query: &str) -> Self {
        self.expr = Some(query.to_string());
        self
    }
}

impl PySelectorList {
    fn collect(items: Vec<PySelector>) -> Self {
        Self { items }
    }
}

#[pymethods]
impl PySelector {
    #[new]
    #[allow(clippy::too_many_arguments)] // mirrors parsel's constructor
    #[pyo3(signature = (text=None, r#type=None, body=None, encoding=None, namespaces=None, root=None, base_url=None, _expr=None, huge_tree=None, mode=None))]
    fn new(
        py: Python<'_>,
        text: Option<&str>,
        r#type: Option<&str>,
        body: Option<&Bound<'_, PyAny>>,
        encoding: Option<String>,
        namespaces: Option<&Bound<'_, PyAny>>,
        root: Option<&Bound<'_, PyAny>>,
        base_url: Option<String>,
        _expr: Option<String>,
        huge_tree: Option<bool>,
        mode: Option<&str>,
    ) -> PyResult<Self> {
        let _ = (py, huge_tree);
        if root.is_some_and(|root| !root.is_none()) {
            return Err(PyNotImplementedError::new_err(
                "simdsel is not lxml: root= (an lxml node) is not supported",
            ));
        }
        let type_name = match r#type {
            None | Some("html") => "html",
            Some("text") => "text",
            Some("xml") | Some("json") => {
                return Err(PyNotImplementedError::new_err(format!(
                    "simdsel only parses HTML: type={:?} is not supported",
                    r#type.unwrap_or_default()
                )));
            }
            Some(other) => return Err(PyValueError::new_err(format!("Invalid type: {other}"))),
        };

        let namespace_map = merge_namespaces(namespaces)?;

        // `type="text"` keeps the string as the selector's value, like parsel.
        if type_name == "text" {
            let text = text.filter(|text| !text.is_empty()).ok_or_else(|| {
                PyValueError::new_err("Selector needs text, body, or root arguments")
            })?;
            let doc = Document::parse_owned_with(Vec::new(), ParseOptions::default())
                .map_err(|error| PyValueError::new_err(error.to_string()))?;
            let mut selector = Self::scalar(Rc::new(doc), text, _expr);
            selector.base_url = base_url;
            selector.namespaces = namespace_map;
            selector.type_name = "text";
            return Ok(selector);
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
                    ParseOptions {
                        repair,
                        encoding: EncodingMode::Explicit(encoding),
                    },
                ),
                None => (
                    text.as_bytes().to_vec(),
                    ParseOptions {
                        repair,
                        encoding: EncodingMode::Utf8,
                    },
                ),
            },
            // `body=` is bytes off the wire, so the charset comes from `encoding=`
            // or, absent that, from the spec's sniffing algorithm (the
            // documented deviation from parsel's hard-coded utf-8).
            (None, Some(body)) => {
                let body = body_bytes(body)?;
                if body.is_empty() {
                    return Err(PyValueError::new_err(
                        "Selector needs text, body, or root arguments",
                    ));
                }
                (
                    body,
                    match encoding {
                        Some(encoding) => ParseOptions {
                            repair,
                            encoding: EncodingMode::Explicit(encoding),
                        },
                        None => ParseOptions {
                            repair,
                            ..Default::default()
                        },
                    },
                )
            }
            (None, None) => {
                return Err(PyValueError::new_err(
                    "Selector needs text, body, or root arguments",
                ));
            }
        };

        let doc = Document::parse_owned_with(bytes, options)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        let doc = Rc::new(doc);
        let scope = vec![doc.root];
        Ok(Self {
            item: Item::Element(document_record(&doc)),
            scope,
            doc,
            cache: Rc::new(QueryCache::root()),
            base_url,
            expr: _expr,
            namespaces: namespace_map,
            type_name: "html",
        })
    }

    /// Parses a list of byte buffers sequentially (simdsel extension).
    #[staticmethod]
    fn parse_many(bodies: Vec<Vec<u8>>) -> PyResult<Vec<PySelector>> {
        bodies
            .into_iter()
            .map(|body| {
                let doc = Document::parse_owned_with(body, ParseOptions::default())
                    .map_err(|error| PyValueError::new_err(error.to_string()))?;
                let doc = Rc::new(doc);
                Ok(Self {
                    item: Item::Element(document_record(&doc)),
                    scope: vec![doc.root],
                    doc,
                    cache: Rc::new(QueryCache::root()),
                    base_url: None,
                    expr: None,
                    namespaces: default_namespaces(),
                    type_name: "html",
                })
            })
            .collect()
    }

    /// Base URL (stored only; simdsel never fetches).
    #[getter]
    fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    /// parsel's `Selector.type` (`"html"` or `"text"`).
    #[getter]
    fn r#type(&self) -> &'static str {
        self.type_name
    }

    /// parsel returns an lxml node here; simdsel has no such object.
    #[getter]
    fn root(&self) -> PyResult<()> {
        Err(PyNotImplementedError::new_err(
            "simdsel is not lxml: there is no .root node",
        ))
    }

    /// Namespace prefixes visible to `xpath()` (defaults to parsel's EXSLT map).
    #[getter]
    fn namespaces(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let dict = PyDict::new(py);
        for (prefix, uri) in self.namespaces.iter() {
            dict.set_item(prefix, uri)?;
        }
        Ok(dict.into_any().unbind())
    }

    /// The class `css()`/`xpath()` return.
    #[getter]
    fn selectorlist_cls(&self, py: Python<'_>) -> Py<PyAny> {
        py.get_type::<PySelectorList>().into_any().unbind()
    }

    /// Compiles and evaluates a CSS selector.
    fn css(&self, query: &str) -> PyResult<PySelectorList> {
        Ok(PySelectorList::collect(self.query_items(query)?))
    }

    /// Evaluates an XPath 1.0 expression.
    ///
    /// Namespace prefixes are accepted but not resolved (the DOM stores local
    /// names only), and XPath variables are not supported.
    #[pyo3(signature = (query, namespaces=None, **kwargs))]
    fn xpath(
        &self,
        query: &str,
        namespaces: Option<&Bound<'_, PyAny>>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<PySelectorList> {
        if namespaces.is_some_and(|map| !map.is_none()) {
            // Prefixes cannot change the result: selectors match local names.
        }
        if let Some(kwargs) = kwargs
            && !kwargs.is_empty()
        {
            return Err(PyNotImplementedError::new_err(
                "XPath variables ($name) are not supported",
            ));
        }
        Ok(PySelectorList::collect(self.xpath_items(query)?))
    }

    /// Serialized current value.
    fn get(&self) -> Option<String> {
        self.value().map(Cow::into_owned)
    }

    /// Serialized values (a one-element list for a Selector).
    fn getall(&self) -> Vec<String> {
        self.value().map(Cow::into_owned).into_iter().collect()
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
        self.text_value().map(Cow::into_owned)
    }

    /// Attributes of the current element result.
    #[getter]
    fn attrib(&self) -> PyResult<HashMap<String, String>> {
        self.attrib_map()
    }

    /// Apply a Python `re` pattern to `get()`.
    ///
    /// The pattern language is CPython's, not a Rust approximation: this defers
    /// to the `re` module, exactly like parsel (`parsel.utils.extract_regex`).
    #[pyo3(signature = (pattern, replace_entities=true))]
    fn re(
        &self,
        py: Python<'_>,
        pattern: &Bound<'_, PyAny>,
        replace_entities: bool,
    ) -> PyResult<Vec<String>> {
        let Some(value) = self.value() else {
            return Ok(Vec::new());
        };
        extract_regex(py, pattern, &value, replace_entities)
    }

    /// First regex match or `default`.
    #[pyo3(signature = (pattern, default=None, replace_entities=true))]
    fn re_first(
        &self,
        py: Python<'_>,
        pattern: &Bound<'_, PyAny>,
        default: Option<String>,
        replace_entities: bool,
    ) -> PyResult<Option<String>> {
        let Some(value) = self.value() else {
            return Ok(default);
        };
        let mut matches = extract_regex(py, pattern, &value, replace_entities)?;
        Ok((!matches.is_empty())
            .then(|| matches.swap_remove(0))
            .or(default))
    }

    /// Nodes cannot be removed from an immutable document.
    fn drop(&mut self) -> PyResult<()> {
        Err(PyNotImplementedError::new_err(
            "simdsel documents are immutable: drop() is not supported",
        ))
    }

    fn __bool__(&self) -> bool {
        self.value().is_some_and(|value| !value.is_empty())
    }

    fn __str__(&self) -> String {
        self.get().unwrap_or_default()
    }

    fn __repr__(&self) -> String {
        self.repr_string()
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
            let len = self.items.len() as isize;
            let index = if index < 0 { len + index } else { index };
            if index < 0 || index >= len {
                return Err(PyIndexError::new_err("SelectorList index out of range"));
            }
            return Ok(Py::new(py, self.items[index as usize].clone())?.into_any());
        }
        if let Ok(slice) = index.cast::<PySlice>() {
            let indices = slice.indices(self.items.len() as isize)?;
            let mut items = Vec::with_capacity(indices.slicelength);
            for k in 0..indices.slicelength {
                let idx = indices.start + k as isize * indices.step;
                items.push(self.items[idx as usize].clone());
            }
            return Ok(Py::new(py, PySelectorList::collect(items))?.into_any());
        }
        Err(PyTypeError::new_err(
            "SelectorList index must be int or slice",
        ))
    }

    fn __iter__(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let list = PyList::new(
            py,
            self.items
                .iter()
                .map(|item| Py::new(py, item.clone()))
                .collect::<PyResult<Vec<_>>>()?,
        )?;
        Ok(list.as_any().try_iter()?.into_any().unbind())
    }

    /// `list + list` returns a plain list of selectors, as in parsel.
    fn __add__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let mut items: Vec<Py<PySelector>> = self
            .items
            .iter()
            .map(|item| Py::new(py, item.clone()))
            .collect::<PyResult<_>>()?;
        let other = other.try_iter()?;
        for item in other {
            let selector: Py<PySelector> = item?.extract()?;
            items.push(selector);
        }
        Ok(PyList::new(py, items)?.into_any().unbind())
    }

    fn append(&mut self, item: PyRef<'_, PySelector>) {
        self.items.push(item.clone());
    }

    fn extend(&mut self, other: &Bound<'_, PyAny>) -> PyResult<()> {
        for item in other.try_iter()? {
            self.items
                .push(item?.extract::<PyRef<'_, PySelector>>()?.clone());
        }
        Ok(())
    }

    fn insert(&mut self, index: isize, item: PyRef<'_, PySelector>) {
        let len = self.items.len() as isize;
        let index = index.clamp(0, len) as usize;
        self.items.insert(index, item.clone());
    }

    #[pyo3(signature = (index=-1))]
    fn pop(&mut self, index: isize) -> PyResult<PySelector> {
        let len = self.items.len() as isize;
        let index = if index < 0 { len + index } else { index };
        if index < 0 || index >= len {
            return Err(PyIndexError::new_err("pop index out of range"));
        }
        Ok(self.items.remove(index as usize))
    }

    fn clear(&mut self) {
        self.items.clear();
    }

    /// Applies CSS to every item and flattens the result.
    fn css(&self, query: &str) -> PyResult<PySelectorList> {
        let mut items = Vec::new();
        for item in &self.items {
            items.append(&mut item.query_items(query)?);
        }
        Ok(PySelectorList::collect(items))
    }

    /// Evaluates XPath per item and flattens the result.
    #[pyo3(signature = (query, namespaces=None, **kwargs))]
    fn xpath(
        &self,
        query: &str,
        namespaces: Option<&Bound<'_, PyAny>>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<PySelectorList> {
        let _ = namespaces;
        if let Some(kwargs) = kwargs
            && !kwargs.is_empty()
        {
            return Err(PyNotImplementedError::new_err(
                "XPath variables ($name) are not supported",
            ));
        }
        let mut items = Vec::new();
        for item in &self.items {
            items.append(&mut item.xpath_items(query)?);
        }
        Ok(PySelectorList::collect(items))
    }

    /// First item's value, or `default` when empty.
    #[pyo3(signature = (default=None))]
    fn get(&self, default: Option<String>) -> Option<String> {
        match self.items.first() {
            Some(item) => item.get(),
            None => default,
        }
    }

    /// All items' values.
    fn getall(&self) -> Vec<String> {
        self.items.iter().filter_map(|item| item.get()).collect()
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
        self.items.iter().filter_map(|item| item.text()).collect()
    }

    /// First item's attributes.
    #[getter]
    fn attrib(&self) -> PyResult<HashMap<String, String>> {
        match self.items.first() {
            Some(item) => item.attrib_map(),
            None => Ok(HashMap::new()),
        }
    }

    /// Apply a regex to every item and flatten.
    #[pyo3(signature = (pattern, replace_entities=true))]
    fn re(
        &self,
        py: Python<'_>,
        pattern: &Bound<'_, PyAny>,
        replace_entities: bool,
    ) -> PyResult<Vec<String>> {
        let mut out = Vec::new();
        for item in &self.items {
            if let Some(value) = item.value() {
                out.extend(extract_regex(py, pattern, &value, replace_entities)?);
            }
        }
        Ok(out)
    }

    /// First regex match across items, or `default`.
    #[pyo3(signature = (pattern, default=None, replace_entities=true))]
    fn re_first(
        &self,
        py: Python<'_>,
        pattern: &Bound<'_, PyAny>,
        default: Option<String>,
        replace_entities: bool,
    ) -> PyResult<Option<String>> {
        for item in &self.items {
            if let Some(value) = item.value() {
                let mut matches = extract_regex(py, pattern, &value, replace_entities)?;
                if !matches.is_empty() {
                    return Ok(Some(matches.swap_remove(0)));
                }
            }
        }
        Ok(default)
    }

    /// Nodes cannot be removed from an immutable document.
    fn drop(&mut self) -> PyResult<()> {
        Err(PyNotImplementedError::new_err(
            "simdsel documents are immutable: drop() is not supported",
        ))
    }

    fn __bool__(&self) -> bool {
        !self.items.is_empty()
    }

    fn __repr__(&self) -> String {
        let items: Vec<String> = self.items.iter().map(PySelector::repr_string).collect();
        format!("[{}]", items.join(", "))
    }
}

/// `repr(str)` with Python's quote choice: single quotes unless the text has
/// a single quote and no double quote.
fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch == quote => {
                out.push('\\');
                out.push(ch);
            }
            ch => out.push(ch),
        }
    }
    out.push(quote);
    out
}

/// parsel's `shorten(text, width)` for `repr`.
fn shorten(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

/// Python's `repr(float)` for the values XPath produces.
fn format_number(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    format!("{value:?}")
}

/// The `type=` / positional `body` conversions.
fn body_bytes(body: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    if let Ok(bytes) = body.cast::<PyBytes>() {
        return Ok(bytes.as_bytes().to_vec());
    }
    if let Ok(bytes) = body.cast::<PyByteArray>() {
        // SAFETY: the bytes are copied before the bytearray can be mutated.
        return Ok(unsafe { bytes.as_bytes() }.to_vec());
    }
    if let Ok(bytes) = body.extract::<Vec<u8>>() {
        // memoryview and other buffers.
        return Ok(bytes);
    }
    Err(PyTypeError::new_err(format!(
        "body argument should be of type bytes or bytearray, got {}",
        body.get_type()
            .name()
            .map(|name| name.to_string())
            .unwrap_or_default()
    )))
}

/// `namespaces=` (defaults + the caller's map).
fn merge_namespaces(map: Option<&Bound<'_, PyAny>>) -> PyResult<Rc<Vec<(String, String)>>> {
    let mut out: Vec<(String, String)> = DEFAULT_NAMESPACES
        .iter()
        .map(|(prefix, uri)| ((*prefix).to_string(), (*uri).to_string()))
        .collect();
    if let Some(map) = map
        && !map.is_none()
    {
        for (key, value) in map.cast::<PyDict>()?.iter() {
            out.push((key.extract()?, value.extract()?));
        }
    }
    Ok(Rc::new(out))
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
/// result) and `spec` is full WHATWG compatibility.
fn repair_mode(mode: Option<&str>) -> PyResult<simdhtml::RepairMode> {
    Ok(match mode.unwrap_or("none").to_ascii_lowercase().as_str() {
        "none" | "" => simdhtml::RepairMode::None,
        "spec" => simdhtml::RepairMode::Spec,
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown parse mode {other:?}: expected \"none\" or \"spec\""
            )));
        }
    })
}

/// parsel's `utils.extract_regex`, on CPython's `re`.
///
/// The policy is parsel's, verbatim:
///
/// * a `str` pattern is compiled with `re.UNICODE`; a compiled pattern keeps
///   its own flags (so `re.compile(p, re.I)` means what it means in Python);
/// * a named group `extract` wins and is taken from the **first** match
///   (`search`, not `finditer`);
/// * otherwise `findall` is used and its tuples are flattened;
/// * `replace_entities` decodes references in each extracted string except
///   `&amp;` / `&lt;` (w3lib's `keep` list), *after* matching.
fn extract_regex(
    py: Python<'_>,
    pattern: &Bound<'_, PyAny>,
    text: &str,
    replace_entities: bool,
) -> PyResult<Vec<String>> {
    let module = py.import("re")?;
    let regex = if pattern.extract::<String>().is_ok() {
        module.call_method1("compile", (pattern, module.getattr("UNICODE")?))?
    } else {
        pattern.clone()
    };

    let mut out = Vec::new();
    // `groupindex` is a mappingproxy, not a dict.
    let extract_named = regex.getattr("groupindex")?.contains("extract")?;
    if extract_named {
        let found = regex.call_method1("search", (text,))?;
        if !found.is_none() {
            let value = found.call_method1("group", ("extract",))?;
            if !value.is_none() {
                out.push(value.extract::<String>()?);
            }
        }
    } else {
        let matches = regex.call_method1("findall", (text,))?;
        flatten_strings(&matches, &mut out)?;
    }

    if replace_entities {
        for item in out.iter_mut() {
            let decoded = keep_lt_amp_entities(item);
            *item = decoded;
        }
    }
    Ok(out)
}

/// `parsel.utils.flatten` over `re.findall` output (strings and tuples of
/// strings; strings are leaves even though they are iterable).
fn flatten_strings(value: &Bound<'_, PyAny>, out: &mut Vec<String>) -> PyResult<()> {
    if let Ok(text) = value.extract::<String>() {
        out.push(text);
        return Ok(());
    }
    for item in value.try_iter()? {
        flatten_strings(&item?, out)?;
    }
    Ok(())
}

/// w3lib's `replace_entities(..., keep=["lt", "amp"])`: decode every character
/// reference except `&lt;` and `&amp;`, which stay literal.
fn keep_lt_amp_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    while let Some(offset) = text[pos..].find('&') {
        let start = pos + offset;
        out.push_str(&text[pos..start]);
        // Character references are short; `;` terminates this one.  A `&`
        // without a `;` nearby is literal text.
        let end = bytes[start + 1..]
            .iter()
            .position(|&byte| byte == b';')
            .filter(|&distance| distance <= 32)
            .map(|distance| start + 1 + distance);
        let Some(end) = end else {
            out.push('&');
            pos = start + 1;
            continue;
        };
        let name = &text[start + 1..end];
        if name.eq_ignore_ascii_case("amp") || name.eq_ignore_ascii_case("lt") {
            out.push_str(&text[start..=end]);
        } else {
            out.push_str(&simdhtml::decode_character_references(
                &text[start..=end],
                false,
            ));
        }
        pos = end + 1;
    }
    out.push_str(&text[pos..]);
    out
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
