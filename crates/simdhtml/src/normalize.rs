//! Normalizes the flat token stream into a well-nested event stream.
//!
//! HTML allows void elements and optional end tags.  The normalizer turns both
//! into synthetic `End` events so the flat-document builder only ever sees a
//! Dyck-like start/end stream.

use std::ops::Range;

use crate::{DocFlags, Warning, WarningKind};

use crate::tagcode::{TagCode, closes_p, is_void};
use crate::tokenizer::{TextRun, Tokenized};

/// A tag identity in the normalizer stack.
///
/// Real tags are identified by their token index; table wrappers are static
/// synthetic names. No global tag-name interning is involved.  The tokenizer
/// additionally stores one [`TagCode`] per token, which is what the implied-end
/// rules switch on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TagRef {
    Real(u32),
    Synthetic(&'static str),
}

/// An event after normalization (materialized by tests only).
#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Event {
    Start {
        token: u32,
    },
    StartSynthetic {
        name: &'static str,
        span: Range<u32>,
    },
    End {
        tag: TagRef,
        span: Range<u32>,
        synthetic: bool,
    },
    Text {
        run: TextRun,
    },
}

/// Direct consumer of the normalized stream.
///
/// The document builder implements this and creates nodes in the same pass;
/// the event enum is only materialized by tests that inspect the stream.
pub(crate) trait EventSink {
    fn start(&mut self, token: u32, span: Range<u32>);
    fn start_synthetic(&mut self, name: &'static str, span: Range<u32>);
    fn end(&mut self, tag: TagRef, span: Range<u32>, synthetic: bool);
    fn text(&mut self, run: TextRun);
}

#[cfg(test)]
impl EventSink for Vec<Event> {
    fn start(&mut self, token: u32, _span: Range<u32>) {
        self.push(Event::Start { token });
    }
    fn start_synthetic(&mut self, name: &'static str, span: Range<u32>) {
        self.push(Event::StartSynthetic { name, span });
    }
    fn end(&mut self, tag: TagRef, span: Range<u32>, synthetic: bool) {
        self.push(Event::End {
            tag,
            span,
            synthetic,
        });
    }
    fn text(&mut self, run: TextRun) {
        self.push(Event::Text { run });
    }
}

pub(crate) fn tag_name<'a>(input: &'a [u8], tokens: &Tokenized, tag: TagRef) -> &'a str {
    match tag {
        TagRef::Real(i) => {
            let i = i as usize;
            let start = tokens.tag_name_start[i] as usize;
            let len = tokens.tag_name_len[i] as usize;
            // SAFETY: the whole input was validated as UTF-8, and tag names are
            // ASCII-delimited slices of it.
            unsafe { std::str::from_utf8_unchecked(&input[start..start + len]) }
        }
        TagRef::Synthetic(name) => name,
    }
}

#[inline]
pub(crate) fn tag_code(tokens: &Tokenized, tag: TagRef) -> TagCode {
    match tag {
        TagRef::Real(i) => tokens.tag_code[i as usize],
        TagRef::Synthetic(name) => TagCode::of(name.as_bytes()),
    }
}

/// Whether two stack entries are the same tag name.
///
/// A dedicated [`TagCode`] is enough; only unknown names need raw span
/// comparison.
pub(crate) fn name_eq(input: &[u8], tokens: &Tokenized, a: TagRef, b: TagRef) -> bool {
    let left_code = tag_code(tokens, a);
    let right_code = tag_code(tokens, b);
    if left_code != TagCode::Other || right_code != TagCode::Other {
        return left_code == right_code;
    }
    let left = tag_name(input, tokens, a);
    let right = tag_name(input, tokens, b);
    left.len() == right.len() && left.eq_ignore_ascii_case(right)
}

#[cfg(test)]
pub(crate) struct Normalized {
    pub events: Vec<Event>,
    pub flags: DocFlags,
    pub warnings: Vec<Warning>,
}

#[cfg(test)]
pub(crate) fn normalize(
    input: &[u8],
    tokens: &Tokenized,
    input_len: u32,
    text_runs: impl Iterator<Item = TextRun>,
) -> Normalized {
    let mut events = Vec::with_capacity(tokens.tags.len() * 2 + 16);
    let (flags, warnings) = normalize_with(input, tokens, input_len, text_runs, &mut events);
    Normalized {
        events,
        flags,
        warnings,
    }
}

