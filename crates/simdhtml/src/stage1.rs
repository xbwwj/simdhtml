//! Stage-1: turn bytes into a [`MarkupIndex`].
//!
//! The scan has two passes:
//!
//! 1. [`raw`] finds comments/declarations/rawtext ranges with `memchr`.
//! 2. 64-byte blocks are classified by `simdhtml::simd`; `in_tag` and the two
//!    quote states are solved with `prefix_xor` fixed points.  A block that
//!    fails to converge (or has overlapping quote states) falls back to a
//!    scalar state machine that only visits candidate bytes.
//!
//! The output stores tag positions and text/raw intervals; no bitmaps survive
//! this function.

use std::fmt;
use std::ops::Range;

use crate::simd::bit_index::for_each_set_bit;
use crate::simd::{classify_stage1, collect_indices, prefix_xor_with_carry};
use crate::{MarkupFlags, MarkupIndex};

use crate::raw::{RawKind, RawSpan, scan_raw_spans};

/// Errors that can be returned before stage-1 scanning starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage1Error {
    /// `MarkupIndex` stores offsets as `u32`.
    InputTooLarge,
}

impl fmt::Display for Stage1Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputTooLarge => write!(f, "input is larger than u32::MAX bytes"),
        }
    }
}

impl std::error::Error for Stage1Error {}

/// Builds the stage-1 markup index for `input`.
///
/// # Errors
///
/// Returns [`Stage1Error::InputTooLarge`] when the input cannot be addressed
/// by `u32` byte offsets.
pub fn build_markup_index(input: &[u8]) -> Result<MarkupIndex, Stage1Error> {
    if input.len() > u32::MAX as usize {
        return Err(Stage1Error::InputTooLarge);
    }

    let (raw_spans, mut flags) = scan_raw_spans(input);
    let mut tag_starts = Vec::new();
    let mut tag_ends = Vec::new();

    let mut carry = Carry::default();
    let mut raw_cursor = 0usize;
    let mut offset = 0usize;
    let mut repaired = false;

    // Full blocks are borrowed in place: the AVX2 classifier loads them
    // directly into registers with an unaligned load.  Only the final partial
    // block needs a zero-padded stack copy.
    while offset + crate::simd::BLOCK_SIZE <= input.len() {
        let block: &[u8; crate::simd::BLOCK_SIZE] = input[offset..offset + crate::simd::BLOCK_SIZE]
            .try_into()
            .expect("full block");
        let valid_len = crate::simd::BLOCK_SIZE;
        let raw = raw_mask_for_block(&raw_spans, &mut raw_cursor, offset, valid_len);
        let next_byte = input.get(offset + valid_len).copied();
        let result = scan_block(block, valid_len, next_byte, raw, carry);

        collect_indices(result.lt_ok, offset as u32, &mut tag_starts);
        collect_indices(result.gt_ok, offset as u32, &mut tag_ends);
        repaired |= result.repaired;
        carry = result.carry;
        offset += valid_len;
    }

    if offset < input.len() {
        let valid_len = input.len() - offset;
        let mut block = [0u8; crate::simd::BLOCK_SIZE];
        block[..valid_len].copy_from_slice(&input[offset..]);
        let raw = raw_mask_for_block(&raw_spans, &mut raw_cursor, offset, valid_len);
        let result = scan_block(&block, valid_len, None, raw, carry);

        collect_indices(result.lt_ok, offset as u32, &mut tag_starts);
        collect_indices(result.gt_ok, offset as u32, &mut tag_ends);
        repaired |= result.repaired;
        carry = result.carry;
    }

    if repaired {
        flags |= MarkupFlags::SCALAR_REPAIR;
    }
    if carry.in_tag {
        flags |= MarkupFlags::UNCLOSED_TAG;
    }
    if !tags_are_alternating(&tag_starts, &tag_ends) {
        flags |= MarkupFlags::UNBALANCED_TAG;
    }

    let text_runs = build_text_runs(input.len(), &tag_starts, &tag_ends, &raw_spans);

    Ok(MarkupIndex {
        tag_starts,
        tag_ends,
        text_runs,
        raw_ranges: raw_spans.into_iter().map(|s| s.span).collect(),
        flags,
    })
}

