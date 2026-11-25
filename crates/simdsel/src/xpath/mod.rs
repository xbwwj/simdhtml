pub mod ast;
pub mod eval;
pub mod parser;
pub mod value;

use value::{NodeRef, Value};

use simdhtml::document::Document;

use crate::error::{Error, XPathErrorKind};

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
    /// Returns [`Error::XPath`] for malformed input and unsupported syntax.
    pub fn new(expr: &str) -> Result<Self, Error> {
        Ok(Self {
            expr: parser::parse(expr).map_err(Error::XPath)?,
        })
    }

    /// Evaluates the expression against `doc`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::XPath`] for evaluation-time problems such as an unknown
    /// function arity, or when the visit budget is exhausted (a guard against
    /// `//a//b` style blow-up on huge documents).
    pub fn eval(&self, doc: &Document<'_>) -> Result<Value, Error> {
        eval::evaluate(doc, &self.expr).map_err(Error::XPath)
    }

    /// Evaluates the expression with `node` as the context node.
    ///
    /// Relative paths (`./p`, `.//a`), `.`, and `string()`/`number()`/… with no
    /// argument resolve against `node` instead of the document node.
    pub fn eval_at(&self, doc: &Document<'_>, node: NodeRef) -> Result<Value, Error> {
        eval::evaluate_at(doc, &self.expr, node).map_err(Error::XPath)
    }
}
