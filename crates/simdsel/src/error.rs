use std::fmt;

/// The single public error type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// Parsing the HTML failed.
    Parse(simdhtml::error::ParseError),

    /// CSS parsing or evaluation failed.
    Css(CssErrorKind),

    /// XPath parsing or evaluation failed.
    XPath(XPathErrorKind),
}

/// CSS error data, carried inside [`Error::Css`].
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum CssErrorKind {
    Selector { message: String, position: usize },
    DepthLimit,
}

/// XPath error data, carried inside [`Error::XPath`].
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum XPathErrorKind {
    Syntax { position: usize, message: String },
    Unsupported(&'static str),
    Eval(String),
    TooComplex,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(error) => write!(f, "parse error: {error}"),
            Self::Css(CssErrorKind::Selector { message, position }) => {
                write!(f, "selector error: {message} at byte {position}")
            }
            Self::Css(CssErrorKind::DepthLimit) => {
                write!(f, "selector evaluation hit the depth limit")
            }
            Self::XPath(XPathErrorKind::Syntax { position, message }) => {
                write!(f, "xpath syntax error at byte {position}: {message}")
            }
            Self::XPath(XPathErrorKind::Unsupported(what)) => {
                write!(f, "xpath unsupported: {what}")
            }
            Self::XPath(XPathErrorKind::Eval(message)) => {
                write!(f, "xpath evaluation error: {message}")
            }
            Self::XPath(XPathErrorKind::TooComplex) => {
                write!(f, "xpath expression exceeded the evaluation budget")
            }
        }
    }
}

impl From<simdhtml::error::ParseError> for Error {
    fn from(error: simdhtml::error::ParseError) -> Self {
        Self::Parse(error)
    }
}

impl std::error::Error for Error {}