/// Scanner state carried across a 64-byte block boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Carry {
    in_tag: bool,
    /// `Some('"')` / `Some('\'')` when a quoted attribute value spans blocks.
    quote: Option<u8>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct BlockScan {
    lt_ok: u64,
    gt_ok: u64,
    carry: Carry,
    repaired: bool,
}

const MAX_ITERS: usize = 8;

/// Solves the `in_tag` / double-quote / single-quote states for one block.
/// Solves per-byte tag and quote state for one block with a bit-parallel fixed
/// point.
///
/// `t` is "inside a tag after this byte"; `d`/`s` are double/single quote
/// parity.  The loop starts from the conservative all-closes state and removes
/// quoted `>` bytes.  If it does not converge, [`scan_block_scalar`] repairs
/// the block using only candidate bytes.
fn scan_block(
    block: &[u8; crate::simd::BLOCK_SIZE],
    valid_len: usize,
    next_byte: Option<u8>,
    raw: u64,
    carry: Carry,
) -> BlockScan {
    debug_assert!((1..=crate::simd::BLOCK_SIZE).contains(&valid_len));
    let valid_mask = valid_mask(valid_len);
    let m = classify_stage1(block);
    let not_raw = !raw & valid_mask;

    let lt = m.lt & not_raw;
    let gt = m.gt & not_raw;
    let dq = m.dq & not_raw;
    let sq = m.sq & not_raw;

    // Tag-start detection is only needed at the sparse bytes after `<`.
    // Classifying A-Z/a-z for the whole block would be wasted SIMD work.
    let mut lt_open = 0u64;
    for_each_set_bit(lt, 0, |p| {
        let p = p as usize;
        let next = if p + 1 < valid_len {
            Some(block[p + 1])
        } else {
            next_byte
        };
        if next.is_some_and(is_tag_start_byte) {
            lt_open |= 1u64 << p;
        }
    });

    let carry_t = carry.in_tag;
    let carry_d = carry.quote == Some(b'"');
    let carry_s = carry.quote == Some(b'\'');

    // Start with every accepted `<` opening a tag and no `>` closing it.  This
    // lets the quote pass see all quote bytes; iteration 1 closes unquoted
    // `>` bytes.  The other initialization (every `>` closes) gives the same
    // fixed point on well-formed input, but this one has fewer transient
    // quote-state flips for mixed-quote attributes.
    let mut t = prefix_xor_with_carry(lt_open | gt, carry_t);
    let mut d = 0u64;
    let mut s = 0u64;
    let mut converged = false;

    for _ in 0..MAX_ITERS {
        let prev_t = prev_state(t, carry_t);
        let prev_d = prev_state(d, carry_d);
        let prev_s = prev_state(s, carry_s);

        // A double quote only opens/closes while inside a tag and not inside a
        // single-quoted value (and vice versa).  Jacobi updates (both use the
        // previous iteration's state) keep this monotone enough to converge in
        // practice.
        let d_new = prefix_xor_with_carry(dq & prev_t & !prev_s, carry_d);
        let s_new = prefix_xor_with_carry(sq & prev_t & !prev_d, carry_s);

        let prev_d_new = prev_state(d_new, carry_d);
        let prev_s_new = prev_state(s_new, carry_s);
        let lt_ok = lt_open & !prev_t;
        let gt_ok = gt & prev_t & !prev_d_new & !prev_s_new;
        let t_new = prefix_xor_with_carry(lt_ok | gt_ok, carry_t);

        if t_new == t && d_new == d && s_new == s {
            t = t_new;
            d = d_new;
            s = s_new;
            converged = true;
            break;
        }

        t = t_new;
        d = d_new;
        s = s_new;
    }

    // Overlapping double/single quote states mean our 4-state per-byte model
    // is being asked to do something it cannot represent (e.g. a quote of the
    // other kind inside a quoted value after a block boundary).  Repair that
    // block with the scalar state machine instead of trusting the fixed point.
    if !converged || (d & s & valid_mask) != 0 {
        return scan_block_scalar(block, valid_len, raw, carry, lt_open);
    }

    let prev_t = prev_state(t, carry_t);
    let prev_d = prev_state(d, carry_d);
    let prev_s = prev_state(s, carry_s);
    let lt_ok = lt_open & !prev_t & valid_mask;
    let gt_ok = gt & prev_t & !prev_d & !prev_s & valid_mask;

    BlockScan {
        lt_ok,
        gt_ok,
        carry: carry_out(t, d, s, valid_len),
        repaired: false,
    }
}

