use std::borrow::Cow;

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};

/// How far into the document the `charset` prescan looks ("prescan a byte
/// stream to determine its encoding").
const PRESCAN_LEN: usize = 1024;

/// Result of [`decode`] or [`decode_with`].
#[derive(Debug)]
pub struct Decoded<'a> {
    /// UTF-8 bytes: the caller's buffer (no copy) or the transcoded text.
    pub bytes: Cow<'a, [u8]>,
    pub encoding: &'static Encoding,
    /// Whether the input held byte sequences invalid in `encoding`.
    pub errors: bool,
}

/// Decodes `input` to UTF-8, sniffing the charset and consuming a leading byte
/// order mark.
#[must_use]
pub fn decode(input: &[u8]) -> Decoded<'_> {
    if let Some((encoding, bom_len)) = Encoding::for_bom(input) {
        let body = &input[bom_len..];
        // A UTF-8 BOM is common: strip it and keep the zero-copy path.
        if encoding == UTF_8
            && let Some(decoded) = borrow_utf8(body)
        {
            return decoded;
        }
        let (bytes, actual, errors) = transcode(body, encoding);
        return Decoded {
            bytes,
            encoding: actual,
            errors,
        };
    }

    let declared = prescan_meta(input);
    if let Some(encoding) = declared
        && encoding != UTF_8
    {
        let (bytes, actual, errors) = transcode(input, encoding);
        return Decoded {
            bytes,
            encoding: actual,
            errors,
        };
    }

    // Validate once: valid UTF-8 stays zero-copy, invalid bytes are decoded the
    // way a browser would.
    if let Some(decoded) = borrow_utf8(input) {
        return decoded;
    }

    let encoding = declared.unwrap_or(WINDOWS_1252);
    let (bytes, actual, errors) = transcode(input, encoding);
    Decoded {
        bytes,
        encoding: actual,
        errors,
    }
}

/// Decodes `input` as `encoding`, skipping the prescan.  This is the transport
/// level of the algorithm: it outranks `<meta charset>`, but a byte order mark
/// still wins.
#[must_use]
pub fn decode_with<'a>(input: &'a [u8], encoding: &'static Encoding) -> Decoded<'a> {
    if Encoding::for_bom(input).is_some() {
        return decode(input);
    }

    if encoding == UTF_8
        && let Some(decoded) = borrow_utf8(input)
    {
        return decoded;
    }

    let (bytes, actual, errors) = transcode(input, encoding);
    Decoded {
        bytes,
        encoding: actual,
        errors,
    }
}

/// [`decode`] for an owned buffer: the bytes are moved, never copied, and a
/// stripped BOM prefix is drained in place.
#[must_use]
pub fn decode_owned(input: Vec<u8>) -> Decoded<'static> {
    take_ownership(input, decode)
}

/// [`decode_with`] for an owned buffer.
#[must_use]
pub fn decode_with_owned(input: Vec<u8>, encoding: &'static Encoding) -> Decoded<'static> {
    take_ownership(input, |bytes| decode_with(bytes, encoding))
}

/// Runs a borrowing decode, then moves the buffer into the result.  A borrowing
/// decode only ever returns a suffix (the input, or that minus a stripped BOM).
fn take_ownership<F>(input: Vec<u8>, decode: F) -> Decoded<'static>
where
    F: for<'a> FnOnce(&'a [u8]) -> Decoded<'a>,
{
    let (keep, encoding, errors) = {
        match decode(&input) {
            Decoded {
                bytes: Cow::Owned(bytes),
                encoding,
                errors,
            } => {
                return Decoded {
                    bytes: Cow::Owned(bytes),
                    encoding,
                    errors,
                };
            }
            Decoded {
                bytes: Cow::Borrowed(slice),
                encoding,
                errors,
            } => (slice.len(), encoding, errors),
        }
    };

    let mut bytes = input;
    let skip = bytes.len() - keep;
    bytes.drain(..skip);
    Decoded {
        bytes: Cow::Owned(bytes),
        encoding,
        errors,
    }
}

/// Looks up a charset label (`"gbk"`, `"windows-1252"`, ...).
#[must_use]
pub fn lookup(label: &str) -> Option<&'static Encoding> {
    Encoding::for_label(label.as_bytes())
}

