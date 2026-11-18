//! Region masks for comments, declarations and rawtext elements.
//!
//! Quoting is a parity problem *inside* a tag; regions are not.  The scanner
//! first finds these ranges with `memchr`/`memmem`, then stage-1 masks the
//! bytes out before running the `in_tag` fixed point.  This is the "Pass A"
//! from `ARCHITECTURE.zh.md` §5.4.
//!
//! The region scan is O(number of `<` candidates), not O(bytes): every jump
//! either skips to the end of the region or to the end of a tag.

use std::ops::Range;
use std::sync::OnceLock;

use crate::MarkupFlags;
use memchr::{memchr, memmem};

/// How a raw range should be treated by later stages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RawKind {
    /// Comment, doctype, processing instruction or CDATA.  Not text.
    NonText,
    /// Contents of a rawtext/RCDATA element.  This is a text node.
    RawText,
}

/// A masked byte range plus its semantic kind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RawSpan {
    /// Byte range, end-exclusive.
    pub span: Range<u32>,
    /// Whether the range is text or non-text markup.
    pub kind: RawKind,
}

/// Length-filtered rawtext tag check; avoids eight full comparisons per tag.
fn is_rawtext_name(name: &[u8]) -> bool {
    match name.len() {
        3 => name.eq_ignore_ascii_case(b"xmp"),
        5 => name.eq_ignore_ascii_case(b"style") || name.eq_ignore_ascii_case(b"title"),
        6 => name.eq_ignore_ascii_case(b"script") || name.eq_ignore_ascii_case(b"iframe"),
        7 => name.eq_ignore_ascii_case(b"noembed"),
        8 => name.eq_ignore_ascii_case(b"textarea") || name.eq_ignore_ascii_case(b"noframes"),
        _ => false,
    }
}

fn is_fallback_name(name: &[u8]) -> bool {
    match name.len() {
        3 => name.eq_ignore_ascii_case(b"svg"),
        4 => name.eq_ignore_ascii_case(b"math"),
        8 => name.eq_ignore_ascii_case(b"template"),
        _ => false,
    }
}

fn comment_end_finder() -> &'static memmem::Finder<'static> {
    static FINDER: OnceLock<memmem::Finder<'static>> = OnceLock::new();
    FINDER.get_or_init(|| memmem::Finder::new(b"-->"))
}

fn cdata_end_finder() -> &'static memmem::Finder<'static> {
    static FINDER: OnceLock<memmem::Finder<'static>> = OnceLock::new();
    FINDER.get_or_init(|| memmem::Finder::new(b"]]>"))
}

/// Finds all regions that stage-1 must mask before looking for tags.
pub(crate) fn scan_raw_spans(input: &[u8]) -> (Vec<RawSpan>, MarkupFlags) {
    let mut spans = Vec::new();
    let mut flags = MarkupFlags::empty();
    let mut i = 0usize;

    while i < input.len() {
        let Some(pos) = memchr(b'<', &input[i..]) else {
            break;
        };
        let p = i + pos;

        // `<!-- ... -->`
        if input[p..].starts_with(b"<!--") {
            match comment_end_finder().find(&input[p + 4..]) {
                Some(rel) => {
                    let end = p + 4 + rel + 3;
                    spans.push(RawSpan {
                        span: p as u32..end as u32,
                        kind: RawKind::NonText,
                    });
                    i = end;
                }
                None => {
                    spans.push(RawSpan {
                        span: p as u32..input.len() as u32,
                        kind: RawKind::NonText,
                    });
                    flags |= MarkupFlags::UNCLOSED_COMMENT | MarkupFlags::NEEDS_FALLBACK;
                    break;
                }
            }
            continue;
        }

        // CDATA is explicitly outside v1.  Mask it so stage-1 does not invent
        // tags, but tell the caller to fall back.
        if input[p..].starts_with(b"<![CDATA[") {
            flags |= MarkupFlags::NEEDS_FALLBACK;
            match cdata_end_finder().find(&input[p + 9..]) {
                Some(rel) => {
                    let end = p + 9 + rel + 3;
                    spans.push(RawSpan {
                        span: p as u32..end as u32,
                        kind: RawKind::NonText,
                    });
                    i = end;
                }
                None => {
                    spans.push(RawSpan {
                        span: p as u32..input.len() as u32,
                        kind: RawKind::NonText,
                    });
                    flags |= MarkupFlags::UNCLOSED_DECLARATION | MarkupFlags::NEEDS_FALLBACK;
                    break;
                }
            }
            continue;
        }

        // `<!DOCTYPE ...>`, `<!bogus>` and `<?pi>`: mask up to the first `>`.
        if input.get(p + 1).is_some_and(|&b| b == b'!' || b == b'?') {
            match memchr(b'>', &input[p + 1..]) {
                Some(rel) => {
                    let end = p + 1 + rel + 1;
                    spans.push(RawSpan {
                        span: p as u32..end as u32,
                        kind: RawKind::NonText,
                    });
                    i = end;
                }
                None => {
                    spans.push(RawSpan {
                        span: p as u32..input.len() as u32,
                        kind: RawKind::NonText,
                    });
                    flags |= MarkupFlags::UNCLOSED_DECLARATION | MarkupFlags::NEEDS_FALLBACK;
                    break;
                }
            }
            continue;
        }

        // End tags are ordinary tags: leave them alone for the tag scanner.
        if input.get(p + 1) == Some(&b'/') {
            match find_tag_end(input, p + 2) {
                Some(end) => i = end + 1,
                None => i = p + 1,
            }
            continue;
        }

        // Start tag.  Only ASCII letters can start a tag in HTML.
        if input.get(p + 1).is_some_and(u8::is_ascii_alphabetic) {
            let name_start = p + 1;
            let mut name_end = name_start;
            while name_end < input.len() && is_tag_name_byte(input[name_end]) {
                name_end += 1;
            }
            let name = &input[name_start..name_end];

            let Some(tag_end) = find_tag_end(input, p + 1) else {
                // No `>` anywhere: this is an unterminated tag.  Stage-1 will
                // report it; there is nothing left to mask.
                break;
            };

            if is_fallback_name(name) {
                flags |= MarkupFlags::NEEDS_FALLBACK;
            }

            if name.eq_ignore_ascii_case(b"plaintext") {
                let content_start = tag_end + 1;
                if content_start < input.len() {
                    spans.push(RawSpan {
                        span: content_start as u32..input.len() as u32,
                        kind: RawKind::RawText,
                    });
                }
                break;
            }

            if is_rawtext_name(name) {
                let content_start = tag_end + 1;
                match find_rawtext_end(input, content_start, name) {
                    Some((close_start, close_end)) => {
                        if content_start < close_start {
                            spans.push(RawSpan {
                                span: content_start as u32..close_start as u32,
                                kind: RawKind::RawText,
                            });
                        }
                        i = close_end;
                    }
                    None => {
                        if content_start < input.len() {
                            spans.push(RawSpan {
                                span: content_start as u32..input.len() as u32,
                                kind: RawKind::RawText,
                            });
                        }
                        flags |= MarkupFlags::UNCLOSED_RAWTEXT | MarkupFlags::NEEDS_FALLBACK;
                        break;
                    }
                }
                continue;
            }

            i = tag_end + 1;
            continue;
        }

        i = p + 1;
    }

    (spans, flags)
}

