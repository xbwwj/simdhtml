use thiserror::Error;

/// Fatal parse errors.
///
/// Recoverable problems are reported per-document in [`Document::flags`] and
/// [`Document::diagnostics`] instead.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ParseError {
    #[error("input exceeds u32::MAX bytes")]
    InputTooLarge,
    #[error("input is not valid UTF-8")]
    InvalidUtf8,
}

pub type Result<T, E = ParseError> = ::core::result::Result<T, E>;
