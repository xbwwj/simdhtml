mod region;
mod text_runs;

use std::ops::Range;

use crate::error::{ParseError, Result};
use crate::simd::StructuralMasks;

use self::region::{StartTagKind, eq_ignore_ascii_case, start_tag_kind};
use self::text_runs::build_text_runs;

pub use self::region::{OpaqueRegion, RegionKind};
pub(crate) use self::text_runs::{TextRun, TextRuns};

/// Markup scan output: positions, not tokens (the simdjson `structural_index`
/// idea).  No bitmaps survive the scan.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MarkupIndex {
    /// Byte offset of each tag's `<`.
    pub tag_open: Vec<u32>,

    /// Byte offset of each tag's `>`, aligned with `tag_open`.
    pub tag_close: Vec<u32>,

    /// Maximal plain-text ranges.  Raw text element *content* is a text run;
    /// its start and end tags are tag spans.
    pub text_runs: Vec<Range<u32>>,

    /// Ranges masked from tag scanning (comments, DOCTYPE, CDATA, raw text
    /// content); `kind` says whether text there decodes character references.
    pub opaque_regions: Vec<OpaqueRegion>,

    /// Flags for recoverable scan problems.
    pub flags: MarkupFlags,
}

/// The 64-byte block loop, instantiated once per target feature level.  The
/// whole loop is instanced, not just the classifier, so the kernels inline and
/// their constants stay in registers across blocks (a per-block indirect call
/// measured at ~5-8 cyc/block).
macro_rules! scan_blocks_body {
    ($scanner:expr, $input:expr, $probe_lt:path, $classify:path) => {{
        let scanner: &mut Scanner<'_> = $scanner;
        let input: &[u8] = $input;
        let (blocks, tail) = input.as_chunks::<{ crate::simd::BLOCK_SIZE }>();
        let mut base = 0usize;
        for block in blocks {
            if scanner.state == State::Plaintext {
                break;
            }
            scan_one_block!(scanner, block, base, $probe_lt, $classify);
            base += crate::simd::BLOCK_SIZE;
        }
        if !tail.is_empty() && scanner.state != State::Plaintext {
            // Padding bytes are NUL, which is not a candidate in any state.
            let mut padded = [0u8; crate::simd::BLOCK_SIZE];
            padded[..tail.len()].copy_from_slice(tail);
            scan_one_block!(scanner, &padded, base, $probe_lt, $classify);
        }
    }};
}