/// The encoding a browser would try first, without validating the document.
#[must_use]
pub fn sniff(input: &[u8]) -> &'static Encoding {
    if let Some((encoding, _)) = Encoding::for_bom(input) {
        return encoding;
    }
    prescan_meta(input).unwrap_or(UTF_8)
}

fn borrow_utf8(input: &[u8]) -> Option<Decoded<'_>> {
    simdutf8::compat::from_utf8(input).ok().map(|_| Decoded {
        bytes: Cow::Borrowed(input),
        encoding: UTF_8,
        errors: false,
    })
}

fn transcode<'a>(
    input: &'a [u8],
    encoding: &'static Encoding,
) -> (Cow<'a, [u8]>, &'static Encoding, bool) {
    let (text, actual, errors) = encoding.decode(input);
    let bytes = match text {
        Cow::Borrowed(text) => Cow::Borrowed(text.as_bytes()),
        Cow::Owned(text) => Cow::Owned(text.into_bytes()),
    };
    (bytes, actual, errors)
}

/// The `<meta>` prescan, in the order the specification checks labels.
fn prescan_meta(input: &[u8]) -> Option<&'static Encoding> {
    let head = &input[..input.len().min(PRESCAN_LEN)];
    let mut from = 0;
    while let Some(open) = find_ignore_ascii_case(head, b"<meta", from) {
        let rest = &head[open..];
        let end = rest
            .iter()
            .position(|&b| b == b'>')
            .map_or(rest.len(), |end| end + 1);
        let tag = &rest[..end];
        from = open + 1;

        // `<meta charset=...>` and `<meta http-equiv=... content="...; charset=...">`
        // both carry the label after a `charset` token.
        let Some(at) = find_ignore_ascii_case(tag, b"charset", 0) else {
            continue;
        };
        let Some(label) = label_after(&tag[at + b"charset".len()..]) else {
            continue;
        };
        let Some(encoding) = Encoding::for_label(label) else {
            continue;
        };
        // A UTF-16 label in a `<meta>` means UTF-8: the bytes are already
        // ASCII-compatible.
        return Some(if encoding == UTF_16LE || encoding == UTF_16BE {
            UTF_8
        } else {
            encoding
        });
    }
    None
}

/// Parses `= label` after a `charset` token.
fn label_after(bytes: &[u8]) -> Option<&[u8]> {
    let mut rest = bytes;
    while let [b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'/', tail @ ..] = rest {
        rest = tail;
    }
    let [b'=', tail @ ..] = rest else {
        return None;
    };
    let mut rest = tail;
    while let [b' ' | b'\t' | b'\n' | b'\r' | b'\x0c', tail @ ..] = rest {
        rest = tail;
    }
    let rest = match rest {
        [quote @ (b'"' | b'\''), tail @ ..] => {
            let end = tail.iter().position(|&b| b == *quote)?;
            &tail[..end]
        }
        tail => {
            let end = tail
                .iter()
                .position(|&b| !is_label_byte(b))
                .unwrap_or(tail.len());
            &tail[..end]
        }
    };
    (!rest.is_empty()).then_some(rest)
}

const fn is_label_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':' | b'+')
}

fn find_ignore_ascii_case(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let last = haystack.len().checked_sub(needle.len())?;
    (from..=last).find(|&at| haystack[at..at + needle.len()].eq_ignore_ascii_case(needle))
}

pub use simdutf8::compat::Utf8Error;