pub(crate) fn normalize_with(
    input: &[u8],
    tokens: &Tokenized,
    input_len: u32,
    text_runs: impl Iterator<Item = TextRun>,
    sink: &mut impl EventSink,
) -> (DocFlags, Vec<Warning>) {
    let mut warnings = Vec::new();
    let mut flags = DocFlags::empty();
    let mut stack: Vec<TagRef> = Vec::new();
    let mut texts = text_runs.peekable();

    let mut ti = 0usize;
    while ti < tokens.tags.len() || texts.peek().is_some() {
        let take_tag = match texts.peek() {
            None => true,
            Some(run) => ti < tokens.tags.len() && tokens.tags[ti].span.start <= run.span.start,
        };

        if take_tag {
            let token = &tokens.tags[ti];
            let tag = TagRef::Real(ti as u32);
            let code = tokens.tag_code[ti];
            match token.kind {
                crate::TagKind::Start => {
                    close_implied(tokens, &mut stack, code, token.span.start, sink, &mut flags);
                    insert_table_wrappers(tokens, &mut stack, code, token.span.start, sink);
                    sink.start(ti as u32, token.span.clone());

                    if is_void(code) {
                        sink.end(tag, token.span.end..token.span.end, true);
                        flags |= DocFlags::SYNTHETIC_END;
                    } else {
                        stack.push(tag);
                    }
                }
                crate::TagKind::End => {
                    let pos = stack
                        .iter()
                        .rposition(|&open| name_eq(input, tokens, open, tag));
                    if let Some(pos) = pos {
                        for &open in stack[pos + 1..].iter().rev() {
                            sink.end(open, token.span.start..token.span.start, true);
                            flags |= DocFlags::SYNTHETIC_END;
                        }
                        sink.end(stack[pos], token.span.clone(), false);
                        stack.truncate(pos);
                    } else {
                        flags |= DocFlags::MISNESTED;
                        warnings.push(Warning {
                            kind: WarningKind::UnmatchedEndTag,
                            span: token.span.clone(),
                        });
                    }
                }
            }
            ti += 1;
        } else {
            let run = texts.next().expect("peeked");
            sink.text(run);
        }
    }

    for &tag in stack.iter().rev() {
        sink.end(tag, input_len..input_len, true);
        flags |= DocFlags::SYNTHETIC_END;
    }

    (flags, warnings)
}

mod table;

use table::insert_table_wrappers;

const P_SET: &[TagCode] = &[TagCode::P];
const P_SCOPE_BOUNDARIES: &[TagCode] = &[
    TagCode::Button,
    TagCode::Table,
    TagCode::Td,
    TagCode::Th,
    TagCode::Marquee,
    TagCode::Object,
    TagCode::Applet,
    TagCode::Template,
];

fn close_implied(
    tokens: &Tokenized,
    stack: &mut Vec<TagRef>,
    incoming: TagCode,
    position: u32,
    sink: &mut impl EventSink,
    flags: &mut DocFlags,
) {
    if closes_p(incoming)
        && let Some(target) = find_last(stack, tokens, P_SET, P_SCOPE_BOUNDARIES)
    {
        pop_through(stack, target, position, sink, flags);
    }

    let target = match incoming {
        TagCode::Li => find_last(
            stack,
            tokens,
            &[TagCode::Li],
            &[TagCode::Ul, TagCode::Ol, TagCode::Menu],
        ),
        TagCode::Dt | TagCode::Dd => {
            find_last(stack, tokens, &[TagCode::Dt, TagCode::Dd], &[TagCode::Dl])
        }
        TagCode::Option => find_last(
            stack,
            tokens,
            &[TagCode::Option],
            &[TagCode::Select, TagCode::Datalist, TagCode::Optgroup],
        ),
        TagCode::Optgroup => find_last(
            stack,
            tokens,
            &[TagCode::Option, TagCode::Optgroup],
            &[TagCode::Select, TagCode::Datalist],
        ),
        TagCode::Tr => find_last(
            stack,
            tokens,
            &[TagCode::Tr, TagCode::Colgroup],
            &[
                TagCode::Table,
                TagCode::Thead,
                TagCode::Tbody,
                TagCode::Tfoot,
            ],
        )
        .or_else(|| {
            find_last(
                stack,
                tokens,
                &[TagCode::Td, TagCode::Th],
                &[TagCode::Table],
            )
        }),
        TagCode::Td | TagCode::Th => find_last(
            stack,
            tokens,
            &[TagCode::Td, TagCode::Th],
            &[
                TagCode::Table,
                TagCode::Tr,
                TagCode::Thead,
                TagCode::Tbody,
                TagCode::Tfoot,
            ],
        ),
        TagCode::Thead | TagCode::Tbody | TagCode::Tfoot => find_last(
            stack,
            tokens,
            &[
                TagCode::Td,
                TagCode::Th,
                TagCode::Tr,
                TagCode::Thead,
                TagCode::Tbody,
                TagCode::Tfoot,
            ],
            &[TagCode::Table],
        ),
        TagCode::Colgroup => find_last(stack, tokens, &[TagCode::Colgroup], &[TagCode::Table]),
        TagCode::Rt | TagCode::Rp => {
            find_last(stack, tokens, &[TagCode::Rt, TagCode::Rp], &[TagCode::Ruby])
        }
        _ => None,
    };

    if let Some(target) = target {
        pop_through(stack, target, position, sink, flags);
    }
}

