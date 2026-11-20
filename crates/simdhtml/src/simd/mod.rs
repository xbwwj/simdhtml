//! Low-level SIMD / bit-parallel kernels for the markup scanner.
//!
//! The layout follows the one axis that makes a kernel module worth splitting:
//! the **target feature level**, exactly like simdjson's per-ISA directories.
//! `fallback` is the portable reference implementation (simdjson's
//! `fallback/`); `x86` is the AVX2 / PCLMULQDQ version of the *same* kernels.
//! Adding an ISA means adding one file and one line in `implementation()`,
//! not editing every kernel.
//!
//! * [`fallback`] — portable reference implementations.
//! * [`x86`] — AVX2 byte classification and PCLMULQDQ `prefix_xor`.
//!
//! The kernel ABI is deliberately tiny: [`StructuralMasks`] (four 64-bit byte masks
//! per 64-byte block) and [`prefix_xor`].  Everything else the scan needs —
//! block iteration, bit-set flattening, input-stream validation — is portable
//! and lives in `crate::markup`.

pub mod fallback;
#[cfg(target_arch = "x86_64")]
pub mod x86;

use std::sync::OnceLock;

/// Bytes processed per block, and the width of every mask in this module.
pub const BLOCK_SIZE: usize = 64;

/// Mask-request bit for `<`.
pub const NEED_LT: u8 = 1 << 0;
/// Mask-request bit for `>`.
pub const NEED_GT: u8 = 1 << 1;
/// Mask-request bit for the two quote bytes (both are produced together).
pub const NEED_QUOTES: u8 = 1 << 2;
/// Every mask.
pub const NEED_ALL: u8 = NEED_LT | NEED_GT | NEED_QUOTES;

/// The subset of masks consumed by the markup scan.
///
/// `StructuralMasks` deliberately contains **no letter classes and no
/// whitespace**: the block classifier only marks the bytes the scan branches
/// on.  Extra classes were measured and rejected: a whitespace mask costs more
/// per block than the short tag-name byte loop it would replace, and a bitmap
/// per letter never pays for itself (see `DESIGN.md`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StructuralMasks {
    /// `<`
    pub lt: u64,
    /// `>`
    pub gt: u64,
    /// `"`
    pub dq: u64,
    /// `'`
    pub sq: u64,
}

/// One complete set of kernels for the selected target feature level.
///
/// Selecting the whole set once (instead of one `OnceLock` per kernel) keeps
/// capability detection in a single place, so `is_x86_feature_detected!` is
/// evaluated once for the process and [`features`] reports exactly what is
/// actually being called.
struct Implementation {
    classify: fn(&[u8; BLOCK_SIZE]) -> StructuralMasks,
    prefix_xor: fn(u64) -> u64,
    avx2: bool,
    pclmulqdq: bool,
}

#[cfg(not(target_arch = "x86_64"))]
const FALLBACK: Implementation = Implementation {
    classify: fallback::classify,
    prefix_xor: fallback::prefix_xor,
    avx2: false,
    pclmulqdq: false,
};

static IMPLEMENTATION: OnceLock<Implementation> = OnceLock::new();

#[inline]
fn implementation() -> &'static Implementation {
    IMPLEMENTATION.get_or_init(detect)
}

/// Picks the fastest available kernel set.
///
/// The two kernels are selected independently: AVX2 and PCLMULQDQ are separate
/// CPU features, and a build may legitimately have one without the other.
fn detect() -> Implementation {
    #[cfg(target_arch = "x86_64")]
    {
        let avx2 = std::is_x86_feature_detected!("avx2");
        let pclmulqdq = std::is_x86_feature_detected!("pclmulqdq");
        Implementation {
            classify: if avx2 {
                x86::classify
            } else {
                fallback::classify
            },
            prefix_xor: if pclmulqdq {
                x86::prefix_xor
            } else {
                fallback::prefix_xor
            },
            avx2,
            pclmulqdq,
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        FALLBACK
    }
}

/// Whether the AVX2 block loop can be used.
///
/// Called once per document, so the `OnceLock` read is negligible; the point is
/// that the scan instantiates its whole block loop for AVX2 rather than calling
/// a classifier through a function pointer for every block.
#[must_use]
#[inline]
pub fn avx2_available() -> bool {
    implementation().avx2
}

/// Classifies one block for the markup scan, using AVX2 when available.
#[must_use]
#[inline]
pub fn classify(block: &[u8; BLOCK_SIZE]) -> StructuralMasks {
    (implementation().classify)(block)
}

/// Returns the cumulative XOR of the bits of `x`.
#[must_use]
#[inline]
pub fn prefix_xor(x: u64) -> u64 {
    (implementation().prefix_xor)(x)
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

/// Target features selected at runtime by this build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstructionSets {
    /// AVX2 classifier is available and selected.
    pub avx2: bool,
    /// PCLMULQDQ `prefix_xor` is available and selected.
    pub pclmulqdq: bool,
}

/// Returns the runtime-selected features of the current CPU.
#[must_use]
pub fn instruction_sets() -> InstructionSets {
    let k = implementation();
    InstructionSets {
        avx2: k.avx2,
        pclmulqdq: k.pclmulqdq,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_classify_matches_scalar() {
        let mut state = 0x1234_5678_9abc_def0u64;
        for _ in 0..4096 {
            let mut block = [0u8; BLOCK_SIZE];
            for b in &mut block {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *b = state as u8;
            }
            assert_eq!(classify(&block), fallback::classify(&block));
        }
    }

    #[test]
    fn dispatch_prefix_xor_matches_scalar() {
        let mut state = 0x0bad_f00d_1234_5678u64;
        for _ in 0..4096 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            assert_eq!(prefix_xor(state), fallback::prefix_xor(state));
        }
    }

    #[test]
    fn carry_flips_all_bits() {
        let x = 0b1010_0101u64;
        assert_eq!(prefix_xor_with_carry(x, false), fallback::prefix_xor(x));
        assert_eq!(prefix_xor_with_carry(x, true), !fallback::prefix_xor(x));
    }

    #[test]
    fn features_match_selection() {
        let f = instruction_sets();
        #[cfg(target_arch = "x86_64")]
        {
            assert_eq!(f.avx2, std::is_x86_feature_detected!("avx2"));
            assert_eq!(f.pclmulqdq, std::is_x86_feature_detected!("pclmulqdq"));
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            assert!(!f.avx2);
            assert!(!f.pclmulqdq);
        }
    }
}
