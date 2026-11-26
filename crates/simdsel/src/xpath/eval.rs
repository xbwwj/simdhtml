use super::XPathError;
use super::ast::{Axis, BinOp, Expr, Func, NodeTest, Path, Step};
use super::value::{NodeRef, RefKind, Value};
use crate::access::{attr_name, attr_range};

/// Last id of the text run starting at `id`, or `None` for a single node.
/// Adjacent text siblings are one node in the XPath data model and are
/// consecutive ids with the same parent.
pub(super) fn text_run(doc: &Document<'_>, id: NodeId) -> Option<NodeId> {
    let parent = doc.node(id).parent();
    let mut last = id;
    while let Some(next) = (last + 1 < NodeId::new(doc.nodes.len() as u32)).then_some(last + 1) {
        let node = doc.node(next);
        if !node.is_text() || node.parent() != parent {
            break;
        }
        last = next;
    }
    (last != id).then_some(last)
}

/// Whether this text node is outside the root element: XPath's root node has no
/// text children, though the tape keeps that text.
fn outside_root(doc: &Document<'_>, id: NodeId) -> bool {
    doc.node(id).parent() == NodeId::ROOT
}

/// Is this text node the continuation of a run whose first node is its
/// preceding sibling?
fn text_continuation(doc: &Document<'_>, id: NodeId) -> bool {
    if id == NodeId::ROOT {
        return false;
    }
    let previous = doc.node(id - 1);
    previous.is_text() && previous.parent() == doc.node(id).parent()
}
use simdhtml::document::Document;
use simdhtml::document::id::NodeId;
use simdhtml::document::node::NodeKind;

/// Evaluation context: the context node plus its position and set size, which
/// `position()` and `last()` read.
#[derive(Clone, Copy)]
struct Ctx {
    node: NodeRef,
    position: usize,
    size: usize,
}

/// Evaluates a compiled expression against `doc`.
pub(super) fn evaluate(doc: &Document<'_>, expr: &Expr) -> Result<Value, XPathError> {
    evaluate_at(doc, expr, NodeRef::document())
}

/// Evaluates `expr` with `node` as the context node (relative paths, `.`,
/// `string()`, `position()` all resolve against it).
pub(super) fn evaluate_at(
    doc: &Document<'_>,
    expr: &Expr,
    node: NodeRef,
) -> Result<Value, XPathError> {
    let mut evaluator = Evaluator {
        doc,
        visits: 0,
        // 256 visits per node bounds `//a//b`-style products.
        budget: (doc.nodes.len() as u64 + 1) * 256,
    };
    let ctx = Ctx {
        node,
        position: 1,
        size: 1,
    };
    evaluator.eval(expr, &ctx)
}

struct Evaluator<'d, 'a> {
    doc: &'d Document<'a>,
    visits: u64,
    budget: u64,
}

impl Evaluator<'_, '_> {
    fn eval(&mut self, expr: &Expr, ctx: &Ctx) -> Result<Value, XPathError> {
        match expr {
            Expr::Path(path) => Ok(Value::Nodes(self.path(path, ctx)?)),
            Expr::Filter {
                primary,
                predicates,
            } => {
                let value = self.eval(primary, ctx)?;
                let Value::Nodes(nodes) = value else {
                    return Err(XPathError::Eval(
                        "a predicate can only filter a node-set".to_owned(),
                    ));
                };
                Ok(Value::Nodes(self.filter(nodes, predicates)?))
            }
            Expr::Union(left, right) => {
                let mut nodes = self.nodes(left, ctx)?;
                nodes.extend(self.nodes(right, ctx)?);
                Ok(Value::Nodes(sorted(nodes)))
            }
            Expr::Binary(op, left, right) => self.binary(*op, left, right, ctx),
            Expr::Negate(operand) => {
                let value = self.eval(operand, ctx)?;
                Ok(Value::Number(-value.to_number(self.doc)))
            }
            Expr::Literal(text) => Ok(Value::Str(text.clone())),
            Expr::Number(value) => Ok(Value::Number(*value)),
            Expr::Call(func, args) => self.call(*func, args, ctx),
        }
    }

