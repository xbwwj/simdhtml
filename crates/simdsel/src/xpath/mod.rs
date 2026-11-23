mod ast;
mod eval;
mod parser;
mod value;

pub use value::{NodeRef, RefKind, Value};

use simdhtml::Document;

/// A compiled XPath 1.0 expression.
///
/// Compilation validates syntax and the supported subset; evaluation can still
/// fail on e.g. an unknown function arity.
#[derive(Clone, Debug)]
pub struct XPath {
    expr: ast::Expr,
}

impl XPath {
    /// Compiles `expr`.
    ///
    /// # Errors
    ///
    /// Returns [`XPathError::Syntax`] for malformed input and
    /// [`XPathError::Unsupported`] for syntax outside the documented subset.
    pub fn new(expr: &str) -> Result<Self, XPathError> {
        Ok(Self {
            expr: parser::parse(expr)?,
        })
    }

    /// Evaluates the expression against `doc`.
    ///
    /// # Errors
    ///
    /// Returns [`XPathError::Eval`] for evaluation-time problems such as an
    /// unknown function arity, and [`XPathError::TooComplex`] when the visit
    /// budget is exhausted (a guard against `//a//b` style blow-up on huge
    /// documents; the budget is `256 × node count`, so it is not reachable for
    /// realistic queries).
    pub fn eval(&self, doc: &Document<'_>) -> Result<Value, XPathError> {
        eval::evaluate(doc, &self.expr)
    }

    /// Evaluates the expression with `node` as the context node.
    ///
    /// Relative paths (`./p`, `.//a`), `.`, and `string()`/`number()`/… with no
    /// argument resolve against `node` instead of the document node.
    pub fn eval_at(&self, doc: &Document<'_>, node: NodeRef) -> Result<Value, XPathError> {
        eval::evaluate_at(doc, &self.expr, node)
    }
}

/// Why an XPath expression failed to compile or evaluate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XPathError {
    /// Malformed expression; `position` is a byte offset into the input.
    Syntax {
        /// Byte offset of the offending token.
        position: usize,
        /// Human-readable description.
        message: String,
    },
    /// Valid XPath 1.0 outside the supported subset.
    Unsupported(&'static str),
    /// Evaluation-time failure (unknown function arity, bad argument type).
    Eval(String),
    /// The evaluation visit budget was exhausted.
    TooComplex,
}

impl std::fmt::Display for XPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax { position, message } => {
                write!(f, "xpath syntax error at byte {position}: {message}")
            }
            Self::Unsupported(what) => write!(f, "xpath unsupported: {what}"),
            Self::Eval(message) => write!(f, "xpath evaluation error: {message}"),
            Self::TooComplex => write!(f, "xpath expression exceeded the evaluation budget"),
        }
    }
}

impl std::error::Error for XPathError {}