fn pop_through(
    stack: &mut Vec<TagRef>,
    target: usize,
    position: u32,
    sink: &mut impl EventSink,
    flags: &mut DocFlags,
) {
    for &tag in stack[target..].iter().rev() {
        sink.end(tag, position..position, true);
        *flags |= DocFlags::SYNTHETIC_END;
    }
    stack.truncate(target);
}

fn find_last(
    stack: &[TagRef],
    tokens: &Tokenized,
    targets: &[TagCode],
    boundaries: &[TagCode],
) -> Option<usize> {
    for (i, &tag) in stack.iter().enumerate().rev() {
        let code = tag_code(tokens, tag);
        if targets.contains(&code) {
            return Some(i);
        }
        if boundaries.contains(&code) {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_markup_index, tokenizer::tokenize};

    fn norm(input: &str) -> Normalized {
        let index = build_markup_index(input.as_bytes()).unwrap();
        let tokens = tokenize(input.as_bytes(), &index);
        normalize(
            input.as_bytes(),
            &tokens,
            input.len() as u32,
            crate::tokenizer::TextRuns::new(input.as_bytes(), &index),
        )
    }

    fn shape(input: &str) -> Vec<String> {
        let index = build_markup_index(input.as_bytes()).unwrap();
        let tokens = tokenize(input.as_bytes(), &index);
        norm(input)
            .events
            .into_iter()
            .filter_map(|event| match event {
                Event::Start { token } => Some(format!(
                    "+{}",
                    tag_name(input.as_bytes(), &tokens, TagRef::Real(token))
                )),
                Event::StartSynthetic { name, .. } => Some(format!("+{name}!")),
                Event::End { tag, synthetic, .. } => Some(format!(
                    "-{}{}",
                    tag_name(input.as_bytes(), &tokens, tag),
                    if synthetic { "!" } else { "" }
                )),
                Event::Text { .. } => None,
            })
            .collect()
    }

    #[test]
    fn void_elements_get_synthetic_end() {
        assert_eq!(shape("<br><img>"), vec!["+br", "-br!", "+img", "-img!"]);
    }

    #[test]
    fn list_items_are_implied_closed() {
        assert_eq!(
            shape("<ul><li>a<li>b</ul>"),
            vec!["+ul", "+li", "-li!", "+li", "-li!", "-ul"]
        );
    }

    #[test]
    fn p_is_closed_by_block_start() {
        assert_eq!(shape("<p>a<div>b</div>"), vec!["+p", "-p!", "+div", "-div"]);
    }

    #[test]
    fn table_cells_are_implied_closed() {
        assert_eq!(
            shape("<table><tr><td>a<td>b<tr><td>c</table>"),
            vec![
                "+table", "+tbody!", "+tr", "+td", "-td!", "+td", "-td!", "-tr!", "+tr", "+td",
                "-td!", "-tr!", "-tbody!", "-table"
            ]
        );
    }

    #[test]
    fn unmatched_end_is_flagged() {
        let n = norm("</div>");
        assert!(n.flags.contains(DocFlags::MISNESTED));
        assert_eq!(n.warnings.len(), 1);
    }
}
