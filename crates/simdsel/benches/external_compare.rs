//! Rust selector/streaming comparison bench.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p simdsel --bench external_compare --features bench-external
//! ```
//!
//! The important number is cold `parse + select`: every iteration re-parses
//! the document.  `parse-only` is reported separately.  Backends have different
//! semantics (streaming rewriter vs DOM), so the raw row labels matter.

use std::cell::Cell;
use std::hint::black_box;
use std::rc::Rc;
use std::time::Instant;

use lol_html::{HtmlRewriter, Settings, doctype, element};
use scraper::{Html, Selector as ScraperSelector};
use simdsel::Selector as SimdselSelector;

const MIB: usize = 1024 * 1024;

struct Corpus {
    name: &'static str,
    html: String,
    queries: &'static [&'static str],
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
        let label = if target >= MIB {
            "4MiB"
        } else if target >= 512 * 1024 {
            "512KiB"
        } else {
            "64KiB"
        };
        // `label` is static-ish but we need a &'static str for the row; use
        // the index-specific names below instead.
        let _ = label;

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
            queries: &["div.item", "div.item .title", "div.item > a.link"],
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
            queries: &["td.price", "tr:nth-child(2n+1)", "tr.row"],
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
            queries: &["li", "li a"],
        });
    }
    out
}

fn best_of(reps: usize, mut f: impl FnMut() -> u64) -> (u64, f64) {
    let mut best = f64::MAX;
    let mut out = 0;
    for _ in 0..reps {
        let start = Instant::now();
        out = f();
        let elapsed = start.elapsed().as_secs_f64();
        if elapsed < best {
            best = elapsed;
        }
    }
    (out, best)
}

fn mbs(bytes: usize, seconds: f64) -> f64 {
    (bytes as f64 / 1e6) / seconds
}

fn parse_simdsel(html: &str) -> simdsel::FlatDoc<'_> {
    simdsel::parse(html.as_bytes(), simdsel::ParseMode::Fast).expect("simdsel parse")
}

fn select_simdsel(html: &str, query: &str) -> u64 {
    let doc = parse_simdsel(html);
    let selector = SimdselSelector::new(&doc);
    selector.css_count(query).expect("simdsel selector") as u64
}

fn parse_scraper(html: &str) -> Html {
    Html::parse_document(html)
}

fn select_scraper(html: &str, query: &str) -> u64 {
    let doc = parse_scraper(html);
    let selector = ScraperSelector::parse(query).expect("scraper selector");
    doc.select(&selector).count() as u64
}

fn lol_scan(html: &str) -> u64 {
    let mut rewriter = HtmlRewriter::new(Settings::new(), |bytes: &[u8]| {
        black_box(bytes);
    });
    rewriter.write(html.as_bytes()).expect("lol write");
    rewriter.end().expect("lol end");
    0
}

fn lol_lex(html: &str) -> u64 {
    let settings = Settings::new().append_document_content_handler(doctype!(|_| Ok(())));
    let mut rewriter = HtmlRewriter::new(settings, |_: &[u8]| {});
    rewriter.write(html.as_bytes()).expect("lol write");
    rewriter.end().expect("lol end");
    0
}

fn lol_select(html: &str, query: &str) -> u64 {
    let count = Rc::new(Cell::new(0u64));
    let handler_count = Rc::clone(&count);
    let settings = Settings::new().append_element_content_handler(element!(query, move |_el| {
        handler_count.set(handler_count.get() + 1);
        Ok(())
    }));
    let mut rewriter = HtmlRewriter::new(settings, |_: &[u8]| {});
    rewriter.write(html.as_bytes()).expect("lol write");
    rewriter.end().expect("lol end");
    count.get()
}

fn simdsel_select_many(html: &str, queries: &[&str]) -> u64 {
    let doc = parse_simdsel(html);
    let selector = SimdselSelector::new(&doc);
    queries
        .iter()
        .map(|query| selector.css_count(query).expect("simdsel selector") as u64)
        .sum()
}