    fn nodes(&mut self, expr: &Expr, ctx: &Ctx) -> Result<Vec<NodeRef>, XPathError> {
        match self.eval(expr, ctx)? {
            Value::Nodes(nodes) => Ok(nodes),
            _ => Err(XPathError::Eval("expected a node-set".to_owned())),
        }
    }

    fn path(&mut self, path: &Path, ctx: &Ctx) -> Result<Vec<NodeRef>, XPathError> {
        let mut set = match &path.base {
            Some(base) => self.nodes(base, ctx)?,
            None if path.absolute => vec![NodeRef::document()],
            None => vec![ctx.node],
        };
        let mut index = 0;
        while index < path.steps.len() {
            let mut step = &path.steps[index];
            let mut axis = step.axis;
            // `//x` is `descendant-or-self::node()/child::x`, which is exactly
            // `descendant::x` when the child step has no predicates; fusing
            // avoids materialising every node twice.
            if step.axis == Axis::DescendantOrSelf
                && matches!(step.test, NodeTest::Node)
                && step.predicates.is_empty()
                && index + 1 < path.steps.len()
                && path.steps[index + 1].axis == Axis::Child
                && path.steps[index + 1].predicates.is_empty()
            {
                index += 1;
                step = &path.steps[index];
                axis = Axis::Descendant;
            }
            if axis == Axis::Descendant
                && step.predicates.is_empty()
                && let NodeTest::Name(name) = &step.test
                && let Some(named) = self.descendant_named(&set, name)?
            {
                set = sorted(named);
                index += 1;
                continue;
            }
            let mut next = Vec::new();
            for node in &set {
                let candidates = self.axis(axis, *node)?;
                let mut candidates = self.test(step, candidates);
                if !step.predicates.is_empty() {
                    candidates = self.filter(candidates, &step.predicates)?;
                }
                next.extend(candidates);
            }
            set = sorted(next);
            index += 1;
        }
        Ok(set)
    }

    /// Descendant elements named `name`, seeded from the tag-code column.  `None`
    /// when the name has no dedicated code (the generic axis+test path).
    fn descendant_named(
        &mut self,
        contexts: &[NodeRef],
        name: &str,
    ) -> Result<Option<Vec<NodeRef>>, XPathError> {
        let code = simdhtml::tag_code::code_of(name.as_bytes());
        if code == 0 {
            return Ok(None);
        }
        let mut ids: Vec<NodeId> = self
            .doc
            .tags
            .element
            .iter()
            .enumerate()
            .filter_map(|(tag, &element)| {
                (element != NodeId::NONE && self.doc.tags.code[tag] == code).then_some(element)
            })
            .collect();
        // Cloned elements share their start tag's token, so they are not in
        // `tags.element`; the adoption agency can create them.
        ids.extend(
            self.doc
                .extra_tag_elements()
                .iter()
                .filter(|(tag, _)| self.doc.tags.code[tag.index()] == code)
                .map(|&(_, node)| node),
        );
        ids.sort_unstable();
        ids.dedup();

        let mut out = Vec::new();
        for ctx in contexts {
            let node = self.doc.node(ctx.node_id());
            if !matches!(node.kind(), NodeKind::Document | NodeKind::Element) {
                continue;
            }
            let start = ctx.node_id() + 1;
            let end = node.subtree_end();
            let lo = ids.partition_point(|&id| id < start);
            let hi = ids.partition_point(|&id| id < end);
            out.extend(ids[lo..hi].iter().map(|&id| NodeRef::element(id)));
        }
        self.visits += (ids.len() + contexts.len() + out.len()) as u64;
        if self.visits > self.budget {
            return Err(XPathError::TooComplex);
        }
        Ok(Some(out))
    }

