//! CSS selector semantics through the public [`Selector`] API.
//!
//! These are fast guards for the generic cases. Parsel compatibility is covered
//! by the Python suite, and the engine-specific invariants (SIMD tag masks,
//! positional budget) stay in `simdsel::css`.

use simdhtml::parse::{ParseOptions, RepairMode};
use simdsel::Selector;

fn selector(html: &str) -> Selector<'_> {
    Selector::from_bytes_with(
        html.as_bytes(),
        ParseOptions {
            repair: RepairMode::Spec,
            ..Default::default()
        },
    )
    .unwrap()
}

fn css<'a>(html: &'a str, query: &str) -> Selector<'a> {
    selector(html).css(query).unwrap()
}

fn count(html: &str, query: &str) -> usize {
    css(html, query).len()
}

#[test]
fn simple_and_combinators() {
    let html = "<div><p class=a><span>x</span></p><p><b>y</b></p></div>";
    assert_eq!(count(html, "p"), 2);
    assert_eq!(count(html, "div > p"), 2);
    assert_eq!(count(html, "p .none"), 0);
    assert_eq!(count(html, "div span"), 1);
    assert_eq!(count(html, "p + p"), 1);
    assert_eq!(count(html, "p ~ p"), 1);
}

#[test]
fn attrs_and_classes() {
    let html = r#"<a id=x class="foo bar" href="/a" data-v="A B">1</a><a class=foo>2</a>"#;
    assert_eq!(count(html, "a#x"), 1);
    assert_eq!(count(html, ".foo"), 2);
    assert_eq!(count(html, ".foo.bar"), 1);
    assert_eq!(count(html, "[href]"), 1);
    assert_eq!(count(html, "[href^='/']"), 1);
    assert_eq!(count(html, "[data-v~='B']"), 1);
    assert_eq!(count(html, "[data-v='a b' i]"), 1);
}

#[test]
fn structural_pseudos() {
    let html = "<ul><li>1<li>2<li>3</ul>";
    assert_eq!(count(html, "li:first-child"), 1);
    assert_eq!(count(html, "li:last-child"), 1);
    assert_eq!(count(html, "li:nth-child(2)"), 1);
    assert_eq!(count(html, "li:nth-last-child(1)"), 1);
    assert_eq!(count(html, "li:not(:first-child)"), 2);
}

#[test]
fn has_relative() {
    let html = "<div><a class=x>1</a></div><div><p>2</p></div>";
    assert_eq!(count(html, "div:has(> a.x)"), 1);
}

#[test]
fn has_adjacent_sibling() {
    let html = "<ul><li>1</li> x <li>2</li><li>3</li></ul>";
    assert_eq!(count(html, "li:has(+ li)"), 2);
}

#[test]
fn scope_and_contains() {
    let html = "<div class=box><span class=title>hello scope</span></div><div class=other>x</div>";
    let boxes = selector(html).css("div").unwrap();
    let title = boxes.css(":scope > .title").unwrap();
    assert_eq!(title.len(), 1);
    assert_eq!(title.text().as_deref(), Some("hello scope"));
    assert_eq!(count(html, "div:contains('hello')"), 1);
}

#[test]
fn pseudo_elements() {
    let html = "<div><a href='/x'>hello<span>!</span></a></div>";
    assert_eq!(css(html, "a::text").get().as_deref(), Some("hello"));
    assert_eq!(css(html, "a::attr(href)").get().as_deref(), Some("/x"));
}

#[test]
fn query_side_child_and_type_indexes() {
    let html = "<div><p>a</p><span>x</span><p>b</p><p>c</p></div>";
    assert_eq!(count(html, "p:nth-of-type(2)"), 1);
    assert_eq!(count(html, "p:last-of-type"), 1);
    assert_eq!(count(html, "p:nth-last-of-type(2)"), 1);
    assert_eq!(count(html, "span:nth-child(2)"), 1);
    assert_eq!(count(html, "p:last-child"), 1);
    assert_eq!(count(html, "*:nth-of-type(2)"), 1);
}
