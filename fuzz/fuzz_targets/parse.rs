#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(doc) = simdsel::parse(data, simdsel::ParseMode::Fast) {
        let selector = simdsel::Selector::new(&doc);
        let _ = selector.css("div .x::text");
        let _ = selector.css(":has(> a)");
    }
});
