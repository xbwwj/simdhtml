//! Stage-2 tag/text tokenizer.
//!
//! Stage-1 has already found tag spans and text runs, so this is O(#tags +
//! #text-runs), not O(bytes).  Attribute and text values keep their original
//! byte spans; entities are decoded lazily later.

use std::ops::Range;

use crate::{MarkupIndex, Quote, TagKind, TagToken};

use crate::tagcode::TagCode;

/// A text run between tags.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TextRun {
    pub span: Range<u32>,
    pub raw: bool,
}

/// Lazy text-run source.  The tokenizer no longer materializes a `Vec<TextRun>`;
/// normalizer/builder consume text runs directly in source order.
pub(crate) struct TextRuns<'a> {
    index: &'a MarkupIndex,
    next: usize,
    raw_cursor: usize,
}

impl<'a> TextRuns<'a> {
    pub(crate) fn new(_input: &'a [u8], index: &'a MarkupIndex) -> Self {
        Self {
            index,
            next: 0,
            raw_cursor: 0,
        }
    }
}

impl Iterator for TextRuns<'_> {
    type Item = TextRun;

    fn next(&mut self) -> Option<Self::Item> {
        let run = self.index.text_runs.get(self.next)?.clone();
        self.next += 1;
        while self.raw_cursor < self.index.raw_ranges.len()
            && self.index.raw_ranges[self.raw_cursor].end <= run.start
        {
            self.raw_cursor += 1;
        }
        let raw = self.raw_cursor < self.index.raw_ranges.len()
            && self.index.raw_ranges[self.raw_cursor].start <= run.start
            && self.index.raw_ranges[self.raw_cursor].end >= run.end;
        Some(TextRun { span: run, raw })
    }
}

/// Tokenizer output: structural tags, per-token tag codes and attribute spans.
#[derive(Clone, Debug, Default)]
pub(crate) struct Tokenized {
    pub tags: Vec<TagToken>,
    pub tag_name_start: Vec<u32>,
    pub tag_name_len: Vec<u16>,
    pub tag_code: Vec<TagCode>,
    pub attr_tag: Vec<u32>,
    pub attr_key_start: Vec<u32>,
    pub attr_key_len: Vec<u16>,
    pub attr_value_start: Vec<u32>,
    pub attr_value_len: Vec<u32>,
    pub attr_quote: Vec<Quote>,
}

/// Tokenizes every stage-1 tag span.
///
/// Attributes are cut into structural key/value spans here (no entity decode,
/// lowercase or interning). Text runs are produced lazily by [`TextRuns`].
pub(crate) fn tokenize(input: &[u8], index: &MarkupIndex) -> Tokenized {
    let mut out = Tokenized {
        tags: Vec::with_capacity(index.tag_starts.len()),
        tag_name_start: Vec::with_capacity(index.tag_starts.len()),
        tag_name_len: Vec::with_capacity(index.tag_starts.len()),
        tag_code: Vec::with_capacity(index.tag_starts.len()),
        attr_tag: Vec::with_capacity(index.tag_starts.len()),
        attr_key_start: Vec::with_capacity(index.tag_starts.len()),
        attr_key_len: Vec::with_capacity(index.tag_starts.len()),
        attr_value_start: Vec::with_capacity(index.tag_starts.len()),
        attr_value_len: Vec::with_capacity(index.tag_starts.len()),
        attr_quote: Vec::with_capacity(index.tag_starts.len()),
    };

    for (&start, &end) in index.tag_starts.iter().zip(&index.tag_ends) {
        let start = start as usize;
        let end = end as usize;
        debug_assert!(start <= end && end < input.len());
        let token = parse_tag(input, start, end, &mut out);
        out.tags.push(token);
    }

    out
}