    /// Applies predicates left to right with per-axis position semantics.
    fn filter(
        &mut self,
        mut nodes: Vec<NodeRef>,
        predicates: &[Expr],
    ) -> Result<Vec<NodeRef>, XPathError> {
        for predicate in predicates {
            let size = nodes.len();
            let mut kept = Vec::with_capacity(size);
            for (index, node) in nodes.iter().enumerate() {
                let ctx = Ctx {
                    node: *node,
                    position: index + 1,
                    size,
                };
                let value = self.eval(predicate, &ctx)?;
                let keep = match value {
                    // A predicate that evaluates to a number is the shorthand
                    // for `position() = number`.
                    Value::Number(n) => n == (index + 1) as f64,
                    other => other.to_boolean(),
                };
                if keep {
                    kept.push(*node);
                }
            }
            nodes = kept;
        }
        Ok(nodes)
    }

    /// Nodes on `axis`, in axis order (reverse document order for `ancestor`
    /// and `preceding-sibling`, as the specification requires for positions).
    fn axis(&mut self, axis: Axis, node: NodeRef) -> Result<Vec<NodeRef>, XPathError> {
        let out = match axis {
            Axis::Self_ => vec![node],
            Axis::Child => self.children(node),
            Axis::Descendant => self.descendants(node),
            Axis::DescendantOrSelf => {
                let mut out = vec![node];
                out.extend(self.descendants(node));
                out
            }
            Axis::Parent => match node.kind() {
                RefKind::Attribute => vec![NodeRef::element(node.node_id())],
                RefKind::Document => Vec::new(),
                _ => {
                    let parent = self.doc.node(node.node_id()).parent();
                    vec![self.ref_of(parent)]
                }
            },
            Axis::Ancestor => {
                let mut out = Vec::new();
                let mut current = match node.kind() {
                    RefKind::Attribute => Some(node.node_id()),
                    RefKind::Document => None,
                    _ => Some(self.doc.node(node.node_id()).parent()),
                };
                while let Some(id) = current {
                    out.push(self.ref_of(id));
                    if self.doc.node(id).kind() == NodeKind::Document {
                        break;
                    }
                    current = Some(self.doc.node(id).parent());
                }
                out
            }
            Axis::Attribute => {
                if node.kind() != RefKind::Element {
                    Vec::new()
                } else {
                    attr_range(self.doc, node.node_id())
                        .map(|attr| NodeRef::attribute(node.node_id(), attr))
                        .collect()
                }
            }
            Axis::FollowingSibling => {
                if node.kind() == RefKind::Attribute {
                    Vec::new()
                } else {
                    let mut out = Vec::new();
                    let mut sibling = self.doc.node(node.node_id()).next_sibling();
                    while sibling != NodeId::NONE {
                        out.push(self.ref_of(sibling));
                        sibling = self.doc.node(sibling).next_sibling();
                    }
                    out
                }
            }
            Axis::PrecedingSibling => {
                if node.kind() == RefKind::Attribute {
                    Vec::new()
                } else {
                    let parent = self.doc.node(node.node_id()).parent();
                    let mut out = Vec::new();
                    let mut child = self.doc.node(parent).first_child();
                    while child != NodeId::NONE && child != node.node_id() {
                        out.push(self.ref_of(child));
                        child = self.doc.node(child).next_sibling();
                    }
                    out.reverse();
                    out
                }
            }
        };
        self.visits += out.len() as u64;
        if self.visits > self.budget {
            return Err(XPathError::TooComplex);
        }
        Ok(out)
    }

