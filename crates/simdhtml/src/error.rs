use thiserror::Error;

use crate::encoding;

/// Fatal parse errors.
///
/// Recoverable problems are reported per-document in [`Document::flags`] and
/// [`Document::diagnostics`] instead.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ParseError {
    #[error("input exceeds u32::MAX bytes")]
    InputTooLarge,
    #[error("{0}")]
    InvalidUtf8(encoding::Utf8Error),
}

pub type Result<T, E = ParseError> = ::core::result::Result<T, E>;