/// Validates `bytes` as UTF-8.
pub fn validate_utf8(bytes: &[u8]) -> Result<(), Utf8Error> {
    simdutf8::compat::from_utf8(bytes).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_utf8_is_borrowed() {
        let input = "<p>naïve</p>".as_bytes();
        let decoded = decode(input);
        assert_eq!(decoded.encoding, UTF_8);
        assert!(!decoded.errors);
        assert!(matches!(decoded.bytes, Cow::Borrowed(_)));
        assert_eq!(&decoded.bytes[..], input);
    }

    #[test]
    fn owned_decode_keeps_the_utf8_fast_path() {
        let mut input = vec![0xEF, 0xBB, 0xBF];
        input.extend_from_slice(b"<p>x</p>");
        let decoded = decode_owned(input);
        assert_eq!(decoded.encoding, UTF_8);
        assert!(matches!(decoded.bytes, Cow::Owned(_)));
        assert_eq!(&decoded.bytes[..], b"<p>x</p>");
    }

    #[test]
    fn utf8_bom_is_consumed() {
        let mut input = vec![0xEF, 0xBB, 0xBF];
        input.extend_from_slice(b"<p>x</p>");
        let decoded = decode(&input);
        assert_eq!(decoded.encoding, UTF_8);
        assert_eq!(&decoded.bytes[..], b"<p>x</p>");
    }

    #[test]
    fn meta_charset_wins_over_utf8_validation() {
        // Valid UTF-8 bytes, but the document declares windows-1252: a browser
        // trusts the declaration, so do we.
        let input = b"<meta charset=windows-1252><p>caf\xc3\xa9</p>";
        let decoded = decode(input);
        assert_eq!(decoded.encoding, WINDOWS_1252);
        // The declaration wins even though the bytes are UTF-8, so "\xc3\xa9"
        // is two windows-1252 characters, exactly like in a browser.
        assert_eq!(
            std::str::from_utf8(&decoded.bytes).unwrap(),
            "<meta charset=windows-1252><p>cafÃ©</p>"
        );
    }

    #[test]
    fn http_equiv_content_charset() {
        let input = b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=gbk\">";
        assert_eq!(prescan_meta(input), Encoding::for_label(b"gbk"));
        let decoded = decode(
            b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=gbk\"><p>\xd6\xd0</p>",
        );
        assert_eq!(decoded.encoding, Encoding::for_label(b"gbk").unwrap());
        assert_eq!(
            std::str::from_utf8(&decoded.bytes).unwrap(),
            "<meta http-equiv=\"Content-Type\" content=\"text/html; charset=gbk\"><p>中</p>"
        );
    }

    #[test]
    fn unlabelled_invalid_utf8_falls_back_to_windows_1252() {
        let decoded = decode(b"<p>caf\xe9</p>");
        assert_eq!(decoded.encoding, WINDOWS_1252);
        assert!(!decoded.errors);
        assert_eq!(std::str::from_utf8(&decoded.bytes).unwrap(), "<p>café</p>");
    }

    #[test]
    fn declared_utf8_with_invalid_bytes_reports_errors() {
        let decoded = decode(b"<meta charset=utf-8><p>caf\xe9</p>");
        assert_eq!(decoded.encoding, UTF_8);
        assert!(decoded.errors);
        assert_eq!(
            std::str::from_utf8(&decoded.bytes).unwrap(),
            "<meta charset=utf-8><p>caf\u{fffd}</p>"
        );
    }

    #[test]
    fn utf16_bom() {
        let mut input = vec![0xFF, 0xFE];
        for unit in "<p>x</p>".encode_utf16() {
            input.extend_from_slice(&unit.to_le_bytes());
        }
        let decoded = decode(&input);
        assert_eq!(decoded.encoding, UTF_16LE);
        assert_eq!(&decoded.bytes[..], b"<p>x</p>");
    }

    #[test]
    fn meta_inside_a_comment_is_not_a_declaration() {
        // The prescan is a byte scan, so a commented-out <meta> would be found;
        // that is what the specification does too (the prescan runs before
        // tokenisation), so the test pins the behaviour rather than a wish.
        let input = b"<!-- <meta charset=windows-1252> --><p>\xc3\xa9</p>";
        assert_eq!(prescan_meta(input), Some(WINDOWS_1252));
    }

    #[test]
    fn charset_without_equals_is_ignored() {
        let input = b"<meta name=charset content=utf-8><p>caf\xc3\xa9</p>";
        assert_eq!(prescan_meta(input), None);
    }

    #[test]
    fn unknown_label_falls_through() {
        let input = b"<meta charset=bogus-enc><p>caf\xc3\xa9</p>";
        assert_eq!(prescan_meta(input), None);
        assert_eq!(decode(input).encoding, UTF_8);
    }

    #[test]
    fn nothing_declared_outside_prescan() {
        let mut input = vec![b' '; PRESCAN_LEN];
        input.extend_from_slice(b"<meta charset=windows-1252><p>caf\xe9</p>");
        // Not found within the prescan window, so the unlabelled fallback runs
        // and, being the same encoding here, produces the same text.
        assert_eq!(prescan_meta(&input), None);
        let decoded = decode(&input);
        assert_eq!(decoded.encoding, WINDOWS_1252);
    }

    #[test]
    fn explicit_encoding_beats_the_meta_declaration() {
        // Valid UTF-8 bytes in a page that claims UTF-8, decoded as 1252 the way
        // a transport header would force it.
        let input = b"<meta charset=utf-8><p>caf\xc3\xa9</p>";
        let decoded = decode_with(input, WINDOWS_1252);
        assert_eq!(decoded.encoding, WINDOWS_1252);
        assert_eq!(
            std::str::from_utf8(&decoded.bytes).unwrap(),
            "<meta charset=utf-8><p>cafÃ©</p>"
        );
    }

    #[test]
    fn explicit_utf8_stays_borrowed() {
        let input = b"<p>na\xc3\xafve</p>";
        let decoded = decode_with(input, UTF_8);
        assert_eq!(decoded.encoding, UTF_8);
        assert!(!decoded.errors);
        assert!(matches!(decoded.bytes, Cow::Borrowed(_)));
        assert_eq!(&decoded.bytes[..], input);
    }

    #[test]
    fn explicit_utf8_reports_invalid_bytes() {
        let decoded = decode_with(b"<p>caf\xe9</p>", UTF_8);
        assert_eq!(decoded.encoding, UTF_8);
        assert!(decoded.errors);
        assert_eq!(
            std::str::from_utf8(&decoded.bytes).unwrap(),
            "<p>caf\u{fffd}</p>"
        );
    }

    #[test]
    fn bom_outranks_the_explicit_encoding() {
        // The specification checks the BOM before the transport label.
        let mut input = vec![0xEF, 0xBB, 0xBF];
        input.extend_from_slice(b"<p>na\xc3\xafve</p>");
        let decoded = decode_with(&input, WINDOWS_1252);
        assert_eq!(decoded.encoding, UTF_8);
        assert_eq!(&decoded.bytes[..], b"<p>na\xc3\xafve</p>");
    }

    #[test]
    fn lookup_accepts_labels_and_aliases() {
        assert_eq!(lookup("UTF-8"), Some(UTF_8));
        assert_eq!(lookup(" latin1 "), Some(WINDOWS_1252));
        assert_eq!(lookup("gbk"), Encoding::for_label(b"gbk"));
        assert_eq!(lookup("bogus-enc"), None);
    }

    // --- UTF-8 validation ------------------------------------------------

    #[test]
    fn accepts_ascii() {
        assert!(validate_utf8(b"<html><body>hello</body></html>").is_ok());
    }

    #[test]
    fn accepts_multibyte() {
        assert!(validate_utf8("中文 ❤️ 🚀".as_bytes()).is_ok());
    }

    #[test]
    fn rejects_invalid_byte() {
        let err = validate_utf8(b"abc\xFFdef").unwrap_err();
        assert_eq!(err.valid_up_to(), 3);
        assert_eq!(err.error_len(), Some(1));
    }

    #[test]
    fn rejects_overlong() {
        // Overlong encoding of U+0000.
        let err = validate_utf8(b"\xC0\x80").unwrap_err();
        assert_eq!(err.valid_up_to(), 0);
        assert!(err.error_len().is_some());
    }

    #[test]
    fn rejects_truncated() {
        // Truncated 3-byte sequence at EOF.
        let err = validate_utf8(b"a\xE4\xB8").unwrap_err();
        assert_eq!(err.valid_up_to(), 1);
        assert_eq!(err.error_len(), None);
    }

    #[test]
    fn matches_std_on_random_bytes() {
        let mut state = 0x5eed_1234_abcd_ef01u64;
        for _ in 0..4096 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let len = (state as usize & 0x1F).max(1);
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                buf.push(state as u8);
            }
            let ours = validate_utf8(&buf);
            let std_result = std::str::from_utf8(&buf);
            assert_eq!(ours.is_ok(), std_result.is_ok(), "buf={buf:?}");
            if let (Err(a), Err(b)) = (ours, std_result) {
                assert_eq!(a.valid_up_to(), b.valid_up_to());
                assert_eq!(a.error_len(), b.error_len());
            }
        }
    }
}