    fn children(&self, node: NodeRef) -> Vec<NodeRef> {
        if !matches!(node.kind(), RefKind::Element | RefKind::Document) {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut child = self.doc.node(node.node_id()).first_child();
        while child != NodeId::NONE {
            out.push(self.ref_of(child));
            child = self.doc.node(child).next_sibling();
        }
        out
    }

    /// Descendants of an element or the document, in document order.
    ///
    /// The tape is preorder with `subtree_end`, so this is one contiguous id
    /// range — no recursion, no pointer chasing.
    fn descendants(&self, node: NodeRef) -> Vec<NodeRef> {
        if !matches!(node.kind(), RefKind::Element | RefKind::Document) {
            return Vec::new();
        }
        let start = node.node_id().get() + 1;
        let end = self.doc.node(node.node_id()).subtree_end().get();
        (start..end)
            .map(|id| self.ref_of(NodeId::new(id)))
            .collect()
    }

    fn ref_of(&self, id: NodeId) -> NodeRef {
        match self.doc.node(id).kind() {
            NodeKind::Document => NodeRef::document(),
            NodeKind::Element => NodeRef::element(id),
            NodeKind::Text => NodeRef::text(id),
            // The XPath data model has comment nodes; they are not part of the
            // supported subset yet, so they are mapped to text-shaped references
            // that `node()` never matches.
            NodeKind::Comment => NodeRef::text(id),
        }
    }

    /// Filters axis candidates by the step's node test.
    fn test(&self, step: &Step, candidates: Vec<NodeRef>) -> Vec<NodeRef> {
        let attribute_axis = step.axis == Axis::Attribute;
        match &step.test {
            NodeTest::Node => candidates
                .into_iter()
                .filter(|node| {
                    node.kind() != RefKind::Text
                        || (!outside_root(self.doc, node.node_id())
                            && !text_continuation(self.doc, node.node_id()))
                })
                .collect(),
            NodeTest::Text => candidates
                .into_iter()
                .filter(|node| {
                    node.kind() == RefKind::Text
                        && !outside_root(self.doc, node.node_id())
                        && !text_continuation(self.doc, node.node_id())
                })
                .collect(),
            NodeTest::Any => {
                let mut seen: Vec<&str> = Vec::new();
                candidates
                    .into_iter()
                    .filter(|node| {
                        if attribute_axis {
                            // HTML keeps the first of duplicate attributes.
                            node.kind() == RefKind::Attribute && {
                                let name = std::str::from_utf8(attr_name(
                                    self.doc,
                                    node.attr_index().unwrap(),
                                ))
                                .expect("Document input is valid UTF-8");
                                let first =
                                    !seen.iter().any(|other| other.eq_ignore_ascii_case(name));
                                if first {
                                    seen.push(name);
                                }
                                first
                            }
                        } else {
                            node.kind() == RefKind::Element
                        }
                    })
                    .collect()
            }
            NodeTest::Name(name) => {
                let mut matched_attribute = false;
                candidates
                    .into_iter()
                    .filter(|node| {
                        if attribute_axis {
                            let hit = !matched_attribute
                                && node.kind() == RefKind::Attribute
                                && name_eq(attr_name(self.doc, node.attr_index().unwrap()), name);
                            matched_attribute |= hit;
                            hit
                        } else {
                            node.kind() == RefKind::Element
                                && name_eq(node.name(self.doc).as_bytes(), name)
                        }
                    })
                    .collect()
            }
        }
    }

    fn binary(
        &mut self,
        op: BinOp,
        left: &Expr,
        right: &Expr,
        ctx: &Ctx,
    ) -> Result<Value, XPathError> {
        match op {
            BinOp::Or => {
                let left = self.eval(left, ctx)?.to_boolean();
                if left {
                    return Ok(Value::Bool(true));
                }
                Ok(Value::Bool(self.eval(right, ctx)?.to_boolean()))
            }
            BinOp::And => {
                let left = self.eval(left, ctx)?.to_boolean();
                if !left {
                    return Ok(Value::Bool(false));
                }
                Ok(Value::Bool(self.eval(right, ctx)?.to_boolean()))
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let left = self.eval(left, ctx)?;
                let right = self.eval(right, ctx)?;
                Ok(Value::Bool(self.compare(op, &left, &right)?))
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                let left = self.eval(left, ctx)?.to_number(self.doc);
                let right = self.eval(right, ctx)?.to_number(self.doc);
                Ok(Value::Number(match op {
                    BinOp::Add => left + right,
                    BinOp::Sub => left - right,
                    BinOp::Mul => left * right,
                    BinOp::Div => left / right,
                    BinOp::Mod => left % right,
                    _ => unreachable!("only arithmetic reaches here"),
                }))
            }
        }
    }

    /// XPath 1.0 section 3.4 comparison rules, including the existential
    /// semantics of `=`/`!=` on node-sets.
    fn compare(&self, op: BinOp, left: &Value, right: &Value) -> Result<bool, XPathError> {
        let equality = matches!(op, BinOp::Eq | BinOp::Ne);
        match (left, right) {
            (Value::Nodes(left), Value::Nodes(right)) if equality => Ok(left.iter().any(|a| {
                right
                    .iter()
                    .any(|b| string_eq(&a.string_value(self.doc), &b.string_value(self.doc), op))
            })),
            (Value::Nodes(left), Value::Nodes(right)) => Ok(left.iter().any(|a| {
                right
                    .iter()
                    .any(|b| numeric_cmp(op, self.number(a), self.number(b)))
            })),
            (Value::Nodes(nodes), other) if equality => Ok(self.compare_nodes_to(op, nodes, other)),
            (other, Value::Nodes(nodes)) if equality => Ok(self.compare_nodes_to(op, nodes, other)),
            (Value::Nodes(nodes), other) => {
                let right = other.to_number(self.doc);
                Ok(nodes
                    .iter()
                    .any(|node| numeric_cmp(op, self.number(node), right)))
            }
            (other, Value::Nodes(nodes)) => {
                let left = other.to_number(self.doc);
                Ok(nodes
                    .iter()
                    .any(|node| numeric_cmp(op, left, self.number(node))))
            }
            _ => Ok(self.compare_atoms(op, left, right)),
        }
    }

    fn compare_nodes_to(&self, op: BinOp, nodes: &[NodeRef], other: &Value) -> bool {
        match other {
            Value::Bool(value) => cmp_bool(op, !nodes.is_empty(), *value),
            Value::Number(value) => nodes
                .iter()
                .any(|node| numeric_cmp(op, self.number(node), *value)),
            Value::Str(value) => nodes
                .iter()
                .any(|node| string_eq(&node.string_value(self.doc), value, op)),
            Value::Nodes(_) => unreachable!("handled by the caller"),
        }
    }

    fn compare_atoms(&self, op: BinOp, left: &Value, right: &Value) -> bool {
        if matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) {
            return numeric_cmp(op, left.to_number(self.doc), right.to_number(self.doc));
        }
        if matches!(left, Value::Bool(_)) || matches!(right, Value::Bool(_)) {
            return cmp_bool(op, left.to_boolean(), right.to_boolean());
        }
        if matches!(left, Value::Number(_)) || matches!(right, Value::Number(_)) {
            return numeric_cmp(op, left.to_number(self.doc), right.to_number(self.doc));
        }
        string_eq(&left.to_string(self.doc), &right.to_string(self.doc), op)
    }

