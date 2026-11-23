//! Smoke test for the PyO3 binding, run from Rust.
//!
//! This replaces the standalone Python test file: the binding is imported via
//! the same embedded interpreter the benchmarks use.  Skip when the `simdsel`
//! extension has not been installed (`uv run maturin develop --release`).

#![cfg(feature = "python")]

use std::ffi::CString;

use pyo3_embed::prelude::*;
use pyo3_embed::types::PyAny;

use simdhtml_bench::python::add_venv_sys_path;

/// `eval` for a `&str` expression (the literals below are long and full of
/// quotes, so `c_str!` would be unreadable).
fn eval<'py>(py: Python<'py>, code: &str) -> PyResult<Bound<'py, PyAny>> {
    let code = CString::new(code).expect("expression has no NUL");
    py.eval(code.as_c_str(), None, None)
}

/// The Python exception type name raised by `code`, or `"none"`.
fn raises(py: Python<'_>, code: &str) -> String {
    match eval(py, code) {
        Ok(_) => "none".to_string(),
        Err(error) => error.get_type(py).name().unwrap().to_string(),
    }
}

fn strings(py: Python<'_>, code: &str) -> Vec<String> {
    eval(py, code)
        .unwrap_or_else(|error| panic!("{code}: {error}"))
        .extract()
        .unwrap()
}

