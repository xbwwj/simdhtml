//! XPath 1.0 core behaviour, with expectations taken from lxml (`html.xpath`)
//! on the same document.

use simdsel::prelude::*;
use simdsel::xpath::value::RefKind;

const PAGE: &str = r#"\
<html>
<head>
  <title>T</title>
</head>
<body>
  <div id=a class="x y"><p>one</p><p class=y>two</p><a href="/l1">l1</a></div>
  <div id=b><span>s1</span><span>s2</span><p>three</p><a href="http://e/x">l2</a></div>
  <table><tr><td>1</td><td>2</td></tr></table>
</body>
</html>"#;

fn doc(html: &str) -> Document<'_> {
    Document::parse(html.as_bytes()).expect("parse failed")
}

fn values(doc: &Document<'_>, expr: &str) -> Vec<String> {
    let path = XPath::new(expr).unwrap_or_else(|error| panic!("{expr}: {error}"));
    match path
        .eval(doc)
        .unwrap_or_else(|error| panic!("{expr}: {error}"))
    {
        Value::Nodes(nodes) => nodes
            .iter()
            .map(|node| match node.kind() {
                RefKind::Element => node.name(doc).to_owned(),
                _ => node.string_value(doc).into_owned(),
            })
            .collect(),
        Value::Number(number) => vec![format!("{number}")],
        Value::Str(text) => vec![text],
        Value::Bool(value) => vec![value.to_string()],
    }
}

#[test]
fn paths_and_predicates() {
    let document = doc(PAGE);
    let cases: &[(&str, &[&str])] = &[
        ("//p", &["p", "p", "p"]),
        ("//p[2]", &["p"]),
        ("//div/p[1]", &["p", "p"]),
        ("//p/text()", &["one", "two", "three"]),
        ("//a/@href", &["/l1", "http://e/x"]),
        ("//div[@id='b']/span", &["span", "span"]),
        ("//div[contains(@class,'y')]", &["div"]),
        ("//p[last()]", &["p", "p"]),
        ("//*[@id]", &["div", "div"]),
        ("//div//a", &["a", "a"]),
        ("/html/body/div[1]", &["div"]),
        ("//span[position()>1]", &["span"]),
        ("//div | //span", &["div", "div", "span", "span"]),
        ("//p[.='two']", &["p"]),
        ("//div/following-sibling::div", &["div"]),
        ("//span[2]/preceding-sibling::span", &["span"]),
        ("count(//p)", &["3"]),
        ("//td[2]", &["td"]),
        ("//p[@class]", &["p"]),
        ("//div[@id='a']/p/text()", &["one", "two"]),
        ("//div[@id='a']/p[2]/text()", &["two"]),
        ("//div[@id='a']/*[1]", &["p"]),
        ("//div[@id='b']/span[2]/preceding-sibling::*[1]", &["span"]),
        ("//body/*[2]", &["div"]),
        ("//*[local-name()='td'][1]", &["td"]),
        ("(//div)[2]/@id", &["b"]),
        ("//div[@id='b']//text()", &["s1", "s2", "three", "l2"]),
        ("//a[starts-with(@href,'http')]", &["a"]),
        ("//p[contains(.,'tw')]", &["p"]),
        ("//div[not(@id='a')]", &["div"]),
        ("//*[@id='a']/following-sibling::*[1]/span[1]", &["span"]),
        ("//span[position()=last()]", &["span"]),
    ];

    for (expr, expected) in cases {
        assert_eq!(values(&document, expr), *expected, "expression {expr}");
    }
}

#[test]
fn attribute_nodes_expose_index_and_value() {
    let document = doc(PAGE);
    let Value::Nodes(nodes) = XPath::new("//div/@id").unwrap().eval(&document).unwrap() else {
        panic!("expected nodes");
    };

    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].kind(), RefKind::Attribute);
    assert_eq!(nodes[0].string_value(&document), "a");
    assert_eq!(nodes[0].name(&document), "id");
    assert_eq!(nodes[0].node_id(), nodes[0].node_id()); // owning element id
    assert!(nodes[0].attr_index().is_some());
}

#[test]
fn case_insensitive_html_names() {
    let document = doc("<DIV CLASS=Top><P>x</P></DIV>");

    assert_eq!(values(&document, "//div"), &["DIV"]);
    assert_eq!(values(&document, "//DIV/P"), &["P"]);
    assert_eq!(values(&document, "//div[@CLASS='Top']/p/text()"), &["x"]);
}

