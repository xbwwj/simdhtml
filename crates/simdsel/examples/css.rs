use simdsel::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let html = r#"\
<html><body>
    <div class="item" data-id="1">
        <h2 class="title">First &amp; best</h2>
        <a class="link" href="/one">one</a>
        <span class="price">$10</span>
    </div>
    <div class="item" data-id="2">
        <h2 class="title">Second</h2>
        <a class="link" href="/two">two</a>
        <span class="price">$20</span>
    </div>
</body></html>"#;

    // The selector owns the document: no separate parse step, no lifetime to
    // thread through the queries.
    let root = Selector::from_text(html)?;
    let titles = root.css(".item .title::text")?;
    let links = root.css(".item a.link::attr(href)")?;

    println!("titles: {:?}", titles.getall());
    println!("links: {:?}", links.getall());
    println!("first title: {:?}", titles.get());
    println!("items: {}", root.css(".item")?.len());

    Ok(())
}
