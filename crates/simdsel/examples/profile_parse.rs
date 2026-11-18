use std::time::Instant;

fn make(target: usize) -> Vec<u8> {
    let item=br#"<div class="item" data-id="42"><h2 class="title">Item &amp; title</h2><a class="link" href="/item/42">link</a><span class="price">$42</span><p class="desc">Description <b>with</b> markup</p></div>"#;
    let mut out = Vec::new();
    out.extend_from_slice(b"<html><head></head><body><div id=\"list\">");
    while out.len() < target {
        out.extend_from_slice(item);
    }
    out.extend_from_slice(b"</div></body></html>");
    out
}

fn best(reps: usize, mut f: impl FnMut() -> usize) -> (f64, usize) {
    let mut b = f64::MAX;
    let mut n = 0;
    for _ in 0..reps {
        let t = Instant::now();
        n = f();
        let d = t.elapsed().as_secs_f64();
        if d < b {
            b = d;
        }
    }
    (b, n)
}

fn main() {
    let data = make(4 * 1024 * 1024);
    let (s, tags) = best(5, || {
        simdhtml::build_markup_index(&data).unwrap().tag_count()
    });
    let (p, nodes) = best(5, || {
        simdhtml::parse(&data, simdhtml::ParseMode::Fast)
            .unwrap()
            .nodes
            .len()
    });
    println!("4MiB tags={tags} nodes={nodes}");
    println!(
        "stage1 {:.2} ms {:.1} MB/s",
        s * 1e3,
        data.len() as f64 / s / 1e6
    );
    println!(
        "full   {:.2} ms {:.1} MB/s",
        p * 1e3,
        data.len() as f64 / p / 1e6
    );
    println!("rest   {:.2} ms", (p - s) * 1e3);
}
