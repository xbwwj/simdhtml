use super::{BLOCK_SIZE, NEED_ALL, NEED_GT, NEED_LT, NEED_QUOTES, StructuralMasks};

/// AVX-512 [`super::classify`].
pub fn classify(block: &[u8; BLOCK_SIZE]) -> StructuralMasks {
    unsafe { classify_avx512(block) }
}

/// AVX-512 [`super::probe_lt`].
pub fn probe_lt(block: &[u8; BLOCK_SIZE]) -> Option<StructuralMasks> {
    unsafe { probe_lt_avx512(block) }
}

/// Inline-only entry point for the scanner's block loop (all masks).
#[inline]
#[target_feature(enable = "avx512f,avx512bw")]
pub(crate) unsafe fn classify_inline(block: &[u8; BLOCK_SIZE]) -> StructuralMasks {
    unsafe { classify_impl(block, NEED_ALL) }
}

/// Inline-only probe for the text/rawtext states.
#[inline]
#[target_feature(enable = "avx512f,avx512bw")]
pub(crate) unsafe fn probe_lt_inline(block: &[u8; BLOCK_SIZE]) -> Option<StructuralMasks> {
    unsafe {
        let lt = lt_mask(block);
        if lt == 0 {
            return None;
        }
        let mut masks = classify_impl(block, NEED_ALL & !NEED_LT);
        masks.lt = lt;
        Some(masks)
    }
}

#[target_feature(enable = "avx512f,avx512bw")]
unsafe fn classify_avx512(block: &[u8; BLOCK_SIZE]) -> StructuralMasks {
    unsafe { classify_impl(block, NEED_ALL) }
}

#[target_feature(enable = "avx512f,avx512bw")]
unsafe fn probe_lt_avx512(block: &[u8; BLOCK_SIZE]) -> Option<StructuralMasks> {
    unsafe { probe_lt_inline(block) }
}

#[target_feature(enable = "avx512f,avx512bw")]
unsafe fn lt_mask(block: &[u8; BLOCK_SIZE]) -> u64 {
    use std::arch::x86_64::{
        __m512i, _mm512_cmpeq_epi8_mask, _mm512_loadu_si512, _mm512_set1_epi8,
    };

    // SAFETY: the block is 64 valid bytes.
    unsafe {
        let v = _mm512_loadu_si512(block.as_ptr().cast::<__m512i>());
        _mm512_cmpeq_epi8_mask(v, _mm512_set1_epi8(b'<' as i8))
    }
}

#[target_feature(enable = "avx512f,avx512bw")]
unsafe fn classify_impl(block: &[u8; BLOCK_SIZE], need: u8) -> StructuralMasks {
    use std::arch::x86_64::{
        __m512i, _mm512_cmpeq_epi8_mask, _mm512_loadu_si512, _mm512_set1_epi8,
    };

    let mut m = StructuralMasks::default();

    // SAFETY: the block is 64 valid bytes; unaligned loads are permitted.
    unsafe {
        let v = _mm512_loadu_si512(block.as_ptr().cast::<__m512i>());

        macro_rules! mask {
            ($b:expr) => {
                _mm512_cmpeq_epi8_mask(v, _mm512_set1_epi8($b as i8))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn have_avx512() -> bool {
        std::is_x86_feature_detected!("avx512f") && std::is_x86_feature_detected!("avx512bw")
    }

    #[test]
    fn avx512_classify_matches_scalar() {
        if !have_avx512() {
            eprintln!("AVX-512BW not available; skipping");
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
    fn avx512_probe_lt_matches_scalar() {
        if !have_avx512() {
            eprintln!("AVX-512BW not available; skipping");
            return;
        }
        let mut state = 0x0bad_f00d_cafe_babeu64;
        for _ in 0..4096 {
            let mut block = [0u8; BLOCK_SIZE];
            for b in &mut block {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *b = state as u8;
            }
            assert_eq!(probe_lt(&block), super::super::fallback::probe_lt(&block));
        }
    }
}
