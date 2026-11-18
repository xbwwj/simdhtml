//! Cold-path parse + first-select throughput probe.
//!
//! Every measured iteration re-parses the document and re-compiles/re-runs the
//! selector.  This measures cold parse+select, not a warm repeated query.
//!
//! ```text
//! cargo bench -p simdsel --bench cold_parse_select
//! ```

use std::hint::black_box;
use std::time::Instant;

use simdsel::{ParseMode, Selector};

const MIB: usize = 1024 * 1024;

fn make_document(target: usize) -> Vec<u8> {
    let item = br#"<div class="item" data-id="42"><h2 class="title">Item &amp; title</h2><a class="link" href="/item/42">link</a><span class="price">$42</span><p class="desc">Description <b>with</b> markup</p></div>"#;
    let mut out = Vec::with_capacity(target + item.len());
    out.extend_from_slice(b"<html><head></head><body><div id=\"list\">");
    while out.len() < target {
        out.extend_from_slice(item);
    }
    out.extend_from_slice(b"</div></body></html>");
    out
}

fn best_of(reps: usize, mut f: impl FnMut() -> u64) -> (u64, f64) {
    let mut best = f64::MAX;
    let mut out = 0;
    for _ in 0..reps {
        let t0 = Instant::now();
        out = f();
        let dt = t0.elapsed().as_secs_f64();
        if dt < best {
            best = dt;
        }
    }
    (out, best)
}

fn main() {
    for target in [64 * 1024, 512 * 1024, 4 * MIB] {
        let data = make_document(target);
        let bytes = data.len();
        let label = if bytes < MIB {
            format!("{:.0} KiB", bytes as f64 / 1024.0)
        } else {
            format!("{:.0} MiB", bytes as f64 / MIB as f64)
        };

        let (_, parse_dt) = best_of(5, || {
            let doc = simdsel::parse(black_box(&data), ParseMode::Fast).unwrap();
            black_box(doc.nodes.len() as u64)
        });

        let (matches, cold_dt) = best_of(5, || {
            let doc = simdsel::parse(black_box(&data), ParseMode::Fast).unwrap();
            let root = Selector::new(&doc);
            let out = root.css(".item .title::text").unwrap();
            black_box(out.len() as u64)
        });

        let parse_mbs = bytes as f64 / parse_dt / 1e6;
        let cold_mbs = bytes as f64 / cold_dt / 1e6;
        println!(
            "{label:>9}: parse {parse_mbs:8.2} MB/s | parse+select {cold_mbs:8.2} MB/s | matches={matches}"
        );
    }
}
