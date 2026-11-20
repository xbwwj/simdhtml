//! XPath 1.0 recursive-descent parser (grammar section 3.7), core subset.

use super::XPathError;
use super::ast::{Axis, BinOp, Expr, Func, NodeTest, Path, Step};

/// Tokens.
#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Name(String),
    Num(f64),
    Str(String),
    Slash,
    DSlash,
    Dot,
    DDot,
    At,
    Star,
    DColon,
    Comma,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Pipe,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Plus,
    Minus,
    Eof,
}

struct Lexer<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            pos: 0,
        }
    }

    fn tokens(mut self) -> Result<Vec<(Tok, usize)>, XPathError> {
        let mut out = Vec::new();
        loop {
            let start = self.pos;
            let Some(&byte) = self.input.get(self.pos) else {
                out.push((Tok::Eof, self.pos));
                return Ok(out);
            };
            match byte {
                b' ' | b'\t' | b'\n' | b'\r' => {
                    self.pos += 1;
                }
                b'/' => {
                    self.pos += 1;
                    if self.input.get(self.pos) == Some(&b'/') {
                        self.pos += 1;
                        out.push((Tok::DSlash, start));
                    } else {
                        out.push((Tok::Slash, start));
                    }
                }
                b'.' => {
                    if self.input.get(self.pos + 1) == Some(&b'.') {
                        self.pos += 2;
                        out.push((Tok::DDot, start));
                    } else if self.input.get(self.pos + 1).is_some_and(u8::is_ascii_digit) {
                        out.push((self.number()?, start));
                    } else {
                        self.pos += 1;
                        out.push((Tok::Dot, start));
                    }
                }
                b'0'..=b'9' => out.push((self.number()?, start)),
                b'@' => {
                    self.pos += 1;
                    out.push((Tok::At, start));
                }
                b'*' => {
                    self.pos += 1;
                    out.push((Tok::Star, start));
                }
                b',' => {
                    self.pos += 1;
                    out.push((Tok::Comma, start));
                }
                b'(' => {
                    self.pos += 1;
                    out.push((Tok::LParen, start));
                }
                b')' => {
                    self.pos += 1;
                    out.push((Tok::RParen, start));
                }
                b'[' => {
                    self.pos += 1;
                    out.push((Tok::LBracket, start));
                }
                b']' => {
                    self.pos += 1;
                    out.push((Tok::RBracket, start));
                }
                b'|' => {
                    self.pos += 1;
                    out.push((Tok::Pipe, start));
                }
                b'+' => {
                    self.pos += 1;
                    out.push((Tok::Plus, start));
                }
                b'-' => {
                    self.pos += 1;
                    out.push((Tok::Minus, start));
                }
                b'=' => {
                    self.pos += 1;
                    out.push((Tok::Eq, start));
                }
                b'!' => {
                    if self.input.get(self.pos + 1) == Some(&b'=') {
                        self.pos += 2;
                        out.push((Tok::Ne, start));
                    } else {
                        return Err(self.error(start, "'!' must start '!='"));
                    }
                }
                b'<' => {
                    self.pos += 1;
                    if self.input.get(self.pos) == Some(&b'=') {
                        self.pos += 1;
                        out.push((Tok::Le, start));
                    } else {
                        out.push((Tok::Lt, start));
                    }
                }
                b'>' => {
                    self.pos += 1;
                    if self.input.get(self.pos) == Some(&b'=') {
                        self.pos += 1;
                        out.push((Tok::Ge, start));
                    } else {
                        out.push((Tok::Gt, start));
                    }
                }
                b':' => {
                    if self.input.get(self.pos + 1) == Some(&b':') {
                        self.pos += 2;
                        out.push((Tok::DColon, start));
                    } else {
                        return Err(XPathError::Unsupported(
                            "namespace prefixes are not supported",
                        ));
                    }
                }
                b'\'' | b'"' => {
                    let quote = byte;
                    self.pos += 1;
                    let begin = self.pos;
                    let mut closed = false;
                    while let Some(&b) = self.input.get(self.pos) {
                        self.pos += 1;
                        if b == quote {
                            closed = true;
                            break;
                        }
                    }
                    if !closed {
                        return Err(self.error(start, "unterminated string literal"));
                    }
                    let text = std::str::from_utf8(&self.input[begin..self.pos - 1])
                        .expect("expression input is UTF-8")
                        .to_owned();
                    out.push((Tok::Str(text), start));
                }
                _ if is_name_start(byte) => {
                    let begin = self.pos;
                    while self.input.get(self.pos).is_some_and(|&b| is_name_byte(b)) {
                        self.pos += 1;
                    }
                    let name = std::str::from_utf8(&self.input[begin..self.pos])
                        .expect("expression input is UTF-8")
                        .to_owned();
                    out.push((Tok::Name(name), start));
                }
                _ => {
                    return Err(self.error(start, &format!("unexpected byte {byte:#04x}")));
                }
            }
        }
    }

    fn number(&mut self) -> Result<Tok, XPathError> {
        let begin = self.pos;
        while self.input.get(self.pos).is_some_and(u8::is_ascii_digit) {
            self.pos += 1;
        }
        if self.input.get(self.pos) == Some(&b'.') {
            self.pos += 1;
            while self.input.get(self.pos).is_some_and(u8::is_ascii_digit) {
                self.pos += 1;
            }
        }
        let text =
            std::str::from_utf8(&self.input[begin..self.pos]).expect("expression input is UTF-8");
        match text.parse() {
            Ok(value) => Ok(Tok::Num(value)),
            Err(_) => Err(self.error(begin, "invalid number")),
        }
    }

    fn error(&self, position: usize, message: &str) -> XPathError {
        XPathError::Syntax {
            position,
            message: message.to_owned(),
        }
    }
}

