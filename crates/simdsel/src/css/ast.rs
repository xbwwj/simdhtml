/// A comma-separated selector list.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SelectorList {
    /// One or more complex selectors.
    pub complexes: Vec<Complex>,
}

/// Compounds joined by combinators, plus an optional trailing pseudo-element.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Complex {
    /// Compounds from left to right.
    pub compounds: Vec<Compound>,
    /// Length is `compounds.len() - 1`.
    pub combinators: Vec<Combinator>,
    /// `::text` / `::attr(...)` at the end.
    pub pseudo: Option<PseudoElement>,
}

/// A compound selector, evaluated as one bitset predicate.
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Compound {
    /// `None` means any element (`*` or omitted).
    pub tag: Option<String>,
    pub universal: bool,
    /// A universal the parser inserted because a pseudo-element had no compound
    /// of its own (`A ::text`).  After a descendant combinator it is
    /// `descendant-or-self`, which is how parsel attaches the pseudo-element.
    pub implied_universal: bool,
    /// `#id`.
    pub id: Option<String>,
    /// `.class` list.
    pub classes: Vec<String>,
    /// Attribute predicates.
    pub attrs: Vec<AttrPred>,
    /// Pseudo classes.
    pub pseudos: Vec<Pseudo>,
}

/// Combinators.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Combinator {
    /// `A B`
    Descendant,
    /// `A > B`
    Child,
    /// `A + B`
    Adjacent,
    /// `A ~ B`
    GeneralSibling,
}

/// Attribute selector operator.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum AttrOp {
    /// `[attr]`
    Exists,
    /// `[attr=value]`
    Equals,
    /// `[attr~=value]`
    Includes,
    /// `[attr|=value]`
    DashMatch,
    /// `[attr^=value]`
    Prefix,
    /// `[attr$=value]`
    Suffix,
    /// `[attr*=value]`
    Substring,
    /// `[attr!=value]` (non-standard CSS extension)
    NotEquals,
}

/// One attribute predicate.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct AttrPred {
    /// Lowercase attribute name.
    pub name: String,
    /// Operator.
    pub op: AttrOp,
    /// Expected value (`None` for [`AttrOp::Exists`]).
    pub value: Option<String>,
    /// `i` flag.
    pub case_insensitive: bool,
}

/// A pseudo class.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Pseudo {
    /// `:first-child`
    FirstChild,
    /// `:last-child`
    LastChild,
    /// `:only-child`
    OnlyChild,
    /// `:nth-child(An+B)`
    NthChild(i32, i32),
    /// `:nth-last-child(An+B)`
    NthLastChild(i32, i32),
    /// `:first-of-type`
    FirstOfType,
    /// `:last-of-type`
    LastOfType,
    /// `:nth-of-type(An+B)`
    NthOfType(i32, i32),
    /// `:nth-last-of-type(An+B)`
    NthLastOfType(i32, i32),
    /// `:not(...)`
    Not(Vec<Complex>),
    /// `:is(...)` / `:where(...)`
    Is(Vec<Complex>),
    /// `:has(...)`
    Has(Vec<RelativeSelector>),
    /// `:root`
    Root,
    /// `:empty`
    Empty,
    /// `:scope` (current query root)
    Scope,
    /// `:link` (`a`/`area`/`link` with an `href`).
    Link,
    /// `:checked` (checked `input`, selected `option`).
    Checked,
    /// `:enabled` (a disable-able element that is not disabled).
    Enabled,
    /// `:disabled` (a disable-able element with the `disabled` attribute).
    Disabled,
    /// `:lang(tag)` (nearest `lang` ancestor-or-self is `tag` or `tag-*`).
    Lang(String),
    /// `:only-of-type`
    OnlyOfType,
    /// Non-standard `:contains(text)`
    Contains(String),
    /// A dynamic pseudo class that can never match a static document.
    Never,
}

/// A relative selector inside `:has()`.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct RelativeSelector {
    /// Relation to the `:has()` subject.
    pub relation: Combinator,
    /// The complex selector after the relation.
    pub complex: Complex,
}

/// Pseudo-elements supported by parsel.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum PseudoElement {
    /// `::text`
    Text,
    /// `::attr(name)`
    Attr(String),
}
