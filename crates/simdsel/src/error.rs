use simdhtml::error::ParseError;
use thiserror::Error;

/// The single public error type.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum Error {
    /// Parsing the HTML failed.
    #[error("parse error: {0}")]
    Parse(#[from] ParseError),
    /// CSS parsing or evaluation failed.
    #[error(transparent)]
    Css(CssError),
    /// XPath parsing or evaluation failed.
    #[error(transparent)]
    XPath(XPathError),
}

/// CSS error data, carried inside [`Error::Css`].
#[derive(Clone, Debug, Eq, Error, PartialEq, Hash)]
pub enum CssError {
    #[error("selector error: {message} at byte {position}")]
    Selector { message: String, position: usize },
    #[error("selector evaluation hit the depth limit")]
    DepthLimit,
}

/// XPath error data, carried inside [`Error::XPath`].
#[derive(Clone, Debug, Eq, Error, PartialEq, Hash)]
pub enum XPathError {
    #[error("xpath syntax error at byte {position}: {message}")]
    Syntax { position: usize, message: String },
    #[error("xpath unsupported: {0}")]
    Unsupported(&'static str),
    #[error("xpath evaluation error: {0}")]
    Eval(String),
    #[error("xpath expression exceeded the evaluation budget")]
    TooComplex,
}

pub type Result<T, E = Error> = ::core::result::Result<T, E>;