    fn number(&self, node: &NodeRef) -> f64 {
        super::value::parse_number(&node.string_value(self.doc))
    }

    fn call(&mut self, func: Func, args: &[Expr], ctx: &Ctx) -> Result<Value, XPathError> {
        match func {
            Func::Position => {
                require_arity(func, args, 0)?;
                Ok(Value::Number(ctx.position as f64))
            }
            Func::Last => {
                require_arity(func, args, 0)?;
                Ok(Value::Number(ctx.size as f64))
            }
            Func::Count => {
                require_arity(func, args, 1)?;
                Ok(Value::Number(self.nodes(&args[0], ctx)?.len() as f64))
            }
            Func::Not => {
                require_arity(func, args, 1)?;
                let value = self.eval(&args[0], ctx)?;
                Ok(Value::Bool(!value.to_boolean()))
            }
            Func::True => {
                require_arity(func, args, 0)?;
                Ok(Value::Bool(true))
            }
            Func::False => {
                require_arity(func, args, 0)?;
                Ok(Value::Bool(false))
            }
            Func::Name | Func::LocalName => {
                if args.len() > 1 {
                    return Err(arity_error(func, args.len()));
                }
                // HTML lowercases element and attribute names in the DOM, so
                // `name()`/`local-name()` on the same markup is lowercase in
                // browsers; the tape keeps the source spelling.
                if args.is_empty() {
                    return Ok(Value::Str(ctx.node.name(self.doc).to_ascii_lowercase()));
                }
                let nodes = self.nodes(&args[0], ctx)?;
                Ok(Value::Str(match nodes.first() {
                    Some(node) => node.name(self.doc).to_ascii_lowercase(),
                    None => String::new(),
                }))
            }
            Func::String => {
                if args.len() > 1 {
                    return Err(arity_error(func, args.len()));
                }
                if args.is_empty() {
                    return Ok(Value::Str(ctx.node.string_value(self.doc).into_owned()));
                }
                let value = self.eval(&args[0], ctx)?;
                Ok(Value::Str(value.to_string(self.doc)))
            }
            Func::Number => {
                if args.len() > 1 {
                    return Err(arity_error(func, args.len()));
                }
                if args.is_empty() {
                    return Ok(Value::Number(self.number(&ctx.node)));
                }
                let value = self.eval(&args[0], ctx)?;
                Ok(Value::Number(value.to_number(self.doc)))
            }
            Func::Boolean => {
                if args.len() > 1 {
                    return Err(arity_error(func, args.len()));
                }
                if args.is_empty() {
                    return Ok(Value::Bool(ctx.node.kind() != RefKind::Document));
                }
                let value = self.eval(&args[0], ctx)?;
                Ok(Value::Bool(value.to_boolean()))
            }
            Func::Contains | Func::StartsWith => {
                require_arity(func, args, 2)?;
                let haystack = self.eval(&args[0], ctx)?.to_string(self.doc);
                let needle = self.eval(&args[1], ctx)?.to_string(self.doc);
                Ok(Value::Bool(if func == Func::Contains {
                    haystack.contains(&needle)
                } else {
                    haystack.starts_with(&needle)
                }))
            }
            Func::NormalizeSpace => {
                if args.len() > 1 {
                    return Err(arity_error(func, args.len()));
                }
                let text = if args.is_empty() {
                    ctx.node.string_value(self.doc).into_owned()
                } else {
                    self.eval(&args[0], ctx)?.to_string(self.doc)
                };
                Ok(Value::Str(normalize_space(&text)))
            }
            Func::Concat => {
                if args.len() < 2 {
                    return Err(arity_error(func, args.len()));
                }
                let mut out = String::new();
                for arg in args {
                    out.push_str(&self.eval(arg, ctx)?.to_string(self.doc));
                }
                Ok(Value::Str(out))
            }
            Func::Substring => {
                if args.len() < 2 || args.len() > 3 {
                    return Err(arity_error(func, args.len()));
                }
                let text = self.eval(&args[0], ctx)?.to_string(self.doc);
                let start = self.eval(&args[1], ctx)?.to_number(self.doc);
                let length = match args.get(2) {
                    Some(arg) => Some(self.eval(arg, ctx)?.to_number(self.doc)),
                    None => None,
                };
                Ok(Value::Str(substring(&text, start, length)))
            }
            Func::SubstringBefore | Func::SubstringAfter => {
                require_arity(func, args, 2)?;
                let text = self.eval(&args[0], ctx)?.to_string(self.doc);
                let needle = self.eval(&args[1], ctx)?.to_string(self.doc);
                let found = text.find(&needle);
                Ok(Value::Str(match (func, found) {
                    (_, None) => String::new(),
                    (Func::SubstringBefore, Some(index)) => text[..index].to_owned(),
                    (_, Some(index)) => text[index + needle.len()..].to_owned(),
                }))
            }
            Func::StringLength => {
                if args.len() > 1 {
                    return Err(arity_error(func, args.len()));
                }
                let text = if args.is_empty() {
                    ctx.node.string_value(self.doc).into_owned()
                } else {
                    self.eval(&args[0], ctx)?.to_string(self.doc)
                };
                Ok(Value::Number(text.chars().count() as f64))
            }
            Func::Translate => {
                require_arity(func, args, 3)?;
                let text = self.eval(&args[0], ctx)?.to_string(self.doc);
                let from = self.eval(&args[1], ctx)?.to_string(self.doc);
                let to = self.eval(&args[2], ctx)?.to_string(self.doc);
                Ok(Value::Str(translate(&text, &from, &to)))
            }
            Func::Sum => {
                require_arity(func, args, 1)?;
                let nodes = self.nodes(&args[0], ctx)?;
                let mut total = 0.0;
                for node in nodes {
                    total += super::value::parse_number(&node.string_value(self.doc));
                }
                Ok(Value::Number(total))
            }
            Func::Floor | Func::Ceiling | Func::Round => {
                require_arity(func, args, 1)?;
                let value = self.eval(&args[0], ctx)?.to_number(self.doc);
                Ok(Value::Number(match func {
                    Func::Floor => value.floor(),
                    Func::Ceiling => value.ceil(),
                    _ => xpath_round(value),
                }))
            }
            Func::Id => {
                require_arity(func, args, 1)?;
                let tokens: Vec<String> = match self.eval(&args[0], ctx)? {
                    Value::Nodes(nodes) => nodes
                        .iter()
                        .flat_map(|node| split_space(&node.string_value(self.doc)))
                        .collect(),
                    other => split_space(&other.to_string(self.doc)),
                };
                let mut out = Vec::new();
                if !tokens.is_empty() {
                    for id in 0..self.doc.nodes.len() as u32 {
                        let id = NodeId::new(id);
                        if !self.doc.node(id).is_element() {
                            continue;
                        }
                        let Some(attr) = crate::access::find_attr(self.doc, id, "id") else {
                            continue;
                        };
                        let value = crate::access::attr_value(self.doc, attr);
                        if tokens.iter().any(|token| token == value.trim()) {
                            out.push(NodeRef::element(id));
                        }
                    }
                }
                Ok(Value::Nodes(sorted(out)))
            }
            Func::Lang => {
                require_arity(func, args, 1)?;
                let wanted = self
                    .eval(&args[0], ctx)?
                    .to_string(self.doc)
                    .to_ascii_lowercase();
                Ok(Value::Bool(self.lang_matches(ctx.node, &wanted)))
            }
            Func::NamespaceUri => {
                if args.len() > 1 {
                    return Err(arity_error(func, args.len()));
                }
                // The DOM keeps local names only, so no node is in a namespace.
                if !args.is_empty() {
                    let _ = self.eval(&args[0], ctx)?;
                }
                Ok(Value::Str(String::new()))
            }
        }
    }