/// Candidate-only scalar state machine used when the bit-parallel fixed point
/// does not converge.
/// Candidate-only scalar repair for blocks where the bit-parallel fixed point
/// oscillates or produces overlapping quote states.
///
/// Only `<`, `>`, `"` and `'` candidates are visited; ordinary text is never
/// touched.
fn scan_block_scalar(
    block: &[u8; crate::simd::BLOCK_SIZE],
    valid_len: usize,
    raw: u64,
    carry: Carry,
    lt_open: u64,
) -> BlockScan {
    let valid_mask = valid_mask(valid_len);
    let m = classify_stage1(block);
    let candidates = (lt_open | (m.gt & !raw) | (m.dq & !raw) | (m.sq & !raw)) & valid_mask;

    let mut lt_ok = 0u64;
    let mut gt_ok = 0u64;
    let mut state = carry;

    for_each_set_bit(candidates, 0, |p| {
        let b = block[p as usize];
        let bit = 1u64 << p;
        match state.quote {
            Some(q) => {
                if b == q {
                    state.quote = None;
                }
            }
            None => {
                if state.in_tag {
                    match b {
                        b'"' => state.quote = Some(b'"'),
                        b'\'' => state.quote = Some(b'\''),
                        b'>' => {
                            state.in_tag = false;
                            gt_ok |= bit;
                        }
                        // `<` inside a tag is just a tag-name/attribute byte.
                        _ => {}
                    }
                } else if b == b'<' {
                    state.in_tag = true;
                    lt_ok |= bit;
                }
            }
        }
    });

    let (t, d, s) = states_from_scalar(state, valid_len);
    BlockScan {
        lt_ok: lt_ok & valid_mask,
        gt_ok: gt_ok & valid_mask,
        carry: carry_out(t, d, s, valid_len),
        repaired: true,
    }
}

/// Converts the scalar per-byte state into the block-end quote masks used by
/// [`carry_out`].  Only the last valid bit matters for the carried state.
fn states_from_scalar(state: Carry, valid_len: usize) -> (u64, u64, u64) {
    let last = 1u64 << (valid_len - 1);
    let t = if state.in_tag { last } else { 0 };
    let d = if state.quote == Some(b'"') { last } else { 0 };
    let s = if state.quote == Some(b'\'') { last } else { 0 };
    (t, d, s)
}

fn carry_out(t: u64, d: u64, s: u64, valid_len: usize) -> Carry {
    let last = 1u64 << (valid_len - 1);
    Carry {
        in_tag: (t & last) != 0,
        quote: if (d & last) != 0 {
            Some(b'"')
        } else if (s & last) != 0 {
            Some(b'\'')
        } else {
            None
        },
    }
}

#[inline]
fn prev_state(x: u64, carry: bool) -> u64 {
    (x << 1) | u64::from(carry)
}

#[inline]
fn valid_mask(valid_len: usize) -> u64 {
    if valid_len >= crate::simd::BLOCK_SIZE {
        u64::MAX
    } else {
        (1u64 << valid_len) - 1
    }
}

#[inline]
fn is_tag_start_byte(b: u8) -> bool {
    b.is_ascii_alphabetic() || matches!(b, b'/' | b'!' | b'?')
}

/// ORs the parts of all raw spans that intersect `[base, base + len)`.
/// Returns the raw-region bits for `[base, base + len)`.
///
/// `cursor` advances monotonically across blocks; spans that cross a block
/// boundary are split without scanning the whole span again.
fn raw_mask_for_block(spans: &[RawSpan], cursor: &mut usize, base: usize, len: usize) -> u64 {
    let block_end = base + len;
    while *cursor < spans.len() && spans[*cursor].span.end as usize <= base {
        *cursor += 1;
    }

    let mut mask = 0u64;
    let mut i = *cursor;
    while i < spans.len() {
        let start = spans[i].span.start as usize;
        let end = spans[i].span.end as usize;
        if start >= block_end {
            break;
        }
        let lo = start.max(base);
        let hi = end.min(block_end);
        if lo < hi {
            mask |= range_mask(lo - base, hi - base);
        }
        if end <= block_end {
            i += 1;
        } else {
            break;
        }
    }
    *cursor = i;
    mask
}

