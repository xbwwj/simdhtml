//! Parse-only comparison: simdhtml vs html5ever + rcdom.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p simdhtml --bench html5ever_compare --features bench-external
//! ```

use std::hint::black_box;
use std::time::Instant;

use html5ever_039::driver::parse_document;
use html5ever_039::tendril::TendrilSink;
use markup5ever_rcdom::RcDom;
use simdhtml::{ParseMode, parse};

const MIB: usize = 1024 * 1024;

struct Corpus {
    name: &'static str,
    html: String,
}

fn repeat_to(target: usize, head: &str, item: &str, tail: &str) -> String {
    let mut out = String::with_capacity(target + head.len() + tail.len());
    out.push_str(head);
    let mut n = 0usize;
    while out.len() < target {
        out.push_str(&item.replace("{n}", &n.to_string()));
        n += 1;
    }
    out.push_str(tail);
    out
}

fn corpora() -> Vec<Corpus> {
    let mut out = Vec::new();
    for target in [64 * 1024, 512 * 1024, 4 * MIB] {
        out.push(Corpus {
            name: match target {
                65536 => "cards/64KiB",
                524288 => "cards/512KiB",
                _ => "cards/4MiB",
            },
            html: repeat_to(
                target,
                r#"<html><head></head><body><div id="list">"#,
                r#"<div class="item" data-id="{n}"><h2 class="title">Item &amp; {n}</h2><a class="link" href="/item/{n}">link</a><span class="price">${n}</span><p class="desc">Description <b>with</b> markup</p></div>"#,
                "</div></body></html>",
            ),
        });
        out.push(Corpus {
            name: match target {
                65536 => "table/64KiB",
                524288 => "table/512KiB",
                _ => "table/4MiB",
            },
            html: repeat_to(
                target,
                "<html><body><table><tbody>",
                r#"<tr class="row"><td class="name">{n}</td><td class="price">${n}</td></tr>"#,
                "</tbody></table></body></html>",
            ),
        });
        out.push(Corpus {
            name: match target {
                65536 => "list/64KiB",
                524288 => "list/512KiB",
                _ => "list/4MiB",
            },
            html: repeat_to(
                target,
                "<html><body><ul>",
                r#"<li class="item"><a href="/{n}">{n}</a></li>"#,
                "</ul></body></html>",
            ),
        });
    }
    out
}

fn best_of(reps: usize, mut f: impl FnMut() -> u64) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..reps {
        let start = Instant::now();
        black_box(f());
        let elapsed = start.elapsed().as_secs_f64();
        if elapsed < best {
            best = elapsed;
        }
    }
    best
}

fn mbs(bytes: usize, seconds: f64) -> f64 {
    (bytes as f64 / 1e6) / seconds
}

fn parse_html5ever(html: &str) -> RcDom {
    parse_document(RcDom::default(), Default::default()).one(html.to_string())
}

fn main() {
    let reps = std::env::var("REPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3usize);
    println!("reps={reps} (best-of)");

    for corpus in corpora() {
        let bytes = corpus.html.len();
        println!("\n== {} ({:.0} KiB) ==", corpus.name, bytes as f64 / 1024.0);
        println!("{:>18} | {:>9} | {:>8}", "backend", "best ms", "MB/s");
        println!("{}", "-".repeat(44));

        let simdhtml_dt = best_of(reps, || {
            let doc = parse(corpus.html.as_bytes(), ParseMode::Fast).expect("simdhtml parse");
            doc.nodes.len() as u64
        });
        let html5ever_dt = best_of(reps, || {
            let dom = parse_html5ever(&corpus.html);
            black_box(&dom);
            0
        });

        for (name, dt) in [("simdhtml", simdhtml_dt), ("html5ever+rcdom", html5ever_dt)] {
            println!("{name:>18} | {:.3} | {:8.1}", dt * 1e3, mbs(bytes, dt));
        }
    }
}