/// Runs one block, using the one-mask probe for text/rawtext states.
macro_rules! scan_one_block {
    ($scanner:expr, $block:expr, $base:expr, $probe_lt:path, $classify:path) => {{
        let scanner: &mut Scanner<'_> = $scanner;
        let block = $block;
        let base: usize = $base;
        // SAFETY: the enclosing instantiation has the target features both
        // kernels require.
        let masks = unsafe {
            if scanner.lt_only() {
                $probe_lt(block)
            } else {
                Some($classify(block))
            }
        };
        if let Some(masks) = masks {
            scanner.scan_block(&masks, base);
        }
    }};
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx512bw")]
unsafe fn scan_blocks_avx512(scanner: &mut Scanner<'_>, input: &[u8]) {
    // SAFETY: this function has AVX-512BW enabled, which is what the kernels
    // require; the loop's own memory accesses are the scanner's safe ones.
    scan_blocks_body!(
        scanner,
        input,
        crate::simd::avx512::probe_lt_inline,
        crate::simd::avx512::classify_inline
    );
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn scan_blocks_avx2(scanner: &mut Scanner<'_>, input: &[u8]) {
    // SAFETY: this function has AVX2 enabled, which is what `classify_subset`
    // requires; the loop's own memory accesses are the scanner's safe ones.
    scan_blocks_body!(
        scanner,
        input,
        crate::simd::avx2::probe_lt_inline,
        crate::simd::avx2::classify_inline
    );
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn scan_blocks_neon(scanner: &mut Scanner<'_>, input: &[u8]) {
    scan_blocks_body!(
        scanner,
        input,
        crate::simd::neon::probe_lt_inline,
        crate::simd::neon::classify_inline
    );
}

#[allow(unused_unsafe)]
fn scan_blocks_fallback(scanner: &mut Scanner<'_>, input: &[u8]) {
    scan_blocks_body!(
        scanner,
        input,
        crate::simd::fallback::probe_lt,
        crate::simd::fallback::classify
    );
}

#[inline]
fn scan_blocks(scanner: &mut Scanner<'_>, input: &[u8]) {
    #[cfg(target_arch = "x86_64")]
    {
        if crate::simd::avx512_available() {
            // SAFETY: the feature is checked immediately above.
            unsafe { scan_blocks_avx512(scanner, input) };
            return;
        }
        if crate::simd::avx2_available() {
            // SAFETY: the feature is checked immediately above.
            unsafe { scan_blocks_avx2(scanner, input) };
            return;
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if crate::simd::neon_available() {
            unsafe { scan_blocks_neon(scanner, input) };
            return;
        }
    }
    scan_blocks_fallback(scanner, input);
}

/// Builds the markup index for `input`.
pub fn scan(input: &[u8]) -> Result<MarkupIndex> {
    if input.len() > u32::MAX as usize {
        return Err(ParseError::InputTooLarge);
    }

    let mut scanner = Scanner::new(input);
    scan_blocks(&mut scanner, input);

    let (tag_open, tag_close, regions, mut flags) = scanner.finish();
    if !tags_are_alternating(&tag_open, &tag_close) {
        flags |= MarkupFlags::UNBALANCED_TAG;
    }

    let text_runs = build_text_runs(input.len(), &tag_open, &tag_close, &regions);

    Ok(MarkupIndex {
        tag_open,
        tag_close,
        text_runs,
        opaque_regions: regions,
        flags,
    })
}

bitflags::bitflags! {
    /// Flags of the markup scan.  The scan is permissive: malformed input still
    /// yields an index.
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
    pub struct MarkupFlags: u32 {
        /// The number of tag starts and tag ends differs, or they are not in
        /// ascending/alternating order.
        const UNBALANCED_TAG = 1 << 0;

        /// A tag start was still open when the input ended.
        const EOF_IN_TAG = 1 << 1;

        /// A `<!--` comment was not terminated before EOF.
        const EOF_IN_COMMENT = 1 << 2;

        /// A `<!...>` / `<?...>` declaration was not terminated before EOF.
        const EOF_IN_DOCTYPE = 1 << 3;

        /// A raw text element (script/style/...) had no matching end tag.
        const EOF_IN_RAW_TEXT = 1 << 4;

        /// A `<![CDATA[` section was not terminated before EOF.
        const EOF_IN_CDATA = 1 << 5;

        /// The document contains a construct outside the fast path (CDATA,
        /// SVG/MathML, `<template>`, ...).
        const NEEDS_FALLBACK = 1 << 6;
    }
}

/// What the scanner is currently looking at.  The state also selects which
/// block mask the scan walks, so only candidate bytes are ever visited.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    /// Character data outside any tag or opaque region: only `<` matters.
    Data,
    /// Inside a start or end tag: `>`, `"` and `'` matter. `quote` is the open
    /// attribute quote (`0` when none).
    Tag { quote: u8 },
    /// `<!-- ... -->` body: only `>` matters, and only after `--` / `--!`.
    Comment,
    /// `<!...>` / `<?...>` bogus comment or DOCTYPE: ends at the first `>`.
    Bogus,
    /// `<![CDATA[ ... ]]>` body: only `>` matters, and only after `]]`.
    Cdata,
    /// Raw text element content: only `</name` ends it.
    Raw,
    /// `<plaintext>` content: it swallows the rest of the input.
    Plaintext,
}

/// A raw text element whose start tag is being scanned, or whose content we
/// are inside.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RawText<'a> {
    name: &'a [u8],
    kind: RegionKind,
    /// `<plaintext>`: no end tag, so the content runs to EOF.
    plaintext: bool,
}

/// A `</name` seen in raw text that has not been terminated by `>` yet: WHATWG
/// only leaves raw text when the end tag is complete, so it is applied
/// tentatively and rolled back at EOF.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingEnd {
    /// Position of the `<` of `</name`.
    tag_start: usize,
    /// Content start of the raw text element being closed.
    content_start: usize,
    kind: RegionKind,
    /// `tag_open` length before the tentative push.
    tag_open_len: usize,
}

/// Single-pass markup state machine: absolute offsets, one candidate bit at a
/// time, skipping every byte that cannot matter.
struct Scanner<'a> {
    input: &'a [u8],
    tag_open: Vec<u32>,
    tag_close: Vec<u32>,
    regions: Vec<OpaqueRegion>,
    flags: MarkupFlags,
    state: State,
    /// Start of the opaque region that `state` is inside: the `<` of a comment
    /// or declaration, the content start of a raw text element.
    region_start: usize,
    /// Kind of that opaque region.
    region_kind: RegionKind,
    /// Raw text element whose start tag we are in ([`State::Tag`]) or whose
    /// content we are in ([`State::Raw`]).
    raw: Option<RawText<'a>>,
    /// Absolute offset below which candidates have already been consumed by a
    /// handler (`<!--`, a tag name, ...).  It can point into the next block.
    resume: usize,
    /// The raw text end tag currently being scanned, if any.
    pending_end: Option<PendingEnd>,
}

