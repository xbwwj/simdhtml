#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(selector) = simdsel::Selector::from_bytes(data) {
        let _ = selector.css("div .x::text");
        let _ = selector.css(":has(> a)");
    }
});