/// Parses one `<...>` span into a `TagToken`.
///
/// End tags ignore attributes. Start-tag attributes are recorded as raw spans;
/// selectors use the attribute SoA or [`FlatDoc::attr_iter`].
fn parse_tag(input: &[u8], start: usize, end: usize, out: &mut Tokenized) -> TagToken {
    let span = start as u32..(end + 1) as u32;
    let tag_index = out.tags.len() as u32;

    if input.get(start + 1) == Some(&b'/') {
        let name_start = start + 2;
        let mut name_end = name_start;
        while name_end <= end && !is_name_delimiter(input[name_end]) {
            name_end += 1;
        }
        out.tag_name_start.push(name_start as u32);
        out.tag_name_len
            .push((name_end - name_start).min(u16::MAX as usize) as u16);
        out.tag_code.push(TagCode::of(&input[name_start..name_end]));
        return TagToken {
            span,
            kind: TagKind::End,
            attrs: 0..0,
            node: crate::NO_NODE,
            self_closing: false,
        };
    }

    let name_start = start + 1;
    let mut i = name_start;
    while i <= end && !is_name_delimiter(input[i]) {
        i += 1;
    }
    out.tag_name_start.push(name_start as u32);
    out.tag_name_len
        .push((i - name_start).min(u16::MAX as usize) as u16);
    out.tag_code.push(TagCode::of(&input[name_start..i]));
    let attrs_start = out.attr_tag.len() as u32;
    let mut self_closing = false;

    while i < end {
        while i < end && is_whitespace(input[i]) {
            i += 1;
        }
        if i >= end {
            break;
        }
        if input[i] == b'/' {
            self_closing = true;
            i += 1;
            continue;
        }
        if input[i] == b'>' {
            break;
        }

        let key_start = i;
        while i < end && !is_whitespace(input[i]) && !matches!(input[i], b'=' | b'/' | b'>') {
            i += 1;
        }
        let key_end = i;
        if key_start == key_end {
            i += 1;
            continue;
        }

        while i < end && is_whitespace(input[i]) {
            i += 1;
        }

        let (value_start, value_len, quote) = if i < end && input[i] == b'=' {
            i += 1;
            while i < end && is_whitespace(input[i]) {
                i += 1;
            }
            if i < end && matches!(input[i], b'"' | b'\'') {
                let q = input[i];
                i += 1;
                let start = i;
                while i < end && input[i] != q {
                    i += 1;
                }
                let len = i - start;
                if i < end {
                    i += 1;
                }
                (
                    start as u32,
                    len as u32,
                    if q == b'"' {
                        Quote::Double
                    } else {
                        Quote::Single
                    },
                )
            } else {
                let start = i;
                while i < end && !is_whitespace(input[i]) && input[i] != b'>' {
                    i += 1;
                }
                (start as u32, (i - start) as u32, Quote::Unquoted)
            }
        } else {
            (i as u32, 0, Quote::Boolean)
        };

        // Structural cut only: keep duplicate attributes in source order.
        // HTML's "first wins" rule is a lookup rule; `find_attr_index` and
        // `find_attr` already return the first occurrence, and multi-attribute
        // projections de-duplicate themselves.
        out.attr_tag.push(tag_index);
        out.attr_key_start.push(key_start as u32);
        out.attr_key_len
            .push((key_end - key_start).min(u16::MAX as usize) as u16);
        out.attr_value_start.push(value_start);
        out.attr_value_len.push(value_len);
        out.attr_quote.push(quote);
    }

    TagToken {
        span,
        kind: TagKind::Start,
        attrs: attrs_start..out.attr_tag.len() as u32,
        node: crate::NO_NODE,
        self_closing,
    }
}

fn is_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0C' | b'\r')
}

fn is_name_delimiter(b: u8) -> bool {
    is_whitespace(b) || b == b'/' || b == b'>'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MarkupIndex;

    fn tok(input: &str) -> (Tokenized, MarkupIndex) {
        let index = crate::build_markup_index(input.as_bytes()).unwrap();
        let t = tokenize(input.as_bytes(), &index);
        (t, index)
    }

    #[test]
    fn simple_tags() {
        let input = r#"<div class="a b" id=x disabled data-q='v'>hi</div>"#;
        let (t, index) = tok(input);
        assert_eq!(t.tags.len(), 2);
        assert_eq!(t.tags[0].kind, TagKind::Start);
        assert_eq!(t.tags[1].kind, TagKind::End);
        let texts: Vec<_> = TextRuns::new(input.as_bytes(), &index).collect();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0].span, 42..44);
    }

    #[test]
    fn self_closing_flag() {
        let (t, _) = tok("<br/><img src=x />");
        assert!(t.tags[0].self_closing);
        assert!(t.tags[1].self_closing);
    }

    #[test]
    fn duplicate_attributes_are_kept_structurally() {
        let (t, _) = tok(r#"<a href="first" HREF="second" href="third">x</a>"#);
        assert_eq!(t.tags[0].attrs, 0..3);
        let keys: Vec<_> = (0..3)
            .map(|a| {
                let start = t.attr_key_start[a] as usize;
                let len = t.attr_key_len[a] as usize;
                &"<a href=\"first\" HREF=\"second\" href=\"third\">x</a>"[start..start + len]
            })
            .collect();
        assert_eq!(keys, vec!["href", "HREF", "href"]);
    }

    #[test]
    fn end_tag_has_no_attributes() {
        let (t, _) = tok("</div foo=bar>");
        assert_eq!(t.tags[0].kind, TagKind::End);
    }

    #[test]
    fn raw_text_flag() {
        let input = "<script>a &amp; <b></script><p>x</p>";
        let (_, index) = tok(input);
        let texts: Vec<_> = TextRuns::new(input.as_bytes(), &index).collect();
        assert!(texts[0].raw);
        assert!(!texts[1].raw);
    }
}
