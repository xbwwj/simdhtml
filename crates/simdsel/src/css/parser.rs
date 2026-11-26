use crate::error::{CssError, Error, Result};

use super::ast::{
    AttrOp, AttrPred, Combinator, Complex, Compound, Pseudo, PseudoElement, RelativeSelector,
    SelectorList,
};

/// Parses a CSS selector list.
pub fn parse_selector(input: &str) -> Result<SelectorList> {
    parse_selector_kind(input).map_err(Error::Css)
}

fn parse_selector_kind(input: &str) -> Result<SelectorList, CssError> {
    // CSS comments (`div/*x*/.item`) are whitespace-free trivia and can sit
    // between any two tokens.  Removing them up front keeps the parser free of
    // comment handling in every state; error positions then refer to the
    // comment-free selector, which is what a caller can act on anyway.
    let stripped;
    let mut input = input;
    if input.contains("/*") {
        stripped = strip_comments(input)?;
        input = &stripped;
    }
    let mut parser = Parser::new(input);
    let complexes = parser.parse_list(false)?;
    parser.skip_ws();
    if !parser.at_end() {
        return Err(parser.error("unexpected trailing input"));
    }
    if complexes.is_empty() {
        return Err(parser.error("empty selector"));
    }
    Ok(SelectorList { complexes })
}

struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            pos: 0,
        }
    }

    fn error(&self, message: impl Into<String>) -> CssError {
        CssError::Selector {
            message: message.into(),
            position: self.pos,
        }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn skip_ws(&mut self) -> bool {
        let start = self.pos;
        while self.peek().is_some_and(is_ws) {
            self.pos += 1;
        }
        self.pos != start
    }

    fn expect(&mut self, byte: u8) -> Result<(), CssError> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error(format!("expected '{}'", byte as char)))
        }
    }

    fn parse_list(&mut self, in_paren: bool) -> Result<Vec<Complex>, CssError> {
        let mut out = Vec::new();
        loop {
            self.skip_ws();
            out.push(self.parse_complex(in_paren)?);
            self.skip_ws();
            if self.peek() == Some(b',') {
                self.pos += 1;
                continue;
            }
            break;
        }
        Ok(out)
    }

    /// Parses compounds and combinators until `,` / `)` / EOF.
    ///
    /// Whitespace is a descendant combinator unless followed by an explicit
    /// `>`, `+` or `~`; a pseudo-element must be the final component.
    fn parse_complex(&mut self, in_paren: bool) -> Result<Complex, CssError> {
        let mut pseudo = None;
        let mut compounds = vec![self.parse_compound(&mut pseudo)?];
        let mut combinators = Vec::new();

        loop {
            let had_ws = self.skip_ws();
            let next = self.peek();
            if next.is_none() || next == Some(b',') || (in_paren && next == Some(b')')) {
                break;
            }
            if pseudo.is_some() {
                return Err(self.error("pseudo-element must be the last component"));
            }

            let combinator = match next {
                Some(b'>') => {
                    self.pos += 1;
                    Combinator::Child
                }
                Some(b'+') => {
                    self.pos += 1;
                    Combinator::Adjacent
                }
                Some(b'~') => {
                    self.pos += 1;
                    Combinator::GeneralSibling
                }
                Some(_) if had_ws => Combinator::Descendant,
                Some(_) => return Err(self.error("expected a combinator")),
                None => break,
            };
            self.skip_ws();
            combinators.push(combinator);
            compounds.push(self.parse_compound(&mut pseudo)?);
        }

        Ok(Complex {
            compounds,
            combinators,
            pseudo,
        })
    }

    /// Parses one compound (`tag`, `.class`, `#id`, `[attr]`, pseudos).
    fn parse_compound(
        &mut self,
        pseudo_element: &mut Option<PseudoElement>,
    ) -> Result<Compound, CssError> {
        let mut compound = Compound::default();
        self.parse_type_selector(&mut compound)?;

        loop {
            match self.peek() {
                Some(b'.') => {
                    self.pos += 1;
                    compound.classes.push(self.parse_ident()?);
                }
                Some(b'#') => {
                    self.pos += 1;
                    let id = self.parse_ident()?;
                    compound.id = Some(id);
                }
                Some(b'[') => {
                    self.pos += 1;
                    compound.attrs.push(self.parse_attr()?);
                }
                Some(b':') => {
                    if self.input.get(self.pos + 1) == Some(&b':') {
                        self.pos += 2;
                        *pseudo_element = Some(self.parse_pseudo_element()?);
                        break;
                    }
                    self.pos += 1;
                    compound.pseudos.push(self.parse_pseudo()?);
                }
                _ => break,
            }
        }

        if compound.tag.is_none()
            && !compound.universal
            && compound.id.is_none()
            && compound.classes.is_empty()
            && compound.attrs.is_empty()
            && compound.pseudos.is_empty()
        {
            // A pseudo-element with no compound in front of it (`::text`,
            // `A ::text`, `A > ::attr(href)`) attaches to an implied universal
            // selector at that position.
            if pseudo_element.is_some() {
                compound.universal = true;
                compound.implied_universal = true;
            } else {
                return Err(self.error("empty compound selector"));
            }
        }
        Ok(compound)
    }

    /// Parses the type selector, with an optional namespace prefix.
    ///
    /// Namespaces are not modelled (`Document` keeps local names only), so a
    /// prefix is accepted and ignored: `*|*`, `*|div`, `|div` and `div` all
    /// select on the local name.  That is strictly more permissive than
    /// cssselect, which resolves prefixes through the XPath namespace map.
    fn parse_type_selector(&mut self, compound: &mut Compound) -> Result<(), CssError> {
        // `|local`: the "no namespace" prefix.
        if self.peek() == Some(b'|') {
            self.pos += 1;
            return self.parse_local_name(compound);
        }
        let mut star = false;
        let mut prefix = None;
        if self.peek() == Some(b'*') {
            self.pos += 1;
            star = true;
        } else if self.peek().is_some_and(is_ident_start) {
            prefix = Some(self.parse_ident()?);
        } else {
            return Ok(());
        }
        if self.peek() == Some(b'|') {
            self.pos += 1;
            return self.parse_local_name(compound);
        }
        if star {
            compound.universal = true;
        } else {
            compound.tag = prefix;
        }
        Ok(())
    }

    /// The local part of a namespaced type selector (`prefix|*`, `*|div`).
    fn parse_local_name(&mut self, compound: &mut Compound) -> Result<(), CssError> {
        if self.peek() == Some(b'*') {
            self.pos += 1;
            compound.universal = true;
            return Ok(());
        }
        if self.peek().is_some_and(is_ident_start) {
            compound.tag = Some(self.parse_ident()?);
            return Ok(());
        }
        Err(self.error("expected a local name after the namespace prefix"))
    }

    fn parse_ident(&mut self) -> Result<String, CssError> {
        let mut out = Vec::new();
        while let Some(b) = self.peek() {
            if b == b'\\' {
                self.pos += 1;
                if let Some(escaped) = self.peek() {
                    out.push(escaped);
                    self.pos += 1;
                }
            } else if is_ident_byte(b) {
                out.push(b);
                self.pos += 1;
            } else {
                break;
            }
        }
        if out.is_empty() {
            return Err(self.error("expected identifier"));
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    fn parse_attr(&mut self) -> Result<AttrPred, CssError> {
        self.skip_ws();
        let name = self.parse_ident()?.to_ascii_lowercase();
        self.skip_ws();

        let op = match self.peek() {
            Some(b']') => {
                self.pos += 1;
                return Ok(AttrPred {
                    name,
                    op: AttrOp::Exists,
                    value: None,
                    case_insensitive: false,
                });
            }
            Some(b'=') => {
                self.pos += 1;
                AttrOp::Equals
            }
            Some(b'~') => {
                self.pos += 1;
                self.expect(b'=')?;
                AttrOp::Includes
            }
            Some(b'|') => {
                self.pos += 1;
                self.expect(b'=')?;
                AttrOp::DashMatch
            }
            Some(b'^') => {
                self.pos += 1;
                self.expect(b'=')?;
                AttrOp::Prefix
            }
            Some(b'$') => {
                self.pos += 1;
                self.expect(b'=')?;
                AttrOp::Suffix
            }
            Some(b'*') => {
                self.pos += 1;
                self.expect(b'=')?;
                AttrOp::Substring
            }
            Some(b'!') => {
                self.pos += 1;
                self.expect(b'=')?;
                AttrOp::NotEquals
            }
            _ => return Err(self.error("expected attribute operator")),
        };

        self.skip_ws();
        let value = match self.peek() {
            Some(q @ (b'"' | b'\'')) => {
                self.pos += 1;
                self.parse_string(q)?
            }
            _ => self.parse_ident()?,
        };
        self.skip_ws();

        let mut case_insensitive = false;
        if self.peek().is_some_and(|b| b == b'i' || b == b'I') {
            self.pos += 1;
            case_insensitive = true;
            self.skip_ws();
        } else if self.peek().is_some_and(|b| b == b's' || b == b'S') {
            self.pos += 1;
            self.skip_ws();
        }

        self.expect(b']')?;
        Ok(AttrPred {
            name,
            op,
            value: Some(value),
            case_insensitive,
        })
    }

    fn parse_string(&mut self, quote: u8) -> Result<String, CssError> {
        let mut out = Vec::new();
        while let Some(b) = self.peek() {
            self.pos += 1;
            if b == quote {
                return Ok(String::from_utf8_lossy(&out).into_owned());
            }
            if b == b'\\' {
                if let Some(escaped) = self.peek() {
                    self.pos += 1;
                    out.push(escaped);
                }
            } else {
                out.push(b);
            }
        }
        Err(self.error("unterminated string"))
    }

    fn parse_pseudo(&mut self) -> Result<Pseudo, CssError> {
        let name = self.parse_ident()?.to_ascii_lowercase();
        match name.as_str() {
            "first-child" => Ok(Pseudo::FirstChild),
            "last-child" => Ok(Pseudo::LastChild),
            "only-child" => Ok(Pseudo::OnlyChild),
            "first-of-type" => Ok(Pseudo::FirstOfType),
            "last-of-type" => Ok(Pseudo::LastOfType),
            "only-of-type" => Ok(Pseudo::OnlyOfType),
            "link" | "any-link" => Ok(Pseudo::Link),
            "checked" => Ok(Pseudo::Checked),
            "enabled" => Ok(Pseudo::Enabled),
            "disabled" => Ok(Pseudo::Disabled),
            "lang" => {
                self.expect(b'(')?;
                self.skip_ws();
                let value = match self.peek() {
                    Some(q @ (b'"' | b'\'')) => {
                        self.pos += 1;
                        self.parse_string(q)?
                    }
                    _ => self.parse_ident()?,
                };
                self.skip_ws();
                self.expect(b')')?;
                Ok(Pseudo::Lang(value.to_ascii_lowercase()))
            }
            "root" => Ok(Pseudo::Root),
            "empty" => Ok(Pseudo::Empty),
            "scope" => Ok(Pseudo::Scope),
            "contains" => {
                self.expect(b'(')?;
                self.skip_ws();
                let value = match self.peek() {
                    Some(q @ (b'"' | b'\'')) => {
                        self.pos += 1;
                        self.parse_string(q)?
                    }
                    _ => self.parse_ident()?,
                };
                self.skip_ws();
                self.expect(b')')?;
                Ok(Pseudo::Contains(value))
            }
            "hover" | "focus" | "active" | "visited" | "target" | "focus-within"
            | "focus-visible" => Ok(Pseudo::Never),
            "not" => Ok(Pseudo::Not(self.parse_nested_list()?)),
            "is" | "where" => Ok(Pseudo::Is(self.parse_nested_list()?)),
            "has" => Ok(Pseudo::Has(self.parse_relative_list()?)),
            "nth-child" => {
                let (a, b) = self.parse_an_plus_b()?;
                Ok(Pseudo::NthChild(a, b))
            }
            "nth-last-child" => {
                let (a, b) = self.parse_an_plus_b()?;
                Ok(Pseudo::NthLastChild(a, b))
            }
            "nth-of-type" => {
                let (a, b) = self.parse_an_plus_b()?;
                Ok(Pseudo::NthOfType(a, b))
            }
            "nth-last-of-type" => {
                let (a, b) = self.parse_an_plus_b()?;
                Ok(Pseudo::NthLastOfType(a, b))
            }
            _ => Err(self.error(format!("unsupported pseudo-class :{name}"))),
        }
    }

    fn parse_nested_list(&mut self) -> Result<Vec<Complex>, CssError> {
        self.expect(b'(')?;
        let list = self.parse_list(true)?;
        self.expect(b')')?;
        Ok(list)
    }

    fn parse_relative_list(&mut self) -> Result<Vec<RelativeSelector>, CssError> {
        self.expect(b'(')?;
        let mut out = Vec::new();
        loop {
            self.skip_ws();
            let relation = match self.peek() {
                Some(b'>') => {
                    self.pos += 1;
                    Combinator::Child
                }
                Some(b'+') => {
                    self.pos += 1;
                    Combinator::Adjacent
                }
                Some(b'~') => {
                    self.pos += 1;
                    Combinator::GeneralSibling
                }
                _ => Combinator::Descendant,
            };
            self.skip_ws();
            out.push(RelativeSelector {
                relation,
                complex: self.parse_complex(true)?,
            });
            self.skip_ws();
            if self.peek() == Some(b',') {
                self.pos += 1;
                continue;
            }
            break;
        }
        self.expect(b')')?;
        Ok(out)
    }

    fn parse_an_plus_b(&mut self) -> Result<(i32, i32), CssError> {
        self.expect(b'(')?;
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b == b')' {
                break;
            }
            self.pos += 1;
        }
        let raw = std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| self.error("invalid nth argument"))?;
        self.expect(b')')?;
        parse_an_plus_b_str(raw).map_err(|message| CssError::Selector {
            message,
            position: start,
        })
    }

    fn parse_pseudo_element(&mut self) -> Result<PseudoElement, CssError> {
        let name = self.parse_ident()?.to_ascii_lowercase();
        match name.as_str() {
            "text" => Ok(PseudoElement::Text),
            "attr" => {
                self.expect(b'(')?;
                self.skip_ws();
                let value = match self.peek() {
                    Some(q @ (b'"' | b'\'')) => {
                        self.pos += 1;
                        self.parse_string(q)?
                    }
                    _ => self.parse_ident()?,
                };
                self.skip_ws();
                self.expect(b')')?;
                Ok(PseudoElement::Attr(value))
            }
            _ => Err(self.error(format!("unsupported pseudo-element ::{name}"))),
        }
    }
}