#[test]
fn numbers_strings_booleans() {
    let document = doc(PAGE);

    assert_eq!(values(&document, "1 + 2 * 3"), &["7"]);
    assert_eq!(values(&document, "10 div 4"), &["2.5"]);
    assert_eq!(values(&document, "10 mod 4"), &["2"]);
    assert_eq!(values(&document, "'abc'"), &["abc"]);
    assert_eq!(values(&document, "concat('a','b','c')"), &["abc"]);
    assert_eq!(values(&document, "normalize-space('  a   b  ')"), &["a b"]);
    assert_eq!(values(&document, "string(//title)"), &["T"]);
    assert_eq!(values(&document, "boolean(//p)"), &["true"]);
    assert_eq!(values(&document, "boolean(//nope)"), &["false"]);
    assert_eq!(values(&document, "not(//nope)"), &["true"]);
}

#[test]
fn node_set_equality_existential() {
    let document = doc("<r><a>1</a><a>2</a></r>");

    // `=` on node-sets is true if any pair matches; `!=` is not its negation.
    assert_eq!(values(&document, "//a = '1'"), &["true"]);
    assert_eq!(values(&document, "//a != '1'"), &["true"]);
    assert_eq!(values(&document, "//a = '3'"), &["false"]);
    assert_eq!(values(&document, "//a = 2"), &["true"]);
    assert_eq!(values(&document, "//a < 2"), &["true"]);
}

#[test]
fn document_order_and_dedup() {
    // Custom element names are not formatting elements, so the active
    // formatting element list does not reconstruct them; the shape is exactly
    // two subtrees with one leaf each.
    let document = doc("<r><s1><x-b></x-b></s1><s1><x-b></x-b></s1></r>");

    // Two leaves, each reached once; union across steps must not duplicate.
    assert_eq!(values(&document, "//s1//x-b"), &["x-b", "x-b"]);
    assert_eq!(values(&document, "//x-b | //x-b"), &["x-b", "x-b"]);
    assert_eq!(
        values(&document, "/descendant::x-b/ancestor::s1"),
        &["s1", "s1"]
    );
}

#[test]
fn unsupported_syntax_is_rejected() {
    for expr in [
        "//a/following::b",
        "//a/preceding::b",
        "//comment()",
        "//*[$var]",
        "count(",
        "//",
        "",
    ] {
        assert!(XPath::new(expr).is_err(), "expected {expr} to be rejected");
    }
}

#[test]
fn function_library() {
    let document = doc(PAGE);
    let cases: &[(&str, &[&str])] = &[
        // string functions
        ("substring-after(//a[1]/@href, '/')", &["l1"]),
        ("substring-before(//div[2]/a/@href, '/x')", &["http://e"]),
        ("substring(//p[1], 1, 2)", &["on"]),
        ("substring(//p[1], 2)", &["ne"]),
        ("substring('12345', 1.5, 2.6)", &["234"]),
        ("substring('12345', 0, 3)", &["12"]),
        ("substring('12345', -1)", &["12345"]),
        ("substring('12345', 1, 1 div 0)", &["12345"]),
        ("substring('12345', 1, -1)", &[""]),
        ("substring('12345', 0 div 0, 3)", &[""]),
        ("substring('12345', 1, 0 div 0)", &[""]),
        ("string-length(//p[1])", &["3"]),
        ("string-length('café')", &["4"]),
        ("translate('bar', 'abc', 'ABC')", &["BAr"]),
        ("translate('--aaa--', 'abc-', 'ABC')", &["AAA"]),
        ("normalize-space('  a   b  ')", &["a b"]),
        // number functions
        ("sum(//td)", &["3"]),
        ("sum(//nosuch)", &["0"]),
        ("floor(1.9)", &["1"]),
        ("floor(-1.1)", &["-2"]),
        ("ceiling(1.1)", &["2"]),
        ("ceiling(-1.1)", &["-1"]),
        ("round(2.5)", &["3"]),
        ("round(-2.5)", &["-2"]),
        ("round(2.4)", &["2"]),
        // node-set functions
        ("name(id('b'))", &["div"]),
        ("count(id('b'))", &["1"]),
        ("count(id('b nosuch'))", &["1"]),
        ("namespace-uri(//p[1])", &[""]),
        ("local-name(//p[1])", &["p"]),
    ];
    for (expr, expected) in cases {
        assert_eq!(values(&document, expr), *expected, "expression {expr}");
    }
}

#[test]
fn language_function() {
    let document =
        doc(r#"<div id=a><span xml:lang="en-GB">s</span><p xml:lang="de">p</p><i>i</i></div>"#);
    let cases: &[(&str, &[&str])] = &[
        ("//*[lang('en')]", &["span"]),
        ("//*[lang('en-gb')]", &["span"]),
        ("//*[lang('de')]", &["p"]),
        ("//*[lang('fr')]", &[]),
        ("lang('en')", &["false"]),
        ("count(//i[lang('en')])", &["0"]),
    ];
    for (expr, expected) in cases {
        assert_eq!(values(&document, expr), *expected, "expression {expr}");
    }
}