    /// `lang()`: the nearest ancestor-or-self `xml:lang` decides, per XPath 1.0
    /// (HTML's `lang` attribute is a *different* attribute in the data model, and
    /// libxml2 — hence parsel — behaves the same way).
    fn lang_matches(&self, node: NodeRef, wanted: &str) -> bool {
        let mut current = match node.kind() {
            RefKind::Attribute => Some(node.node_id()),
            RefKind::Document => None,
            _ => Some(node.node_id()),
        };
        while let Some(id) = current {
            let record = self.doc.node(id);
            if record.is_element()
                && let Some(attr) = crate::access::find_attr(self.doc, id, "xml:lang")
            {
                let value = crate::access::attr_value(self.doc, attr).to_ascii_lowercase();
                return value == wanted
                    || (value.len() > wanted.len()
                        && value.starts_with(wanted)
                        && value.as_bytes()[wanted.len()] == b'-');
            }
            if record.kind() == NodeKind::Document {
                break;
            }
            let parent = record.parent();
            current = (parent != NodeId::NONE).then_some(parent);
        }
        false
    }
}

fn require_arity(func: Func, args: &[Expr], expected: usize) -> Result<(), XPathError> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(arity_error(func, args.len()))
    }
}

fn arity_error(func: Func, got: usize) -> XPathError {
    XPathError::Eval(format!("{func:?} called with {got} arguments"))
}

