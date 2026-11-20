//! Smoke test for the PyO3 binding, run from Rust.
//!
//! This replaces the standalone Python test file: the binding is imported via
//! the same embedded interpreter the benchmarks use.  Skip when the `simdsel`
//! extension has not been installed (`uv run maturin develop --release`).

#![cfg(feature = "python")]

use pyo3_embed::prelude::*;
use simdhtml_bench::python::add_venv_sys_path;

#[test]
fn binding_smoke() {
    Python::attach(|py| {
        add_venv_sys_path(py);
        let Ok(simdsel) = py.import("simdsel") else {
            eprintln!("skip python_binding: simdsel extension is not installed");
            return;
        };

        let selector = simdsel.getattr("Selector").expect("Selector");
        let tree = selector
            .call1((
                "<html><body><div class=item><a href=/one>one</a><a href=/two>two</a></div></body></html>",
            ))
            .expect("parse");
        let links = tree.css("a::attr(href)").expect("css");
        let values: Vec<String> = links.getall().expect("getall");
        assert_eq!(values, vec!["/one", "/two"]);

        // Default mode is none: no implied html/head/body.
        let no_wrapper = selector.call1(("<p>x",)).expect("parse none");
        let html = no_wrapper.css("html").expect("css");
        let values: Vec<String> = html.getall().expect("getall");
        assert!(values.is_empty());

        // Explicit spec mode restores the WHATWG wrappers.
        let spec = py
            .eval(
                pyo3_embed::ffi::c_str!("__import__('simdsel').Selector(text='<p>x', mode='spec')"),
                None,
                None,
            )
            .expect("spec selector");
        let html = spec.css("html").expect("css spec");
        let values: Vec<String> = html.getall().expect("getall");
        assert!(!values.is_empty());

        // `text=` is already decoded text, so a legacy `<meta charset>` inside
        // it is document data, not an encoding hint (this used to come back as
        // "caf\u{c3}\u{a9}").
        let text = py
            .eval(
                pyo3_embed::ffi::c_str!(
                    "__import__('simdsel').Selector(text='<meta charset=windows-1252><p>caf\\u00e9</p>').css('p::text').get()"
                ),
                None,
                None,
            )
            .expect("text selector");
        assert_eq!(text.extract::<String>().unwrap(), "caf\u{e9}");

        // `body=` is bytes off the wire, so the charset is still sniffed (the
        // documented deviation from parsel's hard-coded utf-8).
        let body = py
            .eval(
                pyo3_embed::ffi::c_str!(
                    "__import__('simdsel').Selector(body=b'<meta charset=windows-1252><p>caf\\xc3\\xa9</p>').css('p::text').get()"
                ),
                None,
                None,
            )
            .expect("body selector");
        assert_eq!(body.extract::<String>().unwrap(), "caf\u{c3}\u{a9}");
    });
}

trait BoundExt {
    fn css(&self, query: &str) -> PyResult<Bound<'_, pyo3_embed::types::PyAny>>;
    fn getall(&self) -> PyResult<Vec<String>>;
}

impl<'py> BoundExt for Bound<'py, pyo3_embed::types::PyAny> {
    fn css(&self, query: &str) -> PyResult<Bound<'py, pyo3_embed::types::PyAny>> {
        self.call_method1("css", (query,))
    }

    fn getall(&self) -> PyResult<Vec<String>> {
        self.call_method0("getall")?.extract()
    }
}