impl<'a> Scanner<'a> {
    fn new(input: &'a [u8]) -> Self {
        // Tags are the only thing the index keeps per input byte, and a very
        // dense document still needs four bytes per tag; a quarter of the
        // input length is a safe upper bound for `<`, `>` pairs.
        let tags = input.len() / 8;
        Self {
            input,
            tag_open: Vec::with_capacity(tags),
            tag_close: Vec::with_capacity(tags),
            regions: Vec::new(),
            flags: MarkupFlags::empty(),
            state: State::Data,
            region_start: 0,
            region_kind: RegionKind::Comment,
            raw: None,
            resume: 0,
            pending_end: None,
        }
    }

    /// Whether this state only reacts to `<` and can use the one-mask probe.
    #[inline]
    fn lt_only(&self) -> bool {
        matches!(self.state, State::Data | State::Raw)
    }

    /// Consumes the candidate bits of one block in ascending order.  `remaining`
    /// is the union of every byte the scan could care about; the state selects
    /// the subset it looks at.
    #[inline(always)]
    fn scan_block(&mut self, m: &StructuralMasks, base: usize) {
        let quotes = m.gt | m.dq | m.sq;
        let mut remaining = (m.lt | quotes) & !bits_below(base, self.resume);
        while remaining != 0 {
            // Bytes the current state looks at.
            let cur = match self.state {
                State::Data | State::Raw => remaining & m.lt,
                State::Tag { .. } => remaining & quotes,
                State::Comment | State::Bogus | State::Cdata => remaining & m.gt,
                State::Plaintext => 0,
            };
            if cur == 0 {
                return;
            }
            let bit = cur.trailing_zeros() as usize;
            let resume = self.step(base + bit);
            self.resume = resume;
            // Everything below `resume` is consumed, including bytes a handler
            // skipped (`<!--`, a tag name, ...).
            remaining &= !bits_below(base, resume);
        }
    }

    /// Handles the candidate byte at `p`; returns the offset scanning resumes
    /// from.  Always inline: an out-of-line call measured ~+30 cyc/block on
    /// tag-dense input.
    #[inline(always)]
    fn step(&mut self, p: usize) -> usize {
        match self.state {
            State::Data => self.data_step(p),
            State::Tag { .. } => self.tag_step(p),
            State::Comment => self.comment_step(p),
            State::Bogus | State::Cdata => self.declaration_step(p),
            State::Raw => self.raw_step(p),
            State::Plaintext => self.input.len(),
        }
    }

    fn data_step(&mut self, p: usize) -> usize {
        let input = self.input;
        let Some(&next) = input.get(p + 1) else {
            return p + 1;
        };
        match next {
            // `<!-- ... -->`
            b'!' => {
                if input[p..].starts_with(b"<!--") {
                    self.open_region(p, RegionKind::Comment, State::Comment);
                    p + 4
                } else if input[p..].starts_with(b"<![CDATA[") {
                    self.flags |= MarkupFlags::NEEDS_FALLBACK;
                    self.open_region(p, RegionKind::Cdata, State::Cdata);
                    p + 9
                } else {
                    let kind = if input[p + 1..]
                        .get(..8)
                        .is_some_and(|s| s.eq_ignore_ascii_case(b"!doctype"))
                    {
                        RegionKind::Doctype
                    } else {
                        RegionKind::Comment
                    };
                    self.open_region(p, kind, State::Bogus);
                    p + 2
                }
            }
            // `<?...>` bogus comment.
            b'?' => {
                self.open_region(p, RegionKind::Comment, State::Bogus);
                p + 2
            }
            // End tags are ordinary tags; skip the name like a start tag
            // does, so a quote byte in it cannot open an attribute value.
            //
            // Only an ASCII letter can start an end tag name: `</#` is a bogus
            // comment (data after `</`), and `</` at EOF leaves its two bytes
            // as text.
            b'/' => match input.get(p + 2) {
                Some(b'>') => {
                    self.tag_open.push(p as u32);
                    self.state = State::Tag { quote: 0 };
                    self.tag_name_end(p + 2)
                }
                Some(byte) if byte.is_ascii_alphabetic() => {
                    self.tag_open.push(p as u32);
                    self.state = State::Tag { quote: 0 };
                    self.tag_name_end(p + 2)
                }
                Some(_) => {
                    self.open_region(p, RegionKind::Comment, State::Bogus);
                    p + 2
                }
                None => p + 2,
            },
            // Only ASCII letters can start a tag in HTML.
            b if b.is_ascii_alphabetic() => self.start_tag(p),
            _ => p + 1,
        }
    }

