//! The compiled XPath AST.
//!
//! Abbreviations are expanded during parsing (`.` → `self::node()`,
//! `//` → `descendant-or-self::node()`, `@x` → `attribute::x`), so the
//! evaluator only ever sees the long form.

/// An expression.
#[derive(Clone, Debug)]
pub(super) enum Expr {
    /// A location path.
    Path(Path),
    /// `primary[predicate]...`.
    Filter {
        /// The primary expression.
        primary: Box<Expr>,
        /// Predicates applied to its node-set.
        predicates: Vec<Expr>,
    },
    /// `left | right`.
    Union(Box<Expr>, Box<Expr>),
    /// Binary operator.
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// Unary minus.
    Negate(Box<Expr>),
    /// String literal.
    Literal(String),
    /// Number literal.
    Number(f64),
    /// Function call.
    Call(Func, Vec<Expr>),
}

/// A location path: start set, then steps.
#[derive(Clone, Debug)]
pub(super) struct Path {
    /// Start set for `(expr)/steps`; `None` for a plain relative or absolute
    /// path.
    pub base: Option<Box<Expr>>,
    /// `true` for `/...` (start at the document node).
    pub absolute: bool,
    /// Steps in order.
    pub steps: Vec<Step>,
}

/// One location step.
#[derive(Clone, Debug)]
pub(super) struct Step {
    /// The axis.
    pub axis: Axis,
    /// The node test.
    pub test: NodeTest,
    /// Predicates, applied left to right.
    pub predicates: Vec<Expr>,
}

/// XPath axes in the supported subset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Axis {
    /// `child`.
    Child,
    /// `descendant`.
    Descendant,
    /// `descendant-or-self`.
    DescendantOrSelf,
    /// `self`.
    Self_,
    /// `parent`.
    Parent,
    /// `ancestor`.
    Ancestor,
    /// `attribute`.
    Attribute,
    /// `following-sibling`.
    FollowingSibling,
    /// `preceding-sibling`.
    PrecedingSibling,
}

/// A node test.
#[derive(Clone, Debug)]
pub(super) enum NodeTest {
    /// `*`: any node of the axis' principal kind.
    Any,
    /// A name test.
    Name(String),
    /// `text()`.
    Text,
    /// `node()`.
    Node,
}

/// Binary operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BinOp {
    /// `or`
    Or,
    /// `and`
    And,
    /// `=`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `div`
    Div,
    /// `mod`
    Mod,
}

/// The supported core function library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Func {
    /// `position()`
    Position,
    /// `last()`
    Last,
    /// `count(nodeset)`
    Count,
    /// `not(boolean)`
    Not,
    /// `true()`
    True,
    /// `false()`
    False,
    /// `name()`
    Name,
    /// `local-name()`
    LocalName,
    /// `string()`
    String,
    /// `number()`
    Number,
    /// `boolean()`
    Boolean,
    /// `contains(haystack, needle)`
    Contains,
    /// `starts-with(haystack, prefix)`
    StartsWith,
    /// `normalize-space()`
    NormalizeSpace,
    /// `concat(a, b, ...)`
    Concat,
}

impl Func {
    /// Resolves a function name.
    pub(super) fn lookup(name: &str) -> Option<Self> {
        Some(match name {
            "position" => Self::Position,
            "last" => Self::Last,
            "count" => Self::Count,
            "not" => Self::Not,
            "true" => Self::True,
            "false" => Self::False,
            "name" => Self::Name,
            "local-name" => Self::LocalName,
            "string" => Self::String,
            "number" => Self::Number,
            "boolean" => Self::Boolean,
            "contains" => Self::Contains,
            "starts-with" => Self::StartsWith,
            "normalize-space" => Self::NormalizeSpace,
            "concat" => Self::Concat,
            _ => return None,
        })
    }
}