/// Finds the first unquoted `>` at or after `from`.
///
/// Ordinary bytes stay on the tight scalar path; `memchr` jumps over quoted
/// attribute values, which can be arbitrarily long.
fn find_tag_end(input: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i < input.len() {
        match input[i] {
            b'>' => return Some(i),
            quote @ (b'"' | b'\'') => {
                let after = i + 1;
                i = after + memchr(quote, &input[after..])? + 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// Finds `</name ...>` case-insensitively, returning `(start_of_</,
/// one_past_>)`.
fn find_rawtext_end(input: &[u8], from: usize, name: &[u8]) -> Option<(usize, usize)> {
    let mut i = from;
    while i + 2 + name.len() <= input.len() {
        let pos = memchr(b'<', &input[i..])?;
        let p = i + pos;
        if input.get(p + 1) == Some(&b'/') {
            let name_start = p + 2;
            let name_end = name_start + name.len();
            if name_end <= input.len()
                && input[name_start..name_end].eq_ignore_ascii_case(name)
                && input
                    .get(name_end)
                    .is_none_or(|&b| is_whitespace(b) || b == b'>' || b == b'/')
                && let Some(end) = find_tag_end(input, name_start)
            {
                return Some((p, end + 1));
            }
        }
        i = p + 1;
    }
    None
}

fn is_tag_name_byte(b: u8) -> bool {
    !is_whitespace(b) && b != b'/' && b != b'>'
}

fn is_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0C' | b'\r')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(input: &str) -> Vec<(Range<u32>, RawKind)> {
        scan_raw_spans(input.as_bytes())
            .0
            .into_iter()
            .map(|s| (s.span, s.kind))
            .collect()
    }

    #[test]
    fn comment_is_masked() {
        assert_eq!(spans("a<!-- <div> -->b"), vec![(1..15, RawKind::NonText)]);
    }

    #[test]
    fn unclosed_comment_reaches_eof() {
        let (ranges, flags) = scan_raw_spans(b"a<!-- nope");
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].span, 1..10);
        assert!(flags.contains(MarkupFlags::UNCLOSED_COMMENT));
        assert!(flags.contains(MarkupFlags::NEEDS_FALLBACK));
    }

    #[test]
    fn doctype_and_pi_are_masked() {
        assert_eq!(
            spans("<!DOCTYPE html><html><?pi x?><body>"),
            vec![(0..15, RawKind::NonText), (21..29, RawKind::NonText)]
        );
    }

    #[test]
    fn rawtext_content_is_masked_but_tags_remain() {
        assert_eq!(
            spans("<script>if (a < b) {}</script><p>x"),
            vec![(8..21, RawKind::RawText)]
        );
        assert_eq!(
            spans("<style>a<b{}</STYLE>"),
            vec![(7..12, RawKind::RawText)]
        );
    }

    #[test]
    fn rawtext_end_tag_must_be_delimited() {
        // `</scriptx>` is content, not the end of the script element.
        let s = "<script>a</scriptx>b</script>c";
        assert_eq!(spans(s), vec![(8..20, RawKind::RawText)]);
    }

    #[test]
    fn unclosed_rawtext_reaches_eof() {
        let (ranges, flags) = scan_raw_spans(b"<script>var x = 1;");
        assert_eq!(ranges[0].span, 8..18);
        assert!(flags.contains(MarkupFlags::UNCLOSED_RAWTEXT));
    }

    #[test]
    fn plaintext_has_no_end_tag() {
        let (ranges, flags) = scan_raw_spans(b"<plaintext><b>not a tag");
        assert_eq!(ranges[0].span, 11..23);
        assert!(!flags.contains(MarkupFlags::UNCLOSED_RAWTEXT));
    }

    #[test]
    fn quoted_gt_does_not_end_a_tag() {
        assert!(spans(r#"<div title="a>b"><span>x</span></div>"#).is_empty());
    }

    #[test]
    fn cdata_is_masked_and_flagged() {
        let (ranges, flags) = scan_raw_spans(b"<![CDATA[<x>]]>");
        assert_eq!(ranges[0].span, 0..15);
        assert!(flags.contains(MarkupFlags::NEEDS_FALLBACK));
    }
}
