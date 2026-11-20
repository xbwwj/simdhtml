#![no_main]

use libfuzzer_sys::fuzz_target;

const HTML: &[u8] = br#"<html><body><div class="item"><h2 class="title">T &amp; X</h2><a href="/x">link</a></div></body></html>"#;

fuzz_target!(|data: &[u8]| {
    let Ok(query) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(selector) = simdsel::Selector::from_bytes(HTML) else {
        return;
    };
    let _ = selector.css(query);
});
