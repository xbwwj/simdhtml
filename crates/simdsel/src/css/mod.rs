//! CSS selector AST, parsing, and set-at-a-time matching.

pub mod ast;
pub mod cache;
pub mod eval;
pub mod parser;
pub mod select;

#[cfg(test)]
mod tests {
    use super::parser::parse_selector;
    use super::select::{extract, select, select_within};
    use simdhtml::document::Document;
    use simdhtml::document::id::NodeId;
    use simdhtml::parse::{ParseOptions, RepairMode};

    fn doc(html: &str) -> Document<'_> {
        Document::parse_with(
            html.as_bytes(),
            ParseOptions {
                repair: RepairMode::Spec,
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn tags(doc: &Document<'_>, sel: &str) -> Vec<String> {
        let selector = parse_selector(sel).unwrap();
        select(doc, &selector)
            .unwrap()
            .into_iter()
            .map(|r| doc.tag_name(doc.node(r.node).tag_token()).to_string())
            .collect()
    }

    #[test]
    fn simple_and_combinators() {
        let d = doc("<div><p class=a><span>x</span></p><p><b>y</b></p></div>");
        assert_eq!(tags(&d, "p"), vec!["p", "p"]);
        assert_eq!(tags(&d, "div > p"), vec!["p", "p"]);
        assert_eq!(tags(&d, "p .none"), Vec::<String>::new());
        assert_eq!(tags(&d, "div span"), vec!["span"]);
        assert_eq!(tags(&d, "p + p"), vec!["p"]);
        assert_eq!(tags(&d, "p ~ p"), vec!["p"]);
    }

    #[test]
    fn attrs_and_classes() {
        let d = doc(r#"<a id=x class="foo bar" href="/a" data-v="A B">1</a><a class=foo>2</a>"#);
        assert_eq!(tags(&d, "a#x"), vec!["a"]);
        assert_eq!(tags(&d, ".foo"), vec!["a", "a"]);
        assert_eq!(tags(&d, ".foo.bar"), vec!["a"]);
        assert_eq!(tags(&d, "[href]"), vec!["a"]);
        assert_eq!(tags(&d, "[href^='/']"), vec!["a"]);
        assert_eq!(tags(&d, "[data-v~='B']"), vec!["a"]);
        assert_eq!(tags(&d, "[data-v='a b' i]"), vec!["a"]);
    }

    #[test]
    fn structural_pseudos() {
        let d = doc("<ul><li>1<li>2<li>3</ul>");
        assert_eq!(tags(&d, "li:first-child"), vec!["li"]);
        assert_eq!(tags(&d, "li:last-child"), vec!["li"]);
        assert_eq!(tags(&d, "li:nth-child(2)"), vec!["li"]);
        assert_eq!(tags(&d, "li:nth-last-child(1)"), vec!["li"]);
        assert_eq!(tags(&d, "li:not(:first-child)"), vec!["li", "li"]);
    }

    #[test]
    fn has_relative() {
        let d = doc("<div><a class=x>1</a></div><div><p>2</p></div>");
        let sel = parse_selector("div:has(> a.x)").unwrap();
        let recs = select(&d, &sel).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(doc_tag(&d, recs[0].node), "div");
    }

    #[test]
    fn has_adjacent_sibling() {
        let d = doc("<ul><li>1</li> x <li>2</li><li>3</li></ul>");
        assert_eq!(tags(&d, "li:has(+ li)"), vec!["li", "li"]);
    }

    #[test]
    fn css_scope_and_contains() {
        let d = doc(
            "<div class=box><span class=title>hello scope</span></div><div class=other>x</div>",
        );
        let sel = parse_selector(":scope > .title").unwrap();
        // The parser synthesises html/head/body, so the scope root is looked up
        // by name.
        let root = d
            .nodes
            .iter()
            .position(|node| {
                node.is_element() && d.tag_name(node.tag_token()).eq_ignore_ascii_case("div")
            })
            .map(NodeId::from)
            .expect("div");
        let recs = select_within(&d, &sel, &[root]).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(doc_tag(&d, recs[0].node), "span");
        assert_eq!(tags(&d, "div:contains('hello')"), vec!["div"]);
    }

    #[test]
    fn pseudo_elements() {
        let d = doc("<div><a href='/x'>hello<span>!</span></a></div>");
        let sel = parse_selector("a::text").unwrap();
        let recs = select(&d, &sel).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(&*extract(&d, &recs[0]), "hello");
        let sel = parse_selector("a::attr(href)").unwrap();
        let recs = select(&d, &sel).unwrap();
        assert_eq!(&*extract(&d, &recs[0]), "/x");
    }

    #[test]
    fn simd_tag_mask_matches_scalar_on_many_tags() {
        let mut html = String::from("<html><body>");
        for i in 0..3000 {
            match i % 5 {
                0 => html.push_str("<div class=x>"),
                1 => html.push_str("<p>"),
                2 => html.push_str("<span>"),
                3 => html.push_str("<DIV>"),
                _ => html.push_str("<article>"),
            }
            html.push('x');
            html.push_str("</");
            match i % 5 {
                0 => html.push_str("div>"),
                1 => html.push_str("p>"),
                2 => html.push_str("span>"),
                3 => html.push_str("div>"),
                _ => html.push_str("article>"),
            }
        }
        html.push_str("</body></html>");
        let d = doc(&html);
        // `html` and `body` are explicit in this document; `head` is implied.
        assert_eq!(d.tags.len(), 6004 + 1);
        assert_eq!(tags(&d, "div").len(), 1200);
        assert_eq!(tags(&d, "p").len(), 600);
        assert_eq!(tags(&d, "span").len(), 600);
        assert_eq!(tags(&d, "article").len(), 600);
    }

    /// A single huge list makes an unbudgeted per-candidate sibling walk
    /// quadratic; the budget must fall back to the child index and stay correct.
    #[test]
    fn positional_pseudos_on_a_huge_list() {
        let mut html = String::from("<ul>");
        for i in 0..600 {
            html.push_str(&format!("<li>{i}</li>"));
        }
        html.push_str("</ul>");
        let d = doc(&html);
        assert_eq!(tags(&d, "li:nth-child(1)").len(), 1);
        assert_eq!(tags(&d, "li:nth-child(2)").len(), 1);
        assert_eq!(tags(&d, "li:nth-child(2n+1)").len(), 300);
        assert_eq!(tags(&d, "li:last-child").len(), 1);
        assert_eq!(tags(&d, "li:nth-of-type(3)").len(), 1);
        assert_eq!(tags(&d, "li:nth-last-of-type(1)").len(), 1);
        assert_eq!(tags(&d, "ul > li:nth-child(2n)").len(), 300);
    }

    #[test]
    fn query_side_child_and_type_indexes() {
        let d = doc("<div><p>a</p><span>x</span><p>b</p><p>c</p></div>");
        assert_eq!(tags(&d, "p:nth-of-type(2)"), vec!["p"]);
        assert_eq!(tags(&d, "p:last-of-type"), vec!["p"]);
        assert_eq!(tags(&d, "p:nth-last-of-type(2)"), vec!["p"]);
        assert_eq!(tags(&d, "span:nth-child(2)"), vec!["span"]);
        assert_eq!(tags(&d, "p:last-child"), vec!["p"]);
        assert_eq!(tags(&d, "*:nth-of-type(2)"), vec!["p"]);
    }

    fn doc_tag<'a>(doc: &'a Document<'a>, id: NodeId) -> &'a str {
        doc.tag_name(doc.node(id).tag_token())
    }
}