    /// One past the last byte of the tag name at `from`.  Tag names are short
    /// (median 3-4 bytes); a whitespace/`/`/`>` bitmap costs more than this loop.
    #[inline]
    fn tag_name_end(&self, from: usize) -> usize {
        let mut end = from;
        while self.input.get(end).is_some_and(|&b| is_tag_name_byte(b)) {
            end += 1;
        }
        end
    }

    fn start_tag(&mut self, p: usize) -> usize {
        let name_end = self.tag_name_end(p + 1);
        let name = &self.input[p + 1..name_end];

        self.tag_open.push(p as u32);
        self.state = State::Tag { quote: 0 };

        self.raw = match start_tag_kind(self.input, p + 1, name_end - p - 1) {
            StartTagKind::Ordinary => None,
            StartTagKind::RawText(kind) => Some(RawText {
                name,
                kind,
                plaintext: false,
            }),
            StartTagKind::Plaintext => Some(RawText {
                name,
                kind: RegionKind::RawText,
                plaintext: true,
            }),
            StartTagKind::Fallback => {
                self.flags |= MarkupFlags::NEEDS_FALLBACK;
                None
            }
        };

        name_end
    }

    fn tag_step(&mut self, p: usize) -> usize {
        let State::Tag { quote } = self.state else {
            unreachable!("tag_step outside the tag state")
        };
        let b = self.input[p];

        if b == b'>' {
            // A `>` inside an attribute value is data.
            if quote != 0 {
                return p + 1;
            }
            self.tag_close.push(p as u32);
            if let Some(pending) = self.pending_end.take() {
                self.region_start = pending.content_start;
                self.region_kind = pending.kind;
                self.close_region(pending.tag_start);
                return p + 1;
            }
            return match self.raw.take() {
                Some(raw) if raw.plaintext => {
                    // `<plaintext>` has no end tag: the rest is content.
                    self.state = State::Plaintext;
                    self.region_start = p + 1;
                    self.region_kind = raw.kind;
                    self.input.len()
                }
                Some(raw) => {
                    self.state = State::Raw;
                    self.region_start = p + 1;
                    self.region_kind = raw.kind;
                    self.raw = Some(raw);
                    p + 1
                }
                None => {
                    self.state = State::Data;
                    p + 1
                }
            };
        }

        // The only other candidates in a tag are the two quote bytes.  The
        // quote that opened the value closes it; the other kind is data.
        self.state = State::Tag {
            quote: if quote == 0 {
                b
            } else if quote == b {
                0
            } else {
                quote
            },
        };
        p + 1
    }

    /// `>` inside a comment: it only ends the comment after `--` or `--!`.
    fn comment_step(&mut self, p: usize) -> usize {
        let input = self.input;
        let ends = (p >= 2 && input[p - 2..p] == *b"--") || (p >= 3 && input[p - 3..p] == *b"--!");
        if ends {
            self.close_region(p + 1);
        }
        p + 1
    }

    /// `>` inside a bogus comment / DOCTYPE / CDATA section.
    fn declaration_step(&mut self, p: usize) -> usize {
        let ends = match self.state {
            State::Cdata => p >= 2 && self.input[p - 2..p] == *b"]]",
            _ => true,
        };
        if ends {
            self.close_region(p + 1);
        }
        p + 1
    }

    /// `<` inside raw text: only `</name` (case-insensitive, followed by
    /// whitespace, `/` or `>`) ends the element.
    fn raw_step(&mut self, p: usize) -> usize {
        let input = self.input;
        let Some(raw) = self.raw else {
            return p + 1;
        };
        if input.get(p + 1) != Some(&b'/') {
            return p + 1;
        }

        let name_start = p + 2;
        let name_end = name_start + raw.name.len();
        let is_end_tag = input
            .get(name_start..name_end)
            .is_some_and(|s| eq_ignore_ascii_case(s, raw.name))
            && input
                .get(name_end)
                .is_none_or(|&b| is_whitespace(b) || b == b'>' || b == b'/');
        if !is_end_tag {
            return p + 1;
        }

        self.pending_end = Some(PendingEnd {
            tag_start: p,
            content_start: self.region_start,
            kind: self.region_kind,
            tag_open_len: self.tag_open.len(),
        });
        self.tag_open.push(p as u32);
        self.state = State::Tag { quote: 0 };
        self.raw = None;
        p + 2
    }

    fn open_region(&mut self, start: usize, kind: RegionKind, state: State) {
        self.region_start = start;
        self.region_kind = kind;
        self.state = state;
    }

    fn close_region(&mut self, end: usize) {
        if self.region_start < end {
            self.regions.push(OpaqueRegion {
                span: self.region_start as u32..end as u32,
                kind: self.region_kind,
            });
        }
        self.state = State::Data;
    }