/// Parses the CSS `An+B` grammar used by `:nth-*()`.
fn parse_an_plus_b_str(raw: &str) -> Result<(i32, i32), String> {
    let s: String = raw
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    if s.is_empty() {
        return Err("empty nth argument".to_string());
    }
    if s == "odd" {
        return Ok((2, 1));
    }
    if s == "even" {
        return Ok((2, 0));
    }

    if let Some(n_pos) = s.find('n') {
        let a_part = &s[..n_pos];
        let a = match a_part {
            "" | "+" => 1,
            "-" => -1,
            _ => a_part
                .parse::<i32>()
                .map_err(|_| "invalid nth coefficient".to_string())?,
        };
        let b_part = &s[n_pos + 1..];
        let b = if b_part.is_empty() {
            0
        } else {
            b_part
                .parse::<i32>()
                .map_err(|_| "invalid nth offset".to_string())?
        };
        Ok((a, b))
    } else {
        let b = s
            .parse::<i32>()
            .map_err(|_| "invalid nth argument".to_string())?;
        Ok((0, b))
    }
}

/// Removes `/* ... */` comments.  An unterminated comment is a syntax error.
fn strip_comments(input: &str) -> Result<String, CssError> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    let mut consumed = 0;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("*/") else {
            return Err(CssError::Selector {
                message: "unterminated comment".to_string(),
                position: consumed + start,
            });
        };
        consumed += start + 2 + end + 2;
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0C' | b'\r')
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'-' || b >= 0x80 || b == b'\\'
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b >= 0x80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_selectors() {
        let s = parse_selector("div.foo#bar[data-x='y' i] > span + a ~ p:first-child").unwrap();
        assert_eq!(s.complexes.len(), 1);
        let c = &s.complexes[0];
        assert_eq!(c.compounds.len(), 4);
        assert_eq!(
            c.combinators,
            vec![
                Combinator::Child,
                Combinator::Adjacent,
                Combinator::GeneralSibling
            ]
        );
        assert_eq!(c.compounds[0].tag.as_deref(), Some("div"));
        assert_eq!(c.compounds[0].classes, vec!["foo"]);
        assert_eq!(c.compounds[0].id.as_deref(), Some("bar"));
        assert!(c.compounds[0].attrs[0].case_insensitive);
    }

    #[test]
    fn parses_universal() {
        let s = parse_selector("*").unwrap();
        assert!(s.complexes[0].compounds[0].universal);
        let s = parse_selector("*.foo").unwrap();
        assert!(s.complexes[0].compounds[0].universal);
        assert_eq!(s.complexes[0].compounds[0].classes, vec!["foo"]);
    }

    #[test]
    fn parses_nth() {
        let s = parse_selector("li:nth-child(2n+1)").unwrap();
        assert_eq!(
            s.complexes[0].compounds[0].pseudos,
            vec![Pseudo::NthChild(2, 1)]
        );
        let s = parse_selector("li:nth-child(odd)").unwrap();
        assert_eq!(
            s.complexes[0].compounds[0].pseudos,
            vec![Pseudo::NthChild(2, 1)]
        );
        let s = parse_selector("li:nth-child(-n+3)").unwrap();
        assert_eq!(
            s.complexes[0].compounds[0].pseudos,
            vec![Pseudo::NthChild(-1, 3)]
        );
    }

    #[test]
    fn parses_pseudo_elements() {
        let s = parse_selector("a.link::attr(href)").unwrap();
        assert_eq!(
            s.complexes[0].pseudo,
            Some(PseudoElement::Attr("href".into()))
        );
        let s = parse_selector("p::text").unwrap();
        assert_eq!(s.complexes[0].pseudo, Some(PseudoElement::Text));
    }

    #[test]
    fn parses_has_relative() {
        let s = parse_selector("div:has(> a.foo, ~ p)").unwrap();
        let Pseudo::Has(rels) = &s.complexes[0].compounds[0].pseudos[0] else {
            panic!();
        };
        assert_eq!(rels.len(), 2);
        assert_eq!(rels[0].relation, Combinator::Child);
        assert_eq!(rels[1].relation, Combinator::GeneralSibling);
    }

    #[test]
    fn rejects_bad_selector() {
        assert!(parse_selector("div >").is_err());
        assert!(parse_selector("[a=").is_err());
        assert!(parse_selector(":unknown").is_err());
        assert!(parse_selector("div::bogus").is_err());
    }
}
