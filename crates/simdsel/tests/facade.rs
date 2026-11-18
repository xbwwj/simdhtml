use simdsel::{FlatDoc, ParseMode, Selector};

const HTML: &str = r#"<html><head><title>T</title></head><body>
<div id="list">
  <div class="item" data-id="1"><h2 class="title">First &amp; best</h2><a class="link" href="/one">one</a><span class="price">$10</span></div>
  <div class="item" data-id="2"><h2 class="title">Second</h2><a class="link" href="/two">two</a><span class="price">$20</span></div>
</div>
</body></html>"#;

fn parse_doc(input: &str) -> FlatDoc<'_> {
    simdsel::parse(input.as_bytes(), ParseMode::Fast).expect("valid HTML")
}

#[test]
fn css_end_to_end() {
    let doc = parse_doc(HTML);
    let root = Selector::new(&doc);
    assert_eq!(root.css(".item").unwrap().len(), 2);
    assert_eq!(
        root.css(".item .title::text").unwrap().getall(),
        vec!["First & best", "Second"]
    );
    assert_eq!(
        root.css(".item a.link::attr(href)").unwrap().getall(),
        vec!["/one", "/two"]
    );
    assert_eq!(
        root.css("[data-id='2'] .price::text").unwrap().get(),
        Some("$20".into())
    );
    assert_eq!(
        root.css(".item:nth-child(2) .title::text").unwrap().get(),
        Some("Second".into())
    );
}

#[test]
fn chained_css_searches_descendants() {
    let doc = parse_doc(HTML);
    let root = Selector::new(&doc);
    let items = root.css(".item").unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(
        items.css(".title::text").unwrap().getall(),
        vec!["First & best", "Second"]
    );
    assert_eq!(
        items.css("a::attr(href)").unwrap().getall(),
        vec!["/one", "/two"]
    );
}

#[test]
fn text_and_attrib_helpers() {
    let doc = parse_doc(HTML);
    let root = Selector::new(&doc);
    let item = root.css(".item").unwrap();
    assert_eq!(item.text(), Some("First & bestone$10".into()));
    let attrs = item.attrib().unwrap();
    assert!(attrs.iter().any(|(k, v)| k == "data-id" && v == "1"));
    assert_eq!(root.css(".missing").unwrap().get(), None);
}

#[test]
fn separate_documents_preserve_order() {
    let a = simdsel::parse(b"<p>a</p>", ParseMode::Fast).unwrap();
    let b = simdsel::parse(b"<p>b</p>", ParseMode::Fast).unwrap();
    let docs = [Selector::new(&a), Selector::new(&b)];
    assert_eq!(docs[0].css("p::text").unwrap().get(), Some("a".into()));
    assert_eq!(docs[1].css("p::text").unwrap().get(), Some("b".into()));
}

#[test]
fn deep_nesting_is_bounded() {
    let deep = "<div>".repeat(2000);
    let doc = parse_doc(&deep);
    let root = Selector::new(&doc);
    assert!(root.doc().flags.contains(simdsel::DocFlags::DEPTH_LIMIT));
}

#[test]
fn projection_gat_modes() {
    let doc = parse_doc(HTML);
    let root = Selector::new(&doc);
    assert_eq!(root.css_as::<simdsel::Count>(".item").unwrap(), 2);
    assert_eq!(
        root.css_as::<simdsel::Texts>(".item .title::text").unwrap(),
        vec!["First & best", "Second"]
    );
    assert_eq!(
        root.css_as::<simdsel::Html>(".item .title").unwrap()[0],
        "<h2 class=\"title\">First &amp; best</h2>"
    );
    assert_eq!(root.css_as::<simdsel::Records>(".item").unwrap().len(), 2);
    assert_eq!(root.css_count(".item").unwrap(), 2);
}
