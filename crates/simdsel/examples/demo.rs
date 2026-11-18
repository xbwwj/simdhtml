use simdsel::{ParseMode, Selector};

fn main() {
    let html = r#"<html><body>
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

    // 这里是类似 parsel 的 API 设计
    let doc = simdsel::parse(html.as_bytes(), ParseMode::Fast).expect("valid HTML");
    let root = Selector::new(&doc);
    let titles = root.css(".item .title::text").expect("valid selector");
    let links = root
        .css(".item a.link::attr(href)")
        .expect("valid selector");

    println!("titles: {:?}", titles.getall());
    println!("links: {:?}", links.getall());
    println!("first title: {:?}", titles.get());
    println!(
        "items: {}",
        root.css(".item").expect("valid selector").len()
    );
}