/// Sorts into document order and removes duplicates.
fn sorted(mut nodes: Vec<NodeRef>) -> Vec<NodeRef> {
    nodes.sort_unstable_by_key(|node| node.order_key());
    nodes.dedup();
    nodes
}

/// ASCII-case-insensitive name comparison, since HTML tag and attribute names
/// are case-insensitive but the DOM keeps the author's spelling.
fn name_eq(candidate: &[u8], name: &str) -> bool {
    candidate.len() == name.len() && candidate.eq_ignore_ascii_case(name.as_bytes())
}

fn string_eq(left: &str, right: &str, op: BinOp) -> bool {
    (left == right) == (op == BinOp::Eq)
}

fn cmp_bool(op: BinOp, left: bool, right: bool) -> bool {
    match op {
        BinOp::Eq => left == right,
        BinOp::Ne => left != right,
        _ => false,
    }
}

fn numeric_cmp(op: BinOp, left: f64, right: f64) -> bool {
    match op {
        BinOp::Eq => left == right,
        BinOp::Ne => left != right,
        BinOp::Lt => left < right,
        BinOp::Le => left <= right,
        BinOp::Gt => left > right,
        BinOp::Ge => left >= right,
        _ => false,
    }
}

/// XPath `normalize-space()`: trim and collapse runs of XML whitespace.
/// XPath `round()`: the closest integer, ties toward positive infinity
/// (`round(-2.5)` is `-2`, unlike Rust's `f64::round`).  NaN and the infinities
/// pass through.
fn xpath_round(value: f64) -> f64 {
    if value.is_nan() || value.is_infinite() || value == 0.0 {
        return value;
    }
    (value + 0.5).floor()
}

