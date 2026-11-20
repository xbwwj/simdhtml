//! Opaque regions: comments, DOCTYPE, CDATA and raw text element content.
//!
//! A region is a byte range that the tag scanner must not interpret as markup,
//! plus the tokenizer state that produced it.  The ranges themselves are
//! found by the markup state machine ([`super::scan`]), which visits only
//! candidate bytes; this module owns the vocabulary it needs to name them:
//! the raw text element names and the two region kinds that decide whether a
//! text run decodes character references.

use std::ops::Range;

/// What the tokenizer would be looking at inside this region.
///
/// These are the WHATWG tokenizer states other than the data state: comments,
/// DOCTYPE, CDATA, and the contents of raw text / escapable raw text elements.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionKind {
    /// `<!-- ... -->` and the bogus comment forms `<?...>` / `<!...>`.
    Comment,
    /// `<!DOCTYPE ...>`.
    Doctype,
    /// `<![CDATA[ ... ]]>`.
    Cdata,
    /// Contents of a raw text element (script, style, xmp, iframe, noembed,
    /// noframes, plaintext): character data with **no** character references.
    RawText,
    /// Contents of an escapable raw text element (title, textarea): character
    /// data where character references *are* decoded.
    EscapableRawText,
}

impl RegionKind {
    /// Whether the region's bytes become a text node.
    pub fn is_text(self) -> bool {
        matches!(self, Self::RawText | Self::EscapableRawText)
    }

    /// Whether character references inside the region are decoded.
    pub fn decodes_character_references(self) -> bool {
        self != Self::RawText
    }
}

/// A masked byte range plus its semantic kind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpaqueRegion {
    /// Byte range, end-exclusive.
    pub span: Range<u32>,
    /// Whether the range is text or non-text markup.
    pub kind: RegionKind,
}

/// What a start tag's name means for the region scanner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StartTagKind {
    /// No region and nothing that needs the fallback parser.
    Ordinary,
    /// Opens a raw text element region.
    RawText(RegionKind),
    /// `<plaintext>`: raw text that runs to EOF, with no end tag.
    Plaintext,
    /// Valid HTML outside the v1 fast path (SVG, MathML, `<template>`).
    Fallback,
}

/// Case-folded first eight bytes, or `None` when fewer than eight bytes remain.
#[inline]
fn fold8(input: &[u8], at: usize) -> Option<u64> {
    let bytes: [u8; 8] = input.get(at..at + 8)?.try_into().ok()?;
    Some(u64::from_le_bytes(bytes) | 0x2020_2020_2020_2020)
}

/// `mask(len)` of the low `8 * len` bits.
#[inline]
const fn mask8(len: usize) -> u64 {
    if len >= 8 {
        u64::MAX
    } else {
        (1u64 << (8 * len)) - 1
    }
}

/// Folds the case of a literal name into the comparable word.
const fn pat8(name: &[u8; 8]) -> u64 {
    u64::from_le_bytes(*name) | 0x2020_2020_2020_2020
}

/// Classifies a start tag name for the region scanner.
///
/// Every interesting name has length 3..=9, so the length dispatches first and
/// each candidate is one case-folded 8-byte word compare: no slice walks, no
/// call into `eq_ignore_ascii_case`, no branch per name byte.
#[inline]
pub(super) fn start_tag_kind(input: &[u8], at: usize, len: usize) -> StartTagKind {
    use RegionKind::{EscapableRawText, RawText};
    use StartTagKind::{Fallback, Ordinary, Plaintext};

    // Only lengths 3..=9 can name a region.  Measured: a first-letter prefilter
    // before this check made the scan ~3-10% slower on both tag-heavy and
    // realistic corpora, so the length check alone is kept.
    if !(3..=9).contains(&len) {
        return Ordinary;
    }
    let Some(word) = fold8(input, at) else {
        // Only reachable within eight bytes of EOF; compare byte by byte.
        return start_tag_kind_tail(&input[at..at + len]);
    };
    let mask = mask8(len);
    macro_rules! is {
        ($name:expr) => {
            (word ^ pat8($name)) & mask == 0
        };
    }

    match len {
        3 => {
            if is!(b"xmp\0\0\0\0\0") {
                StartTagKind::RawText(RawText)
            } else if is!(b"svg\0\0\0\0\0") {
                Fallback
            } else {
                Ordinary
            }
        }
        4 if is!(b"math\0\0\0\0") => Fallback,
        5 => {
            if is!(b"style\0\0\0") {
                StartTagKind::RawText(RawText)
            } else if is!(b"title\0\0\0") {
                StartTagKind::RawText(EscapableRawText)
            } else {
                Ordinary
            }
        }
        6 => {
            if is!(b"script\0\0") || is!(b"iframe\0\0") {
                StartTagKind::RawText(RawText)
            } else {
                Ordinary
            }
        }
        7 if is!(b"noembed\0") => StartTagKind::RawText(RawText),
        8 => {
            if is!(b"textarea") {
                StartTagKind::RawText(EscapableRawText)
            } else if is!(b"noframes") {
                StartTagKind::RawText(RawText)
            } else if is!(b"template") {
                Fallback
            } else {
                Ordinary
            }
        }
        // The word only holds eight bytes; `plaintext` is nine.
        9 if is!(b"plaintex") && (input[at + 8] | 0x20) == b't' => Plaintext,
        _ => Ordinary,
    }
}

