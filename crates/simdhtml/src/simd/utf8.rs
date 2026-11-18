//! UTF-8 validation.
//!
//! v1 delegates to [`simdutf8::compat`], which is a SIMD-accelerated,
//! API-compatible replacement for `std::str::from_utf8`.  The wrapper keeps
//! the error information we need for diagnostics and future encoding
//! fallback, and gives us one place to swap in a fused lookup4 validator
//! later.

use std::fmt;

/// Detailed UTF-8 validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Utf8Error {
    valid_up_to: usize,
    error_len: Option<usize>,
}

impl Utf8Error {
    /// Byte index of the first invalid byte.
    #[must_use]
    pub const fn valid_up_to(&self) -> usize {
        self.valid_up_to
    }

    /// Length of the invalid sequence, or `None` if the input ended mid-character.
    #[must_use]
    pub const fn error_len(&self) -> Option<usize> {
        self.error_len
    }
}

impl fmt::Display for Utf8Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.error_len {
            Some(len) => write!(
                f,
                "invalid utf-8 sequence of {len} bytes from index {}",
                self.valid_up_to
            ),
            None => write!(
                f,
                "incomplete utf-8 byte sequence from index {}",
                self.valid_up_to
            ),
        }
    }
}

impl std::error::Error for Utf8Error {}

/// Validates `bytes` as UTF-8.
///
/// # Errors
///
/// Returns [`Utf8Error`] if the input is not valid UTF-8.
pub fn validate_utf8(bytes: &[u8]) -> Result<(), Utf8Error> {
    simdutf8::compat::from_utf8(bytes)
        .map(|_| ())
        .map_err(|e| Utf8Error {
            valid_up_to: e.valid_up_to(),
            error_len: e.error_len(),
        })
}

/// Returns the input as `&str` after validating it.
///
/// # Errors
///
/// Returns [`Utf8Error`] if the input is not valid UTF-8.
pub fn as_str(bytes: &[u8]) -> Result<&str, Utf8Error> {
    simdutf8::compat::from_utf8(bytes).map_err(|e| Utf8Error {
        valid_up_to: e.valid_up_to(),
        error_len: e.error_len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
