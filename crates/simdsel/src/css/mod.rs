pub(crate) mod ast;
pub(crate) mod cache;
pub(crate) mod eval;
pub(crate) mod parser;
pub(crate) mod select;

#[cfg(test)]
mod tests {
    use simdhtml::document::Document;
    use simdhtml::parse::{ParseOptions, RepairMode};

    use super::parser::parse_selector;
    use super::select::select;

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
        // `html` and `body` are explicit (two tags each), `head` is implied.
        assert_eq!(d.tags.len(), 2 * 3000 + 4 + 1);
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
}
