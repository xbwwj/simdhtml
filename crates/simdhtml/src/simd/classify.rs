//! HTML byte classification into 64-bit masks.
//!
//! 将连续的 64 byte HTML, 转换为若干个 u64 位图
//!
//! 本文件中，scanner 只需要处理一系列前缀 bit.
//!
//! Stage-1 only needs four predicates:
//! `<`, `>`, `"`, `'`.  Tag-start detection is done later, only at the sparse
//! bytes after `<`; attribute/whitespace predicates are intentionally not
//! computed for the whole block.
//!
//! Each predicate is a `u64`; bit `i` corresponds to byte `i` in the 64-byte
//! block.  The AVX2 implementation is selected once at runtime via
//! [`OnceLock`]; the scalar implementation is the reference and the fallback.

use std::sync::OnceLock;

/// The subset of masks consumed by stage-1.
///
/// `HotMasks` intentionally contains **only** the structural characters that
/// the bit-parallel in-tag/quote fixed point needs.  In particular it does not
/// classify letters: tag-start detection is done only for the (sparse) bytes
/// immediately after `<`, not for every byte in the block.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HotMasks {
    /// `<`
    pub lt: u64,
    /// `>`
    pub gt: u64,
    /// `"`
    pub dq: u64,
    /// `'`
    pub sq: u64,
}

type ClassifyStage1Fn = fn(&[u8; 64]) -> HotMasks;
static CLASSIFY_STAGE1: OnceLock<ClassifyStage1Fn> = OnceLock::new();

/// Classifies one block for stage-1, using AVX2 when available.
#[must_use]
pub fn classify_stage1(block: &[u8; 64]) -> HotMasks {
    let f = *CLASSIFY_STAGE1.get_or_init(select_stage1_classifier);
    f(block)
}

fn select_stage1_classifier() -> ClassifyStage1Fn {
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx2") {
            return classify_stage1_avx2_dispatch;
        }
    }
    classify_stage1_scalar
}

#[cfg(target_arch = "x86_64")]
fn classify_stage1_avx2_dispatch(block: &[u8; 64]) -> HotMasks {
    // SAFETY: `select_stage1_classifier` checked AVX2.
    unsafe { classify_stage1_avx2(block) }
}

/// Scalar reference implementation for [`classify_stage1`].
#[must_use]
pub fn classify_stage1_scalar(block: &[u8; 64]) -> HotMasks {
    let mut m = HotMasks::default();
    for (i, &b) in block.iter().enumerate() {
        let bit = 1u64 << i;
        match b {
            b'<' => m.lt |= bit,
            b'>' => m.gt |= bit,
            b'"' => m.dq |= bit,
            b'\'' => m.sq |= bit,
            _ => {}
        }
    }
    m
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn classify_stage1_avx2(block: &[u8; 64]) -> HotMasks {
    use std::arch::x86_64::{
        __m256i, _mm256_cmpeq_epi8, _mm256_loadu_si256, _mm256_movemask_epi8, _mm256_set1_epi8,
    };

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

        HotMasks {
            lt: mask!(b'<'),
            gt: mask!(b'>'),
            dq: mask!(b'"'),
            sq: mask!(b'\''),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage1_avx2_matches_scalar() {
        if !std::is_x86_feature_detected!("avx2") {
            eprintln!("AVX2 not available; skipping");
            return;
        }
        let mut state = 0x1234_5678_9abc_def0u64;
        for _ in 0..4096 {
            let mut block = [0u8; 64];
            for b in &mut block {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *b = state as u8;
            }
            // SAFETY: checked AVX2 above.
            let avx2 = unsafe { classify_stage1_avx2(&block) };
            assert_eq!(avx2, classify_stage1_scalar(&block));
        }
    }
}
