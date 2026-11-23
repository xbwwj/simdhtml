use super::{BLOCK_SIZE, StructuralMasks};

/// AVX2 implementation of [`super::classify`].
#[must_use]
pub fn classify(block: &[u8; BLOCK_SIZE]) -> StructuralMasks {
    // SAFETY: installed in the dispatch table only after the AVX2 check.
    unsafe { classify_markup_avx2(block) }
}

/// PCLMULQDQ implementation of [`super::prefix_xor`].
#[must_use]
pub fn prefix_xor(x: u64) -> u64 {
    // SAFETY: installed in the dispatch table only after the PCLMULQDQ check.
    unsafe { prefix_xor_clmul(x) }
}

/// AVX2 implementation of [`super::probe_lt`].
#[must_use]
pub fn probe_lt(block: &[u8; BLOCK_SIZE]) -> Option<StructuralMasks> {
    // SAFETY: installed in the dispatch table only after the AVX2 check.
    unsafe { probe_lt_inline(block) }
}

/// Probe for the text/rawtext states, which only look at `<`: `None` when the
/// block has none, all masks otherwise (a `<` may open a tag whose `>` and
/// quotes are in the same block).  The mask set is a compile-time constant: a
/// runtime `need` argument measured slower than classifying everything.
#[inline]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn probe_lt_inline(block: &[u8; BLOCK_SIZE]) -> Option<StructuralMasks> {
    use super::{NEED_ALL, NEED_LT};
    // SAFETY: the caller has AVX2 enabled (this fn requires it too).
    unsafe {
        let lt = classify_impl(block, NEED_LT).lt;
        if lt == 0 {
            return None;
        }
        Some(StructuralMasks {
            lt,
            ..classify_impl(block, NEED_ALL & !NEED_LT)
        })
    }
}

/// Inline-only entry point for the scanner's per-ISA block loop (all masks).
///
/// Calling this from a `#[target_feature(enable = "avx2")]` function lets the
/// classifier be inlined and its constants hoisted out of the loop; the safe
/// [`classify`] wrapper exists only for the per-block function-pointer dispatch
/// used by tests and the kernel benchmark.
#[inline]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn classify_inline(block: &[u8; BLOCK_SIZE]) -> StructuralMasks {
    // SAFETY: the caller has AVX2 enabled (this fn requires it too).
    unsafe { classify_markup_avx2(block) }
}

#[target_feature(enable = "avx2")]
unsafe fn classify_markup_avx2(block: &[u8; BLOCK_SIZE]) -> StructuralMasks {
    // SAFETY: this function has AVX2 enabled.
    unsafe { classify_impl(block, super::NEED_ALL) }
}

/// Classifies only the masks selected by `need`.
///
/// The scan asks for one mask in text/rawtext (`<`) and in comments (`>`), and
/// for `>`/quotes only inside a tag.  Classifying all four everywhere cost
/// 4× the necessary `cmpeq`+`movemask` on the ~48 % of blocks that are pure
/// rawtext or plain text.
#[target_feature(enable = "avx2")]
unsafe fn classify_impl(block: &[u8; BLOCK_SIZE], need: u8) -> StructuralMasks {
    use super::{NEED_GT, NEED_LT, NEED_QUOTES};
    use std::arch::x86_64::{
        __m256i, _mm256_cmpeq_epi8, _mm256_loadu_si256, _mm256_movemask_epi8, _mm256_set1_epi8,
    };

    let mut m = StructuralMasks::default();

    // SAFETY: AVX2 is enabled; the block is 64 valid bytes; unaligned loads
    // are permitted.
    unsafe {
        let lo = _mm256_loadu_si256(block.as_ptr().cast::<__m256i>());
        let hi = _mm256_loadu_si256(block.as_ptr().add(32).cast::<__m256i>());

        #[inline(always)]
        unsafe fn eq_mask(v: __m256i, byte: u8) -> u32 {
            // SAFETY: called from an AVX2 context.
            unsafe {
                _mm256_movemask_epi8(_mm256_cmpeq_epi8(v, _mm256_set1_epi8(byte as i8))) as u32
            }
        }

        macro_rules! mask {
            ($b:expr) => {
                (eq_mask(lo, $b) as u64) | ((eq_mask(hi, $b) as u64) << 32)
            };
        }

        if need & NEED_LT != 0 {
            m.lt = mask!(b'<');
        }
        if need & NEED_GT != 0 {
            m.gt = mask!(b'>');
        }
        if need & NEED_QUOTES != 0 {
            m.dq = mask!(b'"');
            m.sq = mask!(b'\'');
        }
    }
    m
}

#[target_feature(enable = "pclmulqdq")]
unsafe fn prefix_xor_clmul(x: u64) -> u64 {
    use std::arch::x86_64::{
        __m128i, _mm_clmulepi64_si128, _mm_cvtsi128_si64, _mm_set_epi64x, _mm_set1_epi8,
    };

    // The enclosing `target_feature` guarantees PCLMULQDQ; the intrinsic is
    // safe to call in this context.
    let all_ones = _mm_set1_epi8(-1);
    let input: __m128i = _mm_set_epi64x(0, x as i64);
    let product = _mm_clmulepi64_si128(input, all_ones, 0);
    _mm_cvtsi128_si64(product) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avx2_classify_matches_scalar() {
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("AVX2 not available; skipping");
            return;
        }
        let mut state = 0x1234_5678_9abc_def0u64;
        for _ in 0..4096 {
            let mut block = [0u8; BLOCK_SIZE];
            for b in &mut block {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *b = state as u8;
            }
            assert_eq!(classify(&block), super::super::fallback::classify(&block));
        }
    }

    #[test]
    fn clmul_matches_scalar() {
        if !std::is_x86_feature_detected!("pclmulqdq") {
            eprintln!("PCLMULQDQ not available; skipping");
            return;
        }
        let mut state = 0xdead_beef_cafe_babeu64;
        for _ in 0..10_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            assert_eq!(prefix_xor(state), super::super::fallback::prefix_xor(state));
        }
    }
}
