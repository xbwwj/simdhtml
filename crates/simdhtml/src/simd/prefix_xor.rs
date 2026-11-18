//! `prefix_xor`: cumulative XOR over the bits of a `u64`.
//!
//! For `x = 0b00100100`, `prefix_xor(x) = 0b00011100`: bit `i` of the result
//! is the parity of `x` up to and including `i`.  This is the bit-parallel
//! trick used by simdjson for quote/string masks, and it is also what we use
//! for the `in_tag` fixed point.
//!
//! x86_64 uses PCLMULQDQ (carry-less multiply by all-ones) when available;
//! otherwise a five-step shift/xor fallback is used.

use std::sync::OnceLock;

type PrefixXorFn = fn(u64) -> u64;

static PREFIX_XOR: OnceLock<PrefixXorFn> = OnceLock::new();

/// Returns the cumulative XOR of the bits of `x`.
#[must_use]
pub fn prefix_xor(x: u64) -> u64 {
    let f = *PREFIX_XOR.get_or_init(select_prefix_xor);
    f(x)
}

/// Like [`prefix_xor`], but flips the result when `carry` is true.
///
/// This is the block-boundary carry: if the previous block ended with a set
/// state, every bit in this block is inverted.
#[inline]
#[must_use]
pub fn prefix_xor_with_carry(x: u64, carry: bool) -> u64 {
    let p = prefix_xor(x);
    if carry { !p } else { p }
}

fn select_prefix_xor() -> PrefixXorFn {
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("pclmulqdq") {
            return prefix_xor_clmul_dispatch;
        }
    }
    prefix_xor_scalar
}

#[cfg(target_arch = "x86_64")]
fn prefix_xor_clmul_dispatch(x: u64) -> u64 {
    // SAFETY: selected only after checking PCLMULQDQ.
    unsafe { prefix_xor_clmul(x) }
}

/// Portable five-step implementation.
#[must_use]
pub fn prefix_xor_scalar(mut x: u64) -> u64 {
    x ^= x << 1;
    x ^= x << 2;
    x ^= x << 4;
    x ^= x << 8;
    x ^= x << 16;
    x ^= x << 32;
    x
}

#[cfg(target_arch = "x86_64")]
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

    fn naive(x: u64) -> u64 {
        let mut out = 0u64;
        let mut parity = 0u8;
        for i in 0..64 {
            parity ^= ((x >> i) & 1) as u8;
            if parity != 0 {
                out |= 1u64 << i;
            }
        }
        out
    }

    #[test]
    fn known_values() {
        assert_eq!(prefix_xor_scalar(0), 0);
        assert_eq!(prefix_xor_scalar(1), u64::MAX);
        assert_eq!(prefix_xor_scalar(0b0010_0100), 0b0001_1100);
        assert_eq!(prefix_xor_scalar(u64::MAX), 0x5555_5555_5555_5555);
    }

    #[test]
    fn scalar_matches_naive() {
        let mut state = 0x1234_5678_9abc_def0u64;
        for _ in 0..10_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            assert_eq!(prefix_xor_scalar(state), naive(state));
        }
    }

    #[cfg(target_arch = "x86_64")]
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
            // SAFETY: checked above.
            let got = unsafe { prefix_xor_clmul(state) };
            assert_eq!(got, prefix_xor_scalar(state));
        }
    }

    #[test]
    fn carry_flips_all_bits() {
        let x = 0b1010_0101u64;
        assert_eq!(prefix_xor_with_carry(x, false), prefix_xor_scalar(x));
        assert_eq!(prefix_xor_with_carry(x, true), !prefix_xor_scalar(x));
    }

    #[test]
    fn dispatch_matches_scalar() {
        let mut state = 0x0bad_f00d_1234_5678u64;
        for _ in 0..4096 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            assert_eq!(prefix_xor(state), prefix_xor_scalar(state));
        }
    }
}