/// Bits `[lo, hi)` set, with `hi <= 64`.
#[inline]
/// Returns a mask with bits `[lo, hi)` set. `hi <= 64`.
fn range_mask(lo: usize, hi: usize) -> u64 {
    debug_assert!(lo < hi && hi <= 64);
    let lower = if lo == 0 { u64::MAX } else { u64::MAX << lo };
    let upper = if hi >= crate::simd::BLOCK_SIZE {
        u64::MAX
    } else {
        (1u64 << hi) - 1
    };
    lower & upper
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

/// Complement of tag spans and non-text raw spans.
/// Computes maximal text intervals as the complement of tag spans and
/// non-text raw ranges.
///
/// Rawtext content is intentionally kept as a text run; comments, doctypes and
/// processing instructions are not text.
fn build_text_runs(
    input_len: usize,
    starts: &[u32],
    ends: &[u32],
    raw_spans: &[RawSpan],
) -> Vec<Range<u32>> {
    // Both inputs are already in ascending order: stage-1 emits tag spans in
    // source order and the raw scanner emits its ranges in source order.  A
    // linear union replaces the old collect-and-sort over every tag.
    let mut raw: Vec<Range<u32>> = raw_spans
        .iter()
        .filter(|span| span.kind == RawKind::NonText)
        .map(|span| span.span.clone())
        .collect();
    raw.retain(|range| range.start < range.end);

    let mut tag_spans: Vec<Range<u32>> = Vec::with_capacity(starts.len() + 1);
    let mut i = 0usize;
    let mut j = 0usize;
    while i < starts.len() || j < ends.len() {
        let range = if i >= starts.len() {
            let range = ends[j]..ends[j] + 1;
            j += 1;
            range
        } else if j >= ends.len() {
            let range = starts[i]..input_len as u32;
            i += 1;
            range
        } else if starts[i] < ends[j] {
            let range = starts[i]..ends[j] + 1;
            i += 1;
            j += 1;
            range
        } else {
            // An end without a preceding start.  Keep it as an isolated tag
            // span; the tokenizer will decide whether to drop it.
            let range = ends[j]..ends[j] + 1;
            j += 1;
            range
        };
        tag_spans.push(range);
    }

    let mut runs = Vec::new();
    let mut cursor = 0u32;
    let (mut ri, mut ti) = (0usize, 0usize);
    while ri < raw.len() || ti < tag_spans.len() {
        let next_start = match (raw.get(ri), tag_spans.get(ti)) {
            (Some(r), Some(t)) => {
                if r.start <= t.start {
                    ri += 1;
                    r.clone()
                } else {
                    ti += 1;
                    t.clone()
                }
            }
            (Some(r), None) => {
                ri += 1;
                r.clone()
            }
            (None, Some(t)) => {
                ti += 1;
                t.clone()
            }
            (None, None) => break,
        };
        if cursor < next_start.start {
            runs.push(cursor..next_start.start);
        }
        cursor = cursor.max(next_start.end);
    }
    if (cursor as usize) < input_len {
        runs.push(cursor..input_len as u32);
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn index(input: &str) -> MarkupIndex {
        build_markup_index(input.as_bytes()).unwrap()
    }

    fn tag_names(input: &str) -> Vec<(bool, String, bool)> {
        let idx = index(input);
        idx.tag_starts
            .iter()
            .zip(&idx.tag_ends)
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
        assert_eq!(idx.tag_starts, vec![0, 6, 12, 17, 21, 28]);
        assert_eq!(idx.tag_ends, vec![5, 11, 14, 20, 27, 34]);
        assert!(idx.is_balanced());
        assert_eq!(idx.text_runs, vec![15..17]);
    }

    #[test]
    fn comments_are_not_text_or_tags() {
        let idx = index("<p>a<!-- <b> -->b</p>");
        assert_eq!(idx.tag_starts.len(), 2);
        assert_eq!(idx.text_runs, vec![3..4, 16..17]);
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
        assert_eq!(idx.tag_starts, vec![0, 27]);
        assert_eq!(idx.tag_ends, vec![25, 30]);
        assert_eq!(idx.text_runs, vec![26..27]);
        assert!(idx.is_balanced());
    }

    #[test]
    fn scalar_repair_handles_nonconvergent_quote_blocks() {
        // Two tags with a quoted `>` in one block: the cheap fixed point
        // oscillates here, so the block must go through the scalar repair.
        let input = r#"<div title="a > b">hello</div>"#.repeat(2);
        let idx = index(&input);
        assert!(idx.flags.contains(MarkupFlags::SCALAR_REPAIR), "{idx:?}");
        assert_eq!(idx.tag_starts.len(), 4, "{idx:?}");
        assert_eq!(idx.tag_ends.len(), 4, "{idx:?}");
        assert!(idx.is_balanced(), "{idx:?}");
        assert_eq!(idx.text_runs, vec![19..24, 49..54]);
    }

    #[test]
    fn tag_and_quote_span_block_boundaries() {
        let mut input = String::new();
        input.push_str(&"a".repeat(60));
        input.push_str(r#"<div title="a > b">text</div>"#);
        let idx = index(&input);
        assert_eq!(idx.tag_starts.len(), 2, "{idx:?}");
        assert!(idx.is_balanced(), "{idx:?}");
    }

    #[test]
    fn unterminated_tag_is_flagged() {
        let idx = index("<div class=x");
        assert!(idx.flags.contains(MarkupFlags::UNCLOSED_TAG));
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
        assert_eq!(idx.tag_starts.len(), 2, "{idx:?}");
        assert_eq!(idx.tag_ends.len(), 2, "{idx:?}");
        assert!(idx.raw_ranges.contains(&(64..84)), "{:?}", idx.raw_ranges);
    }

    #[test]
    fn fuzz_smoke_never_panics() {
        let mut state = 0x1234_5678_9abc_def0u64;
        for _ in 0..4096 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let len = (state as usize) & 0x7FF;
            let mut input = Vec::with_capacity(len);
            for _ in 0..len {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                input.push(state as u8);
            }
            let _ = build_markup_index(&input).unwrap();
        }
    }

    #[test]
    fn empty_input() {
        let idx = index("");
        assert_eq!(idx.tag_starts.len(), 0);
        assert_eq!(idx.text_runs.len(), 0);
        assert!(idx.is_balanced());
    }

    #[test]
    fn plain_text_input() {
        let idx = index("just some text");
        assert_eq!(idx.tag_starts.len(), 0);
        assert_eq!(idx.text_runs, vec![0..14]);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2048))]

        #[test]
        fn scanner_matches_scalar_reference(input in proptest::collection::vec(any::<u8>(), 0..512)) {
            let (raw_spans, _) = scan_raw_spans(&input);
            let (want_starts, want_ends) = scalar_reference(&input, &raw_spans);
            let got = build_markup_index(&input).unwrap();
            prop_assert_eq!(&got.tag_starts, &want_starts, "input={:?}", input);
            prop_assert_eq!(&got.tag_ends, &want_ends, "input={:?}", input);
        }
    }

    /// Independent, fully scalar stage-1 reference used only by tests/fuzzing.
    fn scalar_reference(input: &[u8], raw_spans: &[RawSpan]) -> (Vec<u32>, Vec<u32>) {
        let mut is_raw = vec![false; input.len()];
        for span in raw_spans {
            for i in span.span.start..span.span.end {
                is_raw[i as usize] = true;
            }
        }

        let mut starts = Vec::new();
        let mut ends = Vec::new();
        let mut in_tag = false;
        let mut quote = None;

        for (i, &b) in input.iter().enumerate() {
            if is_raw[i] {
                continue;
            }
            if in_tag {
                match quote {
                    Some(q) => {
                        if b == q {
                            quote = None;
                        }
                    }
                    None => match b {
                        b'"' => quote = Some(b'"'),
                        b'\'' => quote = Some(b'\''),
                        b'>' => {
                            in_tag = false;
                            ends.push(i as u32);
                        }
                        _ => {}
                    },
                }
            } else if b == b'<' && input.get(i + 1).copied().is_some_and(is_tag_start_byte) {
                in_tag = true;
                quote = None;
                starts.push(i as u32);
            }
        }

        (starts, ends)
    }
}