fn text(py: Python<'_>, code: &str) -> String {
    eval(py, code)
        .unwrap_or_else(|error| panic!("{code}: {error}"))
        .extract()
        .unwrap()
}

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
        let links = tree.call_method1("css", ("a::attr(href)",)).expect("css");
        let values: Vec<String> = links
            .call_method0("getall")
            .expect("getall")
            .extract()
            .unwrap();
        assert_eq!(values, vec!["/one", "/two"]);

        // Default mode is none: no implied html/head/body.
        let no_wrapper = selector.call1(("<p>x",)).expect("parse none");
        let html = no_wrapper.call_method1("css", ("html",)).expect("css");
        let values: Vec<String> = html
            .call_method0("getall")
            .expect("getall")
            .extract()
            .unwrap();
        assert!(values.is_empty());

        // Explicit spec mode restores the WHATWG wrappers.
        assert_eq!(
            text(
                py,
                "len(__import__('simdsel').Selector(text='<p>x', mode='spec').css('html').getall())"
            ),
            "1"
        );

        // `text=` is already decoded text, so a legacy `<meta charset>` inside
        // it is document data, not an encoding hint.
        assert_eq!(
            text(
                py,
                "__import__('simdsel').Selector(text='<meta charset=windows-1252><p>caf\\u00e9</p>').css('p::text').get()"
            ),
            "caf\u{e9}"
        );

        // `body=` is bytes off the wire, so the charset is sniffed (the
        // documented deviation from parsel's hard-coded utf-8).
        assert_eq!(
            text(
                py,
                "__import__('simdsel').Selector(body=b'<meta charset=windows-1252><p>caf\\xc3\\xa9</p>').css('p::text').get()"
            ),
            "caf\u{c3}\u{a9}"
        );
        assert_eq!(
            text(
                py,
                "__import__('simdsel').Selector(body=bytearray(b'<p>y</p>')).css('p::text').get()"
            ),
            "y"
        );

        // --- parsel-shaped API -------------------------------------------

        // A document-level selector represents the root element, so the
        // projection methods are not empty.
        assert_eq!(
            text(
                py,
                "__import__('simdsel').Selector(text='<html lang=en><body><p>x</p></body></html>').attrib['lang']"
            ),
            "en"
        );
        assert!(
            text(
                py,
                "__import__('simdsel').Selector(text='<html><body><p>x</p></body></html>').get()"
            )
            .starts_with("<html")
        );

        // XPath: node results, relative paths, scalars, attributes.
        assert_eq!(
            strings(
                py,
                "__import__('simdsel').Selector(text='<p>x</p><p>y</p>').xpath('//p/text()').getall()"
            ),
            vec!["x", "y"]
        );
        assert_eq!(
            text(
                py,
                "__import__('simdsel').Selector(text='<p>x</p><p>y</p>').xpath('count(//p)').get()"
            ),
            "2.0"
        );
        assert_eq!(
            text(
                py,
                "__import__('simdsel').Selector(text='<p>x</p>').xpath('boolean(//nope)').get()"
            ),
            "0"
        );
        assert_eq!(
            strings(
                py,
                "__import__('simdsel').Selector(text='<div><p>x</p></div>').css('div').xpath('./p/text()').getall()"
            ),
            vec!["x"]
        );
        assert_eq!(
            strings(
                py,
                "__import__('simdsel').Selector(text='<div class=k><p>x</p></div>').xpath('//div/@class').getall()"
            ),
            vec!["k"]
        );

        // `::text` results are a node set: document order, even when the
        // matched elements are nested.
        assert_eq!(
            strings(
                py,
                "__import__('simdsel').Selector(text='<div id=o><span>s1</span>x1<div id=i>y1</div>x2</div>').css('div::text').getall()"
            ),
            vec!["x1", "y1", "x2"]
        );

        // `A ::text` is every text node under A; standard pseudo-classes work;
        // `:attr`/`:text` items are strings, not elements.
        assert_eq!(
            strings(
                py,
                "__import__('simdsel').Selector(text='<div><span>s</span>t</div>').css('div ::text').getall()"
            ),
            vec!["s", "t"]
        );
        assert_eq!(
            strings(
                py,
                "__import__('simdsel').Selector(text='<a href=/x>l</a><a>n</a>').css('a:link::attr(href)').getall()"
            ),
            vec!["/x"]
        );
        assert_eq!(
            strings(
                py,
                "__import__('simdsel').Selector(text='<ul><li>a</li><li>b</li></ul>').css('li:only-of-type').getall()"
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            text(
                py,
                "__import__('simdsel').Selector(text='<div><!--c--><p>x</p></div>').css('div/*c*/ > p::text').get()"
            ),
            "x"
        );

        // `re()` is CPython's `re`: flags on compiled patterns, Unicode
        // classes, `$`, and `findall`'s flattening.
        assert_eq!(
            strings(
                py,
                "__import__('simdsel').Selector(text='<p>x</p><p>y</p>').css('p::text').re(__import__('re').compile(r'[xy]'))"
            ),
            vec!["x", "y"]
        );
        assert_eq!(
            strings(
                py,
                "__import__('simdsel').Selector(text='<p>ab</p>').css('p::text').re(r'.*')"
            ),
            vec!["ab", ""]
        );
        assert_eq!(
            strings(
                py,
                "__import__('simdsel').Selector(text='<p>Ab</p>').css('p::text').re(__import__('re').compile('AB', __import__('re').I))"
            ),
            vec!["Ab"],
            "a compiled pattern keeps its flags"
        );
        assert_eq!(
            strings(
                py,
                "__import__('simdsel').Selector(text='<p>caf\u{e9}</p>').css('p::text').re(r'\\bcaf\u{e9}\\b')"
            ),
            vec!["caf\u{e9}"],
            "word boundaries are Unicode, as in CPython"
        );
        assert_eq!(
            strings(
                py,
                "__import__('simdsel').Selector(text='<p>a1 b2</p>').css('p::text').re(r'([a-z])(\\d)')"
            ),
            vec!["a", "1", "b", "2"],
            "multi-group findall results are flattened"
        );
        assert_eq!(
            strings(
                py,
                "__import__('simdsel').Selector(text='<p>a1 b2</p>').css('p::text').re(r'(?P<extract>[a-z])1')"
            ),
            vec!["a"],
            "a named `extract` group takes only the first match"
        );

        // SelectorList behaves like the list parsel returns.
        assert_eq!(
            text(
                py,
                "len(list(__import__('simdsel').Selector(text='<p>x</p><p>y</p>').css('p')))"
            ),
            "2"
        );
        assert_eq!(
            text(
                py,
                "len(__import__('simdsel').Selector(text='<p>x</p>').css('p') + [])"
            ),
            "1"
        );

        // parsel's validation and the deviations we document.
        for (code, expected) in [
            (
                "__import__('simdsel').Selector(text='x', type='foo')",
                "ValueError",
            ),
            (
                "__import__('simdsel').Selector(text='x', type='xml')",
                "NotImplementedError",
            ),
            ("__import__('simdsel').Selector()", "ValueError"),
            (
                "__import__('simdsel').Selector(text='<p>x</p>').css('p::text').attrib",
                "AttributeError",
            ),
            (
                "__import__('simdsel').Selector(text='<p>x</p>').drop()",
                "NotImplementedError",
            ),
            (
                "__import__('simdsel').Selector(text='<p>x</p>').xpath('//p', v=1)",
                "NotImplementedError",
            ),
        ] {
            assert_eq!(raises(py, code), expected, "{code}");
        }
    });
}
