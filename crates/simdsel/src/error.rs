use thiserror::Error;

use crate::{MatchError, SelectorError};

/// High-level error type.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum Error {
    /// Parsing the HTML failed.
    #[error("parse error: {0}")]
    Parse(#[from] simdhtml::ParseError),

    /// The selector was invalid.
    #[error("selector error: {0}")]
    Selector(#[from] SelectorError),

    /// Selector evaluation hit a limit.
    #[error("match error: {0:?}")]
    Match(#[from] MatchError),
}