/// XPath `substring()`: characters at 1-based positions `p` with
/// `p >= round(start)` and `p < round(start) + round(length)`.
fn substring(text: &str, start: f64, length: Option<f64>) -> String {
    let begin = xpath_round(start);
    let end = match length {
        Some(length) => {
            let length = xpath_round(length);
            if length.is_nan() {
                return String::new();
            }
            begin + length
        }
        None => f64::INFINITY,
    };
    text.chars()
        .enumerate()
        .filter(|(index, _)| {
            let position = (*index + 1) as f64;
            position >= begin && position < end
        })
        .map(|(_, character)| character)
        .collect()
}

/// XPath `translate()`: map characters through `from`→`to`, dropping characters
/// whose position in `from` has no counterpart in `to`.
fn translate(text: &str, from: &str, to: &str) -> String {
    let from: Vec<char> = from.chars().collect();
    let to: Vec<char> = to.chars().collect();
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match from.iter().position(|candidate| *candidate == character) {
            Some(index) => {
                if let Some(mapped) = to.get(index) {
                    out.push(*mapped);
                }
            }
            None => out.push(character),
        }
    }
    out
}

/// XPath's whitespace-separated token split (`id()` takes a list of ids).
fn split_space(text: &str) -> Vec<String> {
    text.split([' ', '\t', '\n', '\r'])
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

fn normalize_space(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if matches!(ch, ' ' | '\t' | '\n' | '\r') {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(ch);
        }
    }
    out
}