fn scraper_select_many(html: &str, queries: &[&str]) -> u64 {
    let doc = parse_scraper(html);
    queries
        .iter()
        .map(|query| {
            let selector = ScraperSelector::parse(query).expect("scraper selector");
            doc.select(&selector).count() as u64
        })
        .sum()
}

fn lol_select_many(html: &str, queries: &[&str]) -> u64 {
    let count = Rc::new(Cell::new(0u64));
    let mut settings = Settings::new();
    for query in queries {
        let handler_count = Rc::clone(&count);
        settings = settings.append_element_content_handler(element!(query, move |_el| {
            handler_count.set(handler_count.get() + 1);
            Ok(())
        }));
    }
    let mut rewriter = HtmlRewriter::new(settings, |_: &[u8]| {});
    rewriter.write(html.as_bytes()).expect("lol write");
    rewriter.end().expect("lol end");
    count.get()
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
        println!(
            "{:>12} | {:>20} | {:>9} | {:>8} | {:>7}",
            "backend", "query", "best ms", "MB/s", "matches"
        );
        println!("{}", "-".repeat(72));

        let (_, simdsel_parse) = best_of(reps, || {
            black_box(parse_simdsel(&corpus.html));
            0
        });
        let (_, scraper_parse) = best_of(reps, || {
            black_box(parse_scraper(&corpus.html));
            0
        });
        let (_, lol_parse) = best_of(reps, || lol_scan(&corpus.html));
        let (_, lol_lex_parse) = best_of(reps, || lol_lex(&corpus.html));

        for (name, dt) in [
            ("simdsel", simdsel_parse),
            ("scraper", scraper_parse),
            ("lol-html(scan)", lol_parse),
            ("lol-html(lex)", lol_lex_parse),
        ] {
            println!(
                "{name:>12} | {:>20} | {:.3} | {:8.1} | {:>7}",
                "parse-only",
                dt * 1e3,
                mbs(bytes, dt),
                "-"
            );
        }

        for query in corpus.queries {
            let (matches, dt) = best_of(reps, || select_simdsel(&corpus.html, query));
            println!(
                "{:>12} | {:>20} | {:.3} | {:8.1} | {matches:>7}",
                "simdsel",
                query,
                dt * 1e3,
                mbs(bytes, dt)
            );

            if let Ok(selector) = ScraperSelector::parse(query) {
                let (matches, dt) = best_of(reps, || select_scraper(&corpus.html, query));
                println!(
                    "{:>12} | {:>20} | {:.3} | {:8.1} | {matches:>7}",
                    "scraper",
                    query,
                    dt * 1e3,
                    mbs(bytes, dt)
                );
                let _ = selector;
            }

            let (matches, dt) = best_of(reps, || lol_select(&corpus.html, query));
            println!(
                "{:>12} | {:>20} | {:.3} | {:8.1} | {matches:>7}",
                "lol-html",
                query,
                dt * 1e3,
                mbs(bytes, dt)
            );
        }

        // The real Scrapy shape: parse once, run all selectors.
        let (matches, dt) = best_of(reps, || simdsel_select_many(&corpus.html, corpus.queries));
        println!(
            "{:>12} | {:>20} | {:.3} | {:8.1} | {matches:>7}",
            "simdsel",
            format!("parse+{}css", corpus.queries.len()),
            dt * 1e3,
            mbs(bytes, dt)
        );
        let (matches, dt) = best_of(reps, || scraper_select_many(&corpus.html, corpus.queries));
        println!(
            "{:>12} | {:>20} | {:.3} | {:8.1} | {matches:>7}",
            "scraper",
            format!("parse+{}css", corpus.queries.len()),
            dt * 1e3,
            mbs(bytes, dt)
        );
        let (matches, dt) = best_of(reps, || lol_select_many(&corpus.html, corpus.queries));
        println!(
            "{:>12} | {:>20} | {:.3} | {:8.1} | {matches:>7}",
            "lol-html",
            format!("{} handlers", corpus.queries.len()),
            dt * 1e3,
            mbs(bytes, dt)
        );
    }
}
