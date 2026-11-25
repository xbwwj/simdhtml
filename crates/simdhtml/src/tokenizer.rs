use std::ops::Range;

use crate::document::id::TagId;
use crate::markup::MarkupIndex;
use crate::tag_code::TagCode;

/// A parsed tag: span, kind and the attribute range it owns.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TagToken {
    pub span: Range<u32>,
    pub kind: TagKind,
    pub attr_range: Range<u32>,
    pub self_closing: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TagKind {
    Start,
    End,
}

/// Attribute value quoting style, which decides whether references are decoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttrQuote {
    DoubleQuoted,
    SingleQuoted,
    Unquoted,
    Empty,
}

/// Tokenizer output: tags, tag codes and attribute spans.
#[derive(Clone, Debug, Default)]
pub(crate) struct Tokens {
    pub tags: Vec<TagToken>,
    pub tag_name_start: Vec<u32>,
    pub tag_name_len: Vec<u16>,
    pub tag_code: Vec<TagCode>,
    pub attr_tag: Vec<TagId>,
    pub attr_name_start: Vec<u32>,
    pub attr_name_len: Vec<u16>,
    pub attr_value_start: Vec<u32>,
    pub attr_value_len: Vec<u32>,
    pub attr_quote: Vec<AttrQuote>,
}

/// Cuts every tag span into tokens.  Attributes keep raw key/value spans: no
/// decoding, lowercasing or interning.
pub(crate) fn tokenize(input: &[u8], index: &MarkupIndex) -> Tokens {
    let mut out = Tokens {
        tags: Vec::with_capacity(index.tag_open.len()),
        tag_name_start: Vec::with_capacity(index.tag_open.len()),
        tag_name_len: Vec::with_capacity(index.tag_open.len()),
        tag_code: Vec::with_capacity(index.tag_open.len()),
        attr_tag: Vec::with_capacity(index.tag_open.len()),
        attr_name_start: Vec::with_capacity(index.tag_open.len()),
        attr_name_len: Vec::with_capacity(index.tag_open.len()),
        attr_value_start: Vec::with_capacity(index.tag_open.len()),
        attr_value_len: Vec::with_capacity(index.tag_open.len()),
        attr_quote: Vec::with_capacity(index.tag_open.len()),
    };

    for (&start, &end) in index.tag_open.iter().zip(&index.tag_close) {
        let start = start as usize;
        let end = end as usize;
        debug_assert!(start <= end && end < input.len());
        let token = parse_tag(input, start, end, &mut out);
        out.tags.push(token);
    }

    out
}

/// Parses one `<...>` span.  End tags ignore attributes.
fn parse_tag(input: &[u8], start: usize, end: usize, out: &mut Tokens) -> TagToken {
    let span = start as u32..(end + 1) as u32;
    let tag_index = TagId::new(out.tags.len() as u32);

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
            attr_range: 0..0,
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
        while i < end && is_ws(input[i]) {
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
        while i < end && !is_name_stop(input[i]) {
            i += 1;
        }
        let key_end = i;
        if key_start == key_end {
            i += 1;
            continue;
        }

        while i < end && is_ws(input[i]) {
            i += 1;
        }

        let (value_start, value_len, quote) = if i < end && input[i] == b'=' {
            i += 1;
            while i < end && is_ws(input[i]) {
                i += 1;
            }
            if i < end && matches!(input[i], b'"' | b'\'') {
                let q = input[i];
                i += 1;
                let start = i;
                // Quoted values dominate real pages, so this scan is where the
                // tokenizer's time goes: `memchr` (AVX2) instead of a byte loop.
                i += memchr::memchr(q, &input[start..end]).unwrap_or(end - start);
                let len = i - start;
                if i < end {
                    i += 1;
                }
                (
                    start as u32,
                    len as u32,
                    if q == b'"' {
                        AttrQuote::DoubleQuoted
                    } else {
                        AttrQuote::SingleQuoted
                    },
                )
            } else {
                let start = i;
                while i < end && !is_value_stop(input[i]) {
                    i += 1;
                }
                (start as u32, (i - start) as u32, AttrQuote::Unquoted)
            }
        } else {
            (i as u32, 0, AttrQuote::Empty)
        };

        // Structural cut only: duplicates stay in source order.  HTML's
        // "first wins" is a lookup rule and is applied by the lookups.
        out.attr_tag.push(tag_index);
        out.attr_name_start.push(key_start as u32);
        out.attr_name_len
            .push((key_end - key_start).min(u16::MAX as usize) as u16);
        out.attr_value_start.push(value_start);
        out.attr_value_len.push(value_len);
        out.attr_quote.push(quote);
    }

    TagToken {
        span,
        kind: TagKind::Start,
        attr_range: attrs_start..out.attr_tag.len() as u32,
        self_closing,
    }
}

fn is_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0C' | b'\r')
}

/// Lookup tables for the attribute scanner: three branches per byte of attribute
/// text is most of what the tokenizer spends, a table lookup is one load.
const fn class_table() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut b = 0usize;
    while b < 256 {
        let byte = b as u8;
        let ws = matches!(byte, b' ' | b'\t' | b'\n' | b'\x0C' | b'\r');
        let mut flags = 0u8;
        if ws {
            flags |= FLAG_WS;
        }
        if ws || byte == b'=' || byte == b'/' || byte == b'>' {
            flags |= FLAG_NAME_STOP;
        }
        if ws || byte == b'>' {
            flags |= FLAG_VALUE_STOP;
        }
        if ws || byte == b'/' || byte == b'>' {
            flags |= FLAG_TAG_NAME_STOP;
        }
        table[b] = flags;
        b += 1;
    }
    table
}

const FLAG_WS: u8 = 1;
const FLAG_NAME_STOP: u8 = 1 << 1;
const FLAG_VALUE_STOP: u8 = 1 << 2;
const FLAG_TAG_NAME_STOP: u8 = 1 << 3;

static CLASS: [u8; 256] = class_table();

#[inline]
fn is_ws(b: u8) -> bool {
    CLASS[b as usize] & FLAG_WS != 0
}

#[inline]
fn is_name_stop(b: u8) -> bool {
    CLASS[b as usize] & FLAG_NAME_STOP != 0
}

#[inline]
fn is_value_stop(b: u8) -> bool {
    CLASS[b as usize] & FLAG_VALUE_STOP != 0
}

fn is_name_delimiter(b: u8) -> bool {
    is_whitespace(b) || b == b'/' || b == b'>'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::MarkupIndex;

    fn tok(input: &str) -> (Tokens, MarkupIndex) {
        let index = crate::markup::scan(input.as_bytes()).unwrap();
        let t = tokenize(input.as_bytes(), &index);
        (t, index)
    }

    #[test]
    fn simple_tags() {
        let input = r#"<div class="a b" id=x disabled data-q='v'>hi</div>"#;
        let (t, _) = tok(input);
        assert_eq!(t.tags.len(), 2);
        assert_eq!(t.tags[0].kind, TagKind::Start);
        assert_eq!(t.tags[1].kind, TagKind::End);
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
        assert_eq!(t.tags[0].attr_range, 0..3);
        let keys: Vec<_> = (0..3)
            .map(|a| {
                let start = t.attr_name_start[a] as usize;
                let len = t.attr_name_len[a] as usize;
                &"<a href=\"first\" HREF=\"second\" href=\"third\">x</a>"[start..start + len]
            })
            .collect();
        assert_eq!(keys, vec!["href", "HREF", "href"]);
    }
}