    /// Closes the region that was open at EOF and reports the scan flags.
    fn finish(mut self) -> (Vec<u32>, Vec<u32>, Vec<OpaqueRegion>, MarkupFlags) {
        // `</script` without a `>` is raw text, not a tag.
        if let Some(pending) = self.pending_end.take() {
            self.tag_open.truncate(pending.tag_open_len);
            self.region_start = pending.content_start;
            self.region_kind = pending.kind;
            self.raw = None;
            self.state = State::Raw;
        }
        let eof = match self.state {
            State::Data | State::Plaintext => MarkupFlags::empty(),
            State::Tag { .. } => MarkupFlags::EOF_IN_TAG,
            State::Comment => MarkupFlags::EOF_IN_COMMENT,
            State::Bogus => MarkupFlags::EOF_IN_DOCTYPE,
            State::Cdata => MarkupFlags::EOF_IN_CDATA,
            State::Raw => MarkupFlags::EOF_IN_RAW_TEXT,
        };
        if matches!(
            self.state,
            State::Comment | State::Bogus | State::Cdata | State::Raw | State::Plaintext
        ) {
            self.close_region(self.input.len());
        }
        self.flags |= eof;
        (self.tag_open, self.tag_close, self.regions, self.flags)
    }
}

/// Bits of `[base, base + 64)` that are below `resume`.
#[inline]
fn bits_below(base: usize, resume: usize) -> u64 {
    if resume <= base {
        0
    } else {
        let d = resume - base;
        if d >= crate::simd::BLOCK_SIZE {
            u64::MAX
        } else {
            (1u64 << d) - 1
        }
    }
}

#[inline]
fn is_tag_name_byte(b: u8) -> bool {
    !is_whitespace(b) && b != b'/' && b != b'>'
}

#[inline]
fn is_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0C' | b'\r')
}