fn is_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
}

/// Has a node test at the current position begun?
fn is_node_test_name(name: &str) -> bool {
    matches!(name, "text" | "node" | "comment" | "processing-instruction")
}

struct Parser {
    toks: Vec<(Tok, usize)>,
    i: usize,
}

/// Parses `input` into an [`Expr`].
pub(super) fn parse(input: &str) -> Result<Expr, XPathError> {
    let toks = Lexer::new(input).tokens()?;
    if toks.len() == 1 {
        return Err(XPathError::Syntax {
            position: 0,
            message: "empty expression".to_owned(),
        });
    }
    let mut parser = Parser { toks, i: 0 };
    let expr = parser.expr()?;
    if parser.peek() != &Tok::Eof {
        return Err(parser.error("trailing input"));
    }
    Ok(expr)
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.i].0
    }

    fn peek_at(&self, offset: usize) -> &Tok {
        match self.toks.get(self.i + offset) {
            Some((tok, _)) => tok,
            None => &Tok::Eof,
        }
    }

    fn bump(&mut self) -> Tok {
        let tok = self.toks[self.i].0.clone();
        if self.i + 1 < self.toks.len() {
            self.i += 1;
        }
        tok
    }

    fn position(&self) -> usize {
        self.toks[self.i].1
    }

    fn error(&self, message: &str) -> XPathError {
        XPathError::Syntax {
            position: self.position(),
            message: message.to_owned(),
        }
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        if self.peek() == tok {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, tok: &Tok) -> Result<(), XPathError> {
        if self.eat(tok) {
            Ok(())
        } else {
            Err(self.error(&format!("expected {tok:?}, found {:?}", self.peek())))
        }
    }

    fn keyword(&self, word: &str) -> bool {
        matches!(self.peek(), Tok::Name(name) if name == word)
    }

    // Expr := OrExpr
    fn expr(&mut self) -> Result<Expr, XPathError> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> Result<Expr, XPathError> {
        let mut expr = self.and_expr()?;
        while self.keyword("or") {
            self.bump();
            let rhs = self.and_expr()?;
            expr = Expr::Binary(BinOp::Or, Box::new(expr), Box::new(rhs));
        }
        Ok(expr)
    }

    fn and_expr(&mut self) -> Result<Expr, XPathError> {
        let mut expr = self.comparison_expr()?;
        while self.keyword("and") {
            self.bump();
            let rhs = self.comparison_expr()?;
            expr = Expr::Binary(BinOp::And, Box::new(expr), Box::new(rhs));
        }
        Ok(expr)
    }

    fn comparison_expr(&mut self) -> Result<Expr, XPathError> {
        let mut expr = self.additive_expr()?;
        loop {
            let op = match self.peek() {
                Tok::Eq => BinOp::Eq,
                Tok::Ne => BinOp::Ne,
                Tok::Lt => BinOp::Lt,
                Tok::Le => BinOp::Le,
                Tok::Gt => BinOp::Gt,
                Tok::Ge => BinOp::Ge,
                _ => return Ok(expr),
            };
            self.bump();
            let rhs = self.additive_expr()?;
            expr = Expr::Binary(op, Box::new(expr), Box::new(rhs));
        }
    }

    fn additive_expr(&mut self) -> Result<Expr, XPathError> {
        let mut expr = self.multiplicative_expr()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => return Ok(expr),
            };
            self.bump();
            let rhs = self.multiplicative_expr()?;
            expr = Expr::Binary(op, Box::new(expr), Box::new(rhs));
        }
    }

    fn multiplicative_expr(&mut self) -> Result<Expr, XPathError> {
        let mut expr = self.unary_expr()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Name(name) if name == "div" => BinOp::Div,
                Tok::Name(name) if name == "mod" => BinOp::Mod,
                _ => return Ok(expr),
            };
            self.bump();
            let rhs = self.unary_expr()?;
            expr = Expr::Binary(op, Box::new(expr), Box::new(rhs));
        }
    }

    fn unary_expr(&mut self) -> Result<Expr, XPathError> {
        if self.eat(&Tok::Minus) {
            let operand = self.unary_expr()?;
            return Ok(Expr::Negate(Box::new(operand)));
        }
        self.union_expr()
    }

    fn union_expr(&mut self) -> Result<Expr, XPathError> {
        let mut expr = self.path_expr()?;
        while self.eat(&Tok::Pipe) {
            let rhs = self.path_expr()?;
            expr = Expr::Union(Box::new(expr), Box::new(rhs));
        }
        Ok(expr)
    }

    fn starts_step(&self) -> bool {
        match self.peek() {
            Tok::Dot | Tok::DDot | Tok::At | Tok::Star => true,
            Tok::Name(name) => {
                if self.peek_at(1) == &Tok::LParen {
                    is_node_test_name(name)
                } else {
                    true
                }
            }
            _ => false,
        }
    }

    fn path_expr(&mut self) -> Result<Expr, XPathError> {
        if self.eat(&Tok::Slash) {
            let steps = if self.starts_step() {
                self.relative_path()?
            } else {
                Vec::new()
            };
            return Ok(Expr::Path(Path {
                base: None,
                absolute: true,
                steps,
            }));
        }
        if self.eat(&Tok::DSlash) {
            let mut steps = vec![descendant_or_self_node()];
            steps.extend(self.relative_path()?);
            return Ok(Expr::Path(Path {
                base: None,
                absolute: true,
                steps,
            }));
        }
        if self.starts_step() {
            return Ok(Expr::Path(Path {
                base: None,
                absolute: false,
                steps: self.relative_path()?,
            }));
        }

        let primary = self.primary()?;
        let predicates = self.predicates()?;
        let expr = if predicates.is_empty() {
            primary
        } else {
            Expr::Filter {
                primary: Box::new(primary),
                predicates,
            }
        };
        if self.peek() == &Tok::Slash || self.peek() == &Tok::DSlash {
            let mut steps = Vec::new();
            while let Tok::Slash | Tok::DSlash = self.peek() {
                let double = self.peek() == &Tok::DSlash;
                self.bump();
                if double {
                    steps.push(descendant_or_self_node());
                }
                steps.push(self.step()?);
            }
            return Ok(Expr::Path(Path {
                base: Some(Box::new(expr)),
                absolute: false,
                steps,
            }));
        }
        Ok(expr)
    }

    fn relative_path(&mut self) -> Result<Vec<Step>, XPathError> {
        let mut steps = vec![self.step()?];
        loop {
            let double = match self.peek() {
                Tok::Slash => false,
                Tok::DSlash => true,
                _ => return Ok(steps),
            };
            self.bump();
            if double {
                steps.push(descendant_or_self_node());
            }
            steps.push(self.step()?);
        }
    }

    fn step(&mut self) -> Result<Step, XPathError> {
        if self.eat(&Tok::Dot) {
            return Ok(Step {
                axis: Axis::Self_,
                test: NodeTest::Node,
                predicates: Vec::new(),
            });
        }
        if self.eat(&Tok::DDot) {
            return Ok(Step {
                axis: Axis::Parent,
                test: NodeTest::Node,
                predicates: Vec::new(),
            });
        }

        let axis = if self.eat(&Tok::At) {
            Axis::Attribute
        } else if let Tok::Name(name) = self.peek().clone() {
            if self.peek_at(1) == &Tok::DColon {
                self.bump();
                self.bump();
                axis_from_name(&name)?
            } else {
                Axis::Child
            }
        } else {
            Axis::Child
        };

        let test = self.node_test()?;
        let predicates = self.predicates()?;
        Ok(Step {
            axis,
            test,
            predicates,
        })
    }

    fn node_test(&mut self) -> Result<NodeTest, XPathError> {
        if self.eat(&Tok::Star) {
            return Ok(NodeTest::Any);
        }
        let Tok::Name(name) = self.peek().clone() else {
            return Err(self.error("expected a node test"));
        };
        self.bump();
        if self.eat(&Tok::LParen) {
            self.expect(&Tok::RParen)?;
            return Ok(match name.as_str() {
                "text" => NodeTest::Text,
                "node" => NodeTest::Node,
                "comment" | "processing-instruction" => {
                    return Err(XPathError::Unsupported(
                        "comment()/processing-instruction() node tests",
                    ));
                }
                _ => return Err(self.error("expected a node test function")),
            });
        }
        Ok(NodeTest::Name(name))
    }

    fn predicates(&mut self) -> Result<Vec<Expr>, XPathError> {
        let mut out = Vec::new();
        while self.eat(&Tok::LBracket) {
            out.push(self.expr()?);
            self.expect(&Tok::RBracket)?;
        }
        Ok(out)
    }

    fn primary(&mut self) -> Result<Expr, XPathError> {
        match self.peek().clone() {
            Tok::Str(text) => {
                self.bump();
                Ok(Expr::Literal(text))
            }
            Tok::Num(value) => {
                self.bump();
                Ok(Expr::Number(value))
            }
            Tok::LParen => {
                self.bump();
                let inner = self.expr()?;
                self.expect(&Tok::RParen)?;
                Ok(inner)
            }
            Tok::Name(name) => {
                if self.peek_at(1) != &Tok::LParen {
                    return Err(self.error(&format!("unexpected name '{name}'")));
                }
                self.bump();
                self.bump();
                let mut args = Vec::new();
                if !self.eat(&Tok::RParen) {
                    loop {
                        args.push(self.expr()?);
                        if self.eat(&Tok::Comma) {
                            continue;
                        }
                        self.expect(&Tok::RParen)?;
                        break;
                    }
                }
                match Func::lookup(&name) {
                    Some(func) => Ok(Expr::Call(func, args)),
                    None => Err(XPathError::Unsupported("unknown function")),
                }
            }
            other => Err(self.error(&format!("unexpected {other:?}"))),
        }
    }
}

fn axis_from_name(name: &str) -> Result<Axis, XPathError> {
    Ok(match name {
        "child" => Axis::Child,
        "descendant" => Axis::Descendant,
        "descendant-or-self" => Axis::DescendantOrSelf,
        "self" => Axis::Self_,
        "parent" => Axis::Parent,
        "ancestor" => Axis::Ancestor,
        "attribute" => Axis::Attribute,
        "following-sibling" => Axis::FollowingSibling,
        "preceding-sibling" => Axis::PrecedingSibling,
        "following" | "preceding" => {
            return Err(XPathError::Unsupported(
                "the following/preceding axes are not implemented yet",
            ));
        }
        _ => return Err(XPathError::Unsupported("unknown axis")),
    })
}

fn descendant_or_self_node() -> Step {
    Step {
        axis: Axis::DescendantOrSelf,
        test: NodeTest::Node,
        predicates: Vec::new(),
    }
}
