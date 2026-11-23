//! aarch64 NEON kernels: byte classification over four 16-byte lanes.
//!
//! NEON is mandatory on aarch64, so this file has no runtime detection: the
//! dispatch in [`super`] selects it unconditionally on `target_arch =
//! "aarch64"`.  A `BLOCK_SIZE` block is four 16-byte loads; each delimiter class
//! costs four `cmeq` plus four 16-bit lane masks that are shifted into place.
//!
//! `prefix_xor` stays on the scalar shift chain, not PMULL.  simdjson's arm64
//! backend measured the carry-less multiply as slower because every call needs
//! a GPR -> FP move and an FP -> GPR move around it; see
//! `Refs/simdjson/include/simdjson/arm64/bitmask.h`.

use super::{BLOCK_SIZE, NEED_ALL, NEED_GT, NEED_LT, NEED_QUOTES, StructuralMasks};

/// NEON implementation of [`super::classify`].
#[must_use]
pub fn classify(block: &[u8; BLOCK_SIZE]) -> StructuralMasks {
    // SAFETY: NEON is part of the base aarch64 ISA.
    unsafe { classify_neon(block) }
}

/// NEON implementation of [`super::probe_lt`].
#[must_use]
pub fn probe_lt(block: &[u8; BLOCK_SIZE]) -> Option<StructuralMasks> {
    // SAFETY: NEON is part of the base aarch64 ISA.
    unsafe { probe_lt_neon(block) }
}

/// Inline-only entry point for the scanner's per-ISA block loop (all masks).
#[inline]
#[target_feature(enable = "neon")]
pub(crate) unsafe fn classify_inline(block: &[u8; BLOCK_SIZE]) -> StructuralMasks {
    // SAFETY: the caller has NEON enabled (this fn requires it too).
    unsafe { classify_impl(block, NEED_ALL) }
}

/// Inline-only probe for the text/rawtext states.
#[inline]
#[target_feature(enable = "neon")]
pub(crate) unsafe fn probe_lt_inline(block: &[u8; BLOCK_SIZE]) -> Option<StructuralMasks> {
    // SAFETY: the caller has NEON enabled (this fn requires it too).
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

#[target_feature(enable = "neon")]
unsafe fn classify_neon(block: &[u8; BLOCK_SIZE]) -> StructuralMasks {
    // SAFETY: this function has NEON enabled.
    unsafe { classify_impl(block, NEED_ALL) }
}

#[target_feature(enable = "neon")]
unsafe fn probe_lt_neon(block: &[u8; BLOCK_SIZE]) -> Option<StructuralMasks> {
    // SAFETY: this function has NEON enabled.
    unsafe { probe_lt_inline(block) }
}

#[target_feature(enable = "neon")]
unsafe fn lt_mask(block: &[u8; BLOCK_SIZE]) -> u64 {
    use std::arch::aarch64::{uint8x16_t, vceqq_u8, vdupq_n_u8, vld1q_u8};

    // SAFETY: NEON is enabled; the block is 64 valid bytes.
    unsafe {
        let ptr = block.as_ptr();
        let needle = vdupq_n_u8(b'<');
        let m = |v: uint8x16_t| movemask_u16(vceqq_u8(v, needle)) as u64;
        m(vld1q_u8(ptr))
            | (m(vld1q_u8(ptr.add(16))) << 16)
            | (m(vld1q_u8(ptr.add(32))) << 32)
            | (m(vld1q_u8(ptr.add(48))) << 48)
    }
}

#[target_feature(enable = "neon")]
unsafe fn classify_impl(block: &[u8; BLOCK_SIZE], need: u8) -> StructuralMasks {
    use std::arch::aarch64::{vceqq_u8, vdupq_n_u8, vld1q_u8};

    let mut m = StructuralMasks::default();

    // SAFETY: NEON is enabled; the block is 64 valid bytes; unaligned loads
    // are permitted.
    unsafe {
        let ptr = block.as_ptr();
        let v0 = vld1q_u8(ptr);
        let v1 = vld1q_u8(ptr.add(16));
        let v2 = vld1q_u8(ptr.add(32));
        let v3 = vld1q_u8(ptr.add(48));

        macro_rules! mask {
            ($b:expr) => {{
                let needle = vdupq_n_u8($b);
                (movemask_u16(vceqq_u8(v0, needle)) as u64)
                    | ((movemask_u16(vceqq_u8(v1, needle)) as u64) << 16)
                    | ((movemask_u16(vceqq_u8(v2, needle)) as u64) << 32)
                    | ((movemask_u16(vceqq_u8(v3, needle)) as u64) << 48)
            }};
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

/// Emulates x86 `_mm_movemask_epi8` for one 16-byte all-ones/all-zeros vector.
///
/// `AND` with `1,2,4,...,128` turns lane `i` into `2^(i%8)`, then the two
/// 8-byte halves are reduced separately with `vaddv_u8`.  The halves cannot
/// collide because each reduction sees exactly one of every bit weight
/// (the sum is at most `255`, so the `u8` result is exact) and they are
/// combined at different bit positions.
///
/// This is public because the selector's dense `tag_code` scan in `simdsel`
/// needs the same 16-bit lane mask that the block classifier builds.
///
/// # Safety
///
/// The CPU must support NEON (guaranteed on aarch64).
#[inline]
#[target_feature(enable = "neon")]
pub unsafe fn movemask_u16(v: std::arch::aarch64::uint8x16_t) -> u16 {
    use std::arch::aarch64::{vaddv_u8, vandq_u8, vget_high_u8, vget_low_u8, vld1q_u8};

    const WEIGHTS: [u8; 16] = [1, 2, 4, 8, 16, 32, 64, 128, 1, 2, 4, 8, 16, 32, 64, 128];

    // SAFETY: NEON is enabled; WEIGHTS has 16 readable bytes.
    unsafe {
        let bits = vandq_u8(v, vld1q_u8(WEIGHTS.as_ptr()));
        let lo = u16::from(vaddv_u8(vget_low_u8(bits)));
        let hi = u16::from(vaddv_u8(vget_high_u8(bits)));
        lo | (hi << 8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neon_classify_matches_scalar() {
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
    fn neon_probe_lt_matches_scalar() {
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

    #[test]
    fn movemask_matches_bit_positions() {
        let mut block = [0u8; BLOCK_SIZE];
        for (i, b) in block.iter_mut().enumerate() {
            *b = if i % 3 == 0 { b'<' } else { b'x' };
        }
        let m = classify(&block);
        let expected: u64 = (0..BLOCK_SIZE as u64)
            .filter(|i| i % 3 == 0)
            .fold(0, |acc, i| acc | (1 << i));
        assert_eq!(m.lt, expected);
    }
}