fn tags_are_alternating(starts: &[u32], ends: &[u32]) -> bool {
    if starts.len() != ends.len() {
        return false;
    }
    for i in 0..starts.len() {
        if starts[i] >= ends[i] {
            return false;
        }
        if i + 1 < starts.len() && ends[i] >= starts[i + 1] {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn index(input: &str) -> MarkupIndex {
        scan(input.as_bytes()).unwrap()
    }

    fn tag_names(input: &str) -> Vec<(bool, String, bool)> {
        let idx = index(input);
        idx.tag_open
            .iter()
            .zip(&idx.tag_close)
            .map(|(&start, &end)| tag_from_span(input.as_bytes(), start as usize, end as usize))
            .collect()
    }

    fn tag_from_span(input: &[u8], start: usize, end: usize) -> (bool, String, bool) {
        assert_eq!(input[start], b'<');
        let (is_end, name_start) = if input.get(start + 1) == Some(&b'/') {
            (true, start + 2)
        } else {
            (false, start + 1)
        };
        let mut name_end = name_start;
        while name_end < input.len()
            && !matches!(
                input[name_end],
                b' ' | b'\t' | b'\n' | b'\x0C' | b'\r' | b'/' | b'>'
            )
        {
            name_end += 1;
        }
        let name = String::from_utf8_lossy(&input[name_start..name_end]).to_ascii_lowercase();
        let mut k = end - 1;
        while k > start && matches!(input[k], b' ' | b'\t' | b'\n' | b'\x0C' | b'\r') {
            k -= 1;
        }
        let self_closing = input[k] == b'/';
        (is_end, name, self_closing)
    }

    #[test]
    fn simple_document() {
        let idx = index("<html><body><p>hi</p></body></html>");
        assert_eq!(idx.tag_open, vec![0, 6, 12, 17, 21, 28]);
        assert_eq!(idx.tag_close, vec![5, 11, 14, 20, 27, 34]);
        assert!(!idx.flags.contains(MarkupFlags::UNBALANCED_TAG));
        assert_eq!(idx.text_runs, vec![15..17]);
    }

    #[test]
    fn comments_are_not_text_or_tags() {
        let idx = index("<p>a<!-- <b> -->b</p>");
        assert_eq!(idx.tag_open.len(), 2);
        assert_eq!(idx.text_runs, vec![3..4, 16..17]);
    }

    #[test]
    fn unterminated_rawtext_end_tag_is_text() {
        // WHATWG leaves raw text only when `</script` is followed by `>`; at
        // EOF those bytes are still script text.
        let idx = index("<script>x</script ");
        assert_eq!(idx.tag_open.len(), 1, "{idx:?}");
        assert_eq!(idx.tag_close.len(), 1, "{idx:?}");
        assert_eq!(
            idx.opaque_regions[0].span,
            8..18,
            "{:?}",
            idx.opaque_regions
        );
        assert!(idx.flags.contains(MarkupFlags::EOF_IN_RAW_TEXT));

        // The same end tag with a `>` closes the element.
        let idx = index("<script>x</script >");
        assert_eq!(idx.tag_open.len(), 2, "{idx:?}");
        assert_eq!(idx.opaque_regions[0].span, 8..9, "{:?}", idx.opaque_regions);
    }

    #[test]
    fn abridged_comment_ends_close_the_comment() {
        // `<!-->`, `<!--->` and `--!>` all close a comment.
        for input in ["a<!-->b", "a<!--->b", "a<!-- x --!>b"] {
            let idx = index(input);
            assert_eq!(idx.opaque_regions.len(), 1, "{input}: {idx:?}");
            assert!(!idx.flags.contains(MarkupFlags::EOF_IN_COMMENT), "{input}");
            let last = input.len() as u32 - 1;
            assert_eq!(
                idx.text_runs,
                vec![0..1, last..last + 1],
                "{input}: {idx:?}"
            );
        }
    }

    #[test]
    fn rawtext_content_is_text_but_not_tags() {
        let idx = index("<script>if (a < b) {}</script><p>x</p>");
        assert_eq!(
            tag_names("<script>if (a < b) {}</script><p>x</p>"),
            vec![
                (false, "script".to_string(), false),
                (true, "script".to_string(), false),
                (false, "p".to_string(), false),
                (true, "p".to_string(), false),
            ]
        );
        assert_eq!(idx.text_runs, vec![8..21, 33..34]);
    }

    #[test]
    fn mixed_quotes_in_one_tag() {
        let idx = index(r#"<a href='x"y' title="a'b">z</a>"#);
        assert_eq!(idx.tag_open, vec![0, 27]);
        assert_eq!(idx.tag_close, vec![25, 30]);
        assert_eq!(idx.text_runs, vec![26..27]);
        assert!(!idx.flags.contains(MarkupFlags::UNBALANCED_TAG));
    }

    #[test]
    fn quoted_gt_in_dense_block() {
        // Two tags with a quoted `>` in one block: the state machine must not
        // treat either `>` as the end of the first tag.
        let input = r#"<div title="a > b">hello</div>"#.repeat(2);
        let idx = index(&input);
        assert_eq!(idx.tag_open.len(), 4, "{idx:?}");
        assert_eq!(idx.tag_close.len(), 4, "{idx:?}");
        assert!(!idx.flags.contains(MarkupFlags::UNBALANCED_TAG), "{idx:?}");
        assert_eq!(idx.text_runs, vec![19..24, 49..54]);
    }

    #[test]
    fn comment_and_rawtext_share_one_pass() {
        // A tag, a comment and a raw text element inside one block: the state
        // machine must switch between all three without a second scan.
        let idx = index(r#"<div>a<!-- <b> --><script>x<y</script></div>"#);
        assert_eq!(idx.tag_open.len(), 4, "{idx:?}");
        assert_eq!(idx.tag_close.len(), 4, "{idx:?}");
        assert_eq!(idx.text_runs, vec![5..6, 26..29]);
    }

    #[test]
    fn tag_and_quote_span_block_boundaries() {
        let mut input = String::new();
        input.push_str(&"a".repeat(60));
        input.push_str(r#"<div title="a > b">text</div>"#);
        let idx = index(&input);
        assert_eq!(idx.tag_open.len(), 2, "{idx:?}");
        assert!(!idx.flags.contains(MarkupFlags::UNBALANCED_TAG), "{idx:?}");
    }

    #[test]
    fn unterminated_tag_is_flagged() {
        let idx = index("<div class=x");
        assert!(idx.flags.contains(MarkupFlags::EOF_IN_TAG));
        assert!(idx.flags.contains(MarkupFlags::UNBALANCED_TAG));
    }

    #[test]
    fn text_runs_exclude_comments_and_keep_rawtext() {
        let idx = index("<p>a<!--c-->b</p><script>x</script>");
        assert_eq!(idx.text_runs, vec![3..4, 12..13, 25..26]);
    }

    #[test]
    fn rawtext_content_starting_at_block_boundary_is_masked() {
        // `<script>` ends exactly at byte 63, so its content starts at 64.
        let mut input = "x".repeat(56);
        input.push_str("<script><div>not a tag</div></script>");
        let idx = index(&input);
        assert_eq!(idx.tag_open.len(), 2, "{idx:?}");
        assert_eq!(idx.tag_close.len(), 2, "{idx:?}");
        assert!(
            idx.opaque_regions.iter().any(|r| r.span == (64..84)),
            "{:?}",
            idx.opaque_regions
        );
    }

    #[test]
    fn plain_text_input() {
        let idx = index("just some text");
        assert_eq!(idx.tag_open.len(), 0);
        assert_eq!(idx.text_runs, vec![0..14]);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(4096))]

        /// The bitmap scanner must agree with a byte-at-a-time tokenizer on
        /// tag spans and on every opaque region.
        #[test]
        fn scanner_matches_byte_reference(input in proptest::collection::vec(any::<u8>(), 0..1024)) {
            let want = byte_reference(&input);
            let got = scan(&input).unwrap();
            prop_assert_eq!(&got.tag_open, &want.tag_open, "input={:?}", input);
            prop_assert_eq!(&got.tag_close, &want.tag_close, "input={:?}", input);
            prop_assert_eq!(&got.opaque_regions, &want.regions, "input={:?}", input);
            prop_assert_eq!(got.flags, want.flags, "input={:?}", input);
        }
    }

    #[derive(Default)]
    struct Reference {
        tag_open: Vec<u32>,
        tag_close: Vec<u32>,
        regions: Vec<OpaqueRegion>,
        flags: MarkupFlags,
    }

    /// Raw text element names, written independently of `region.rs`.
    fn is_rawtext_name_ref(name: &[u8]) -> bool {
        let lower = name.to_ascii_lowercase();
        matches!(
            lower.as_slice(),
            b"xmp"
                | b"style"
                | b"title"
                | b"script"
                | b"iframe"
                | b"noembed"
                | b"noframes"
                | b"textarea"
                | b"plaintext"
        )
    }

    /// Independent, fully scalar reference scanner used only by tests.
    ///
    /// It walks every byte and has no bitmaps, no block loop and no candidate
    /// skipping, so it shares no failure mode with [`scan`].
    fn byte_reference(input: &[u8]) -> Reference {
        #[derive(Clone, Copy, PartialEq)]
        enum S {
            Data,
            Tag,
            Comment,
            Bogus,
            Cdata,
            Raw,
            Plaintext,
        }

        let mut out = Reference::default();
        let mut state = S::Data;
        let mut quote = 0u8;
        let mut region_start = 0usize;
        let mut region_kind = RegionKind::Comment;
        let mut raw_name: &[u8] = b"";
        let mut raw_kind = RegionKind::RawText;
        let mut pending: Option<(usize, usize, RegionKind, usize)> = None;
        let mut i = 0usize;

        while i < input.len() {
            let b = input[i];
            match state {
                S::Data => {
                    let Some(&next) = input.get(i + 1) else {
                        i += 1;
                        continue;
                    };
                    if b != b'<' {
                        i += 1;
                        continue;
                    }
                    if input[i..].starts_with(b"<!--") {
                        region_start = i;
                        region_kind = RegionKind::Comment;
                        state = S::Comment;
                        i += 4;
                    } else if input[i..].starts_with(b"<![CDATA[") {
                        region_start = i;
                        region_kind = RegionKind::Cdata;
                        out.flags |= MarkupFlags::NEEDS_FALLBACK;
                        state = S::Cdata;
                        i += 9;
                    } else if next == b'!' {
                        region_start = i;
                        region_kind = if input[i + 1..]
                            .get(..8)
                            .is_some_and(|s| s.eq_ignore_ascii_case(b"!doctype"))
                        {
                            RegionKind::Doctype
                        } else {
                            RegionKind::Comment
                        };
                        state = S::Bogus;
                        i += 2;
                    } else if next == b'?' {
                        region_start = i;
                        region_kind = RegionKind::Comment;
                        state = S::Bogus;
                        i += 2;
                    } else if next == b'/'
                        && input
                            .get(i + 2)
                            .is_some_and(|c| !c.is_ascii_alphabetic() && *c != b'>')
                    {
                        // `</` followed by anything but a letter (or `>`, which
                        // is a tag with an empty name) opens a bogus comment.
                        region_start = i;
                        region_kind = RegionKind::Comment;
                        state = S::Bogus;
                        i += 2;
                    } else if next == b'/' && input.get(i + 2).is_none() {
                        // `</` at end of input leaves its bytes as text.
                        i += 2;
                    } else if next == b'/' || next.is_ascii_alphabetic() {
                        out.tag_open.push(i as u32);
                        quote = 0;
                        state = S::Tag;
                        // Consume the tag name so its bytes cannot be read as
                        // quotes.
                        let mut j = i + 1;
                        if next == b'/' {
                            j += 1;
                        }
                        let name_start = j;
                        while input
                            .get(j)
                            .is_some_and(|&c| !is_whitespace(c) && c != b'/' && c != b'>')
                        {
                            j += 1;
                        }
                        let name = &input[name_start..j];
                        // The reference deliberately re-derives the raw text
                        // names instead of calling `start_tag_kind`, so the
                        // two classifiers are checked against each other.
                        if matches!(
                            name.to_ascii_lowercase().as_slice(),
                            b"svg" | b"math" | b"template"
                        ) {
                            out.flags |= MarkupFlags::NEEDS_FALLBACK;
                        }
                        if is_rawtext_name_ref(name) {
                            raw_name = name;
                            raw_kind = if eq_ignore_ascii_case(name, b"title")
                                || eq_ignore_ascii_case(name, b"textarea")
                            {
                                RegionKind::EscapableRawText
                            } else {
                                RegionKind::RawText
                            };
                        } else {
                            raw_name = b"";
                        }
                        i = j;
                    } else {
                        i += 1;
                    }
                }
                S::Tag => {
                    if b == b'>' && quote == 0 {
                        out.tag_close.push(i as u32);
                        i += 1;
                        if let Some((tag_start, content_start, kind, _)) = pending.take() {
                            if content_start < tag_start {
                                out.regions.push(OpaqueRegion {
                                    span: content_start as u32..tag_start as u32,
                                    kind,
                                });
                            }
                            state = S::Data;
                        } else if raw_name.is_empty() {
                            state = S::Data;
                        } else if eq_ignore_ascii_case(raw_name, b"plaintext") {
                            region_start = i;
                            region_kind = raw_kind;
                            state = S::Plaintext;
                        } else {
                            region_start = i;
                            region_kind = raw_kind;
                            state = S::Raw;
                        }
                    } else if (b == b'"' || b == b'\'') && quote != b {
                        // Opens a value, or is data inside the other kind of
                        // quoted value.
                        quote = if quote == 0 { b } else { quote };
                        i += 1;
                    } else if b == quote {
                        quote = 0;
                        i += 1;
                    } else {
                        i += 1;
                    }
                }
                S::Comment => {
                    if b == b'>'
                        && ((i >= 2 && input[i - 2..i] == *b"--")
                            || (i >= 3 && input[i - 3..i] == *b"--!"))
                    {
                        out.regions.push(OpaqueRegion {
                            span: region_start as u32..(i + 1) as u32,
                            kind: region_kind,
                        });
                        state = S::Data;
                    }
                    i += 1;
                }
                S::Bogus => {
                    if b == b'>' {
                        out.regions.push(OpaqueRegion {
                            span: region_start as u32..(i + 1) as u32,
                            kind: region_kind,
                        });
                        state = S::Data;
                    }
                    i += 1;
                }
                S::Cdata => {
                    if b == b'>' && i >= 2 && input[i - 2..i] == *b"]]" {
                        out.regions.push(OpaqueRegion {
                            span: region_start as u32..(i + 1) as u32,
                            kind: region_kind,
                        });
                        state = S::Data;
                    }
                    i += 1;
                }
                S::Raw => {
                    let is_end = b == b'<'
                        && input.get(i + 1) == Some(&b'/')
                        && input
                            .get(i + 2..i + 2 + raw_name.len())
                            .is_some_and(|s| s.eq_ignore_ascii_case(raw_name))
                        && input
                            .get(i + 2 + raw_name.len())
                            .is_none_or(|&c| is_whitespace(c) || c == b'>' || c == b'/');
                    if is_end {
                        pending = Some((i, region_start, region_kind, out.tag_open.len()));
                        out.tag_open.push(i as u32);
                        quote = 0;
                        raw_name = b"";
                        state = S::Tag;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                S::Plaintext => i += 1,
            }
        }

        // `</script` without a `>` is raw text, not a tag.
        if let Some((_, content_start, kind, tag_open_len)) = pending.take() {
            out.tag_open.truncate(tag_open_len);
            region_start = content_start;
            region_kind = kind;
            state = S::Raw;
        }
        let eof = match state {
            S::Data | S::Plaintext => MarkupFlags::empty(),
            S::Tag => MarkupFlags::EOF_IN_TAG,
            S::Comment => MarkupFlags::EOF_IN_COMMENT,
            S::Bogus => MarkupFlags::EOF_IN_DOCTYPE,
            S::Cdata => MarkupFlags::EOF_IN_CDATA,
            S::Raw => MarkupFlags::EOF_IN_RAW_TEXT,
        };
        out.flags |= eof;
        if matches!(
            state,
            S::Comment | S::Bogus | S::Cdata | S::Raw | S::Plaintext
        ) && region_start < input.len()
        {
            out.regions.push(OpaqueRegion {
                span: region_start as u32..input.len() as u32,
                kind: region_kind,
            });
        }
        if !tags_are_alternating(&out.tag_open, &out.tag_close) {
            out.flags |= MarkupFlags::UNBALANCED_TAG;
        }
        out
    }
}