/// Byte-at-a-time fallback for names that end within eight bytes of EOF.
fn start_tag_kind_tail(name: &[u8]) -> StartTagKind {
    use RegionKind::{EscapableRawText, RawText};
    let eq = |lit: &[u8]| name.len() == lit.len() && name.eq_ignore_ascii_case(lit);
    if name.len() == 9 && eq(b"plaintext") {
        StartTagKind::Plaintext
    } else if eq(b"template") || eq(b"svg") || eq(b"math") {
        StartTagKind::Fallback
    } else if eq(b"textarea") || eq(b"title") {
        StartTagKind::RawText(EscapableRawText)
    } else if eq(b"script")
        || eq(b"style")
        || eq(b"xmp")
        || eq(b"iframe")
        || eq(b"noembed")
        || eq(b"noframes")
    {
        StartTagKind::RawText(RawText)
    } else {
        StartTagKind::Ordinary
    }
}

/// Case-insensitive ASCII comparison that stays inlined for short names.
#[inline]
pub(super) fn eq_ignore_ascii_case(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(name: &str) -> StartTagKind {
        // `\0` padding after the name makes the 8-byte window valid.
        let mut buf = name.as_bytes().to_vec();
        buf.extend_from_slice(b"\0\0\0\0\0\0\0\0\0");
        start_tag_kind(&buf, 0, name.len())
    }

    #[test]
    fn rawtext_names_are_recognized_by_length() {
        assert_eq!(kind("script"), StartTagKind::RawText(RegionKind::RawText));
        assert_eq!(kind("SCRIPT"), StartTagKind::RawText(RegionKind::RawText));
        assert_eq!(kind("style"), StartTagKind::RawText(RegionKind::RawText));
        assert_eq!(
            kind("textarea"),
            StartTagKind::RawText(RegionKind::EscapableRawText)
        );
        assert_eq!(
            kind("title"),
            StartTagKind::RawText(RegionKind::EscapableRawText)
        );
        assert_eq!(kind("plaintext"), StartTagKind::Plaintext);
        assert_eq!(kind("svg"), StartTagKind::Fallback);
        assert_eq!(kind("math"), StartTagKind::Fallback);
        assert_eq!(kind("template"), StartTagKind::Fallback);
        assert_eq!(kind("select"), StartTagKind::Ordinary);
        assert_eq!(kind("div"), StartTagKind::Ordinary);
        assert_eq!(kind("scriptx"), StartTagKind::Ordinary);
        assert_eq!(kind("scrip"), StartTagKind::Ordinary);
    }

    #[test]
    fn names_at_eof_use_the_tail_path() {
        assert_eq!(
            start_tag_kind(b"script", 0, 6),
            StartTagKind::RawText(RegionKind::RawText)
        );
        assert_eq!(start_tag_kind(b"div", 0, 3), StartTagKind::Ordinary);
        assert_eq!(start_tag_kind(b"plaintext", 0, 9), StartTagKind::Plaintext);
    }

    #[test]
    fn escapable_raw_text_is_rcdata() {
        assert!(RegionKind::EscapableRawText.decodes_character_references());
        assert!(!RegionKind::RawText.decodes_character_references());
        assert!(RegionKind::EscapableRawText.is_text());
        assert!(RegionKind::RawText.is_text());
        assert!(!RegionKind::Comment.is_text());
    }
}
