use simdsel::Selector;

const HTML: &str = r#"\
<html>
<head>
  <title>T</title>
</head>
<body>
<div id="list">
  <div class="item" data-id="1"><h2 class="title">First &amp; best</h2><a class="link" href="/one">one</a><span class="price">$10</span></div>
  <div class="item" data-id="2"><h2 class="title">Second</h2><a class="link" href="/two">two</a><span class="price">$20</span></div>
</div>
</body></html>"#;

fn root(input: &str) -> Selector<'_> {
    Selector::from_text(input).expect("valid HTML")
}

#[test]
fn css_root() {
    let root = root(HTML);

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
fn css_chained() {
    let root = root(HTML);

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
fn missing_attrs() {
    let root = root(HTML);

    let item = root.css(".item").unwrap();
    assert_eq!(item.text(), Some("First & bestone$10".into()));

    let attrs = item.attrib().unwrap();
    assert!(attrs.iter().any(|(k, v)| k == "data-id" && v == "1"));
    assert_eq!(root.css(".missing").unwrap().get(), None);
}

#[test]
fn deep_limit() {
    let deep = "<div>".repeat(2000);
    let root = root(&deep);

    assert!(
        root.doc()
            .flags
            .contains(simdsel::DocumentFlags::DEPTH_LIMIT)
    );
}

#[test]
fn projection_gat_modes() {
    let root = root(HTML);

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

#[test]
fn selector_owns_its_document() {
    // Owned bytes: the selector stays valid after the buffer that produced it
    // is gone, which is what a crawl step needs.
    fn build(body: Vec<u8>) -> Selector<'static> {
        Selector::new(simdsel::Document::parse_owned(body).expect("parse"))
    }
    let owned = build(b"<div class=item><a href=/x>hi</a></div>".to_vec());
    assert_eq!(
        owned.css(".item a::attr(href)").unwrap().get(),
        Some("/x".into())
    );

    // `From<Document>` is the same as `Selector::new`.
    let html = String::from("<p class=x>hi</p>");
    let doc = simdsel::Document::parse(html.as_bytes()).expect("parse");
    let from: Selector<'_> = doc.into();
    assert_eq!(from.css("p::text").unwrap().get(), Some("hi".into()));

    // `from_bytes` sniffs the encoding, `from_text` treats the input as UTF-8.
    let bytes = b"<meta charset=windows-1252><p class=x>caf\xE9</p>";
    assert_eq!(
        Selector::from_bytes(bytes)
            .unwrap()
            .css("p::text")
            .unwrap()
            .get(),
        Some("caf\u{e9}".into())
    );
}

#[test]
fn split_shares_the_document() {
    let items = Selector::from_text(HTML)
        .unwrap()
        .css(".item")
        .unwrap()
        .split();

    assert_eq!(items.len(), 2);
    assert_eq!(
        items[1].css(".title::text").unwrap().get(),
        Some("Second".into())
    );
    assert_eq!(
        items[0].css("a::attr(href)").unwrap().get(),
        Some("/one".into())
    );
}

#[test]
fn owned_entries_are_lifetime_free() {
    // The selectors take the buffer over, so they can outlive the caller that
    // produced it (this is what a fetch-then-extract function returns).
    fn fetch_bytes(body: Vec<u8>) -> Selector<'static> {
        Selector::from_vec(body).expect("parse")
    }
    fn fetch_text(html: String) -> Selector<'static> {
        Selector::from_string(html).expect("parse")
    }

    // `from_vec` sniffs, so a declared legacy charset is honoured.
    let sniffed = fetch_bytes(b"<meta charset=windows-1252><p class=x>caf\xE9</p>".to_vec());
    assert_eq!(
        sniffed.css("p::text").unwrap().get(),
        Some("caf\u{e9}".into())
    );

    // `from_string` takes the text as UTF-8 as it is: the `<meta>` is document
    // data, not an encoding hint (sniffing valid UTF-8 as windows-1252 would
    // give "caf\u{c3}\u{a9}").
    let decoded = fetch_text("<meta charset=windows-1252><p class=x>café</p>".to_string());
    assert_eq!(decoded.css("p::text").unwrap().get(), Some("café".into()));

    // Options still apply to owned input.
    let spec = Selector::from_vec_with(
        b"<p>x".to_vec(),
        simdsel::ParseOptions::new().repair(simdsel::RepairMode::Spec),
    )
    .unwrap();
    assert_eq!(spec.css("html").unwrap().len(), 1);
}
