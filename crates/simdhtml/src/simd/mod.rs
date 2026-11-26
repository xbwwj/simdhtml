//! Byte-classification kernels for the markup scanner.
//!
//! `avx512`, `x86` (AVX2 + PCLMULQDQ) and `neon` implement the same
//! `classify` / `probe_lt` / `prefix_xor` that `fallback` defines portably.

use std::sync::OnceLock;

#[cfg(target_arch = "x86_64")]
pub mod avx2;
#[cfg(target_arch = "x86_64")]
pub mod avx512;
pub mod fallback;
#[cfg(target_arch = "aarch64")]
pub mod neon;

/// Bytes per block, and the width of every mask here.
pub const BLOCK_SIZE: usize = 64;
// Mask-request bit for `<`.
pub const NEED_LT: u8 = 1 << 0;
// Mask-request bit for `>`.
pub const NEED_GT: u8 = 1 << 1;
// Mask-request bit for both quote bytes (produced together).
pub const NEED_QUOTES: u8 = 1 << 2;
// Every mask.
pub const NEED_ALL: u8 = NEED_LT | NEED_GT | NEED_QUOTES;

/// Probe the machine simd support on first usage.
static IMPLEMENTATION: OnceLock<Implementation> = OnceLock::new();

struct Implementation {
    classify: fn(&[u8; BLOCK_SIZE]) -> StructuralMasks,
    probe_lt: fn(&[u8; BLOCK_SIZE]) -> Option<StructuralMasks>,
    prefix_xor: fn(u64) -> u64,
    avx512: bool,
    avx2: bool,
    pclmulqdq: bool,
    neon: bool,
}

#[inline]
fn implementation() -> &'static Implementation {
    IMPLEMENTATION.get_or_init(detect)
}

/// Picks the fastest available kernel set.
fn detect() -> Implementation {
    #[cfg(target_arch = "x86_64")]
    {
        let avx512 =
            std::is_x86_feature_detected!("avx512f") && std::is_x86_feature_detected!("avx512bw");
        let avx2 = std::is_x86_feature_detected!("avx2");
        let pclmulqdq = std::is_x86_feature_detected!("pclmulqdq");

        Implementation {
            classify: if avx512 {
                avx512::classify
            } else if avx2 {
                avx2::classify
            } else {
                fallback::classify
            },
            probe_lt: if avx512 {
                avx512::probe_lt
            } else if avx2 {
                avx2::probe_lt
            } else {
                fallback::probe_lt
            },
            prefix_xor: if pclmulqdq {
                avx2::prefix_xor
            } else {
                fallback::prefix_xor
            },
            avx512,
            avx2,
            pclmulqdq,
            neon: false,
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // NEON is part of the base aarch64 ISA; there is nothing to detect.
        Implementation {
            classify: neon::classify,
            probe_lt: neon::probe_lt,
            // simdjson's arm64 backend measured PMULL as slower than the
            // six-shift scalar chain: the GPR<->FP transfers dominate.
            prefix_xor: fallback::prefix_xor,
            avx512: false,
            avx2: false,
            pclmulqdq: false,
            neon: true,
        }
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        FALLBACK
    }
}

/// The masks the markup scan consumes: no letter classes and no whitespace,
/// because only the bytes the scan branches on are classified.  A whitespace
/// mask measured slower than the short tag-name byte loop it would replace.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StructuralMasks {
    pub lt: u64,
    pub gt: u64,
    pub dq: u64,
    pub sq: u64,
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
const FALLBACK: Implementation = Implementation {
    classify: fallback::classify,
    probe_lt: fallback::probe_lt,
    prefix_xor: fallback::prefix_xor,
    avx512: false,
    avx2: false,
    pclmulqdq: false,
    neon: false,
};

// Whether the AVX-512 block loop can be used.
#[must_use]
#[inline]
pub fn avx512_available() -> bool {
    implementation().avx512
}

// Whether the AVX2 block loop can be used.  Called once per document: the scan
// instantiates its whole block loop per ISA instead of calling a classifier
// through a function pointer per block.
#[must_use]
#[inline]
pub fn avx2_available() -> bool {
    implementation().avx2
}

// Whether the NEON block loop can be used.
#[must_use]
#[inline]
pub fn neon_available() -> bool {
    implementation().neon
}

/// Classifies one block, using the selected ISA.
#[must_use]
#[inline]
pub fn classify(block: &[u8; BLOCK_SIZE]) -> StructuralMasks {
    (implementation().classify)(block)
}

/// Classifies only `<`, returning `None` when the block has none: the
/// text/rawtext fast path.
#[must_use]
#[inline]
pub fn probe_lt(block: &[u8; BLOCK_SIZE]) -> Option<StructuralMasks> {
    (implementation().probe_lt)(block)
}

/// Cumulative XOR of the bits of `x`.
#[must_use]
#[inline]
pub fn prefix_xor(x: u64) -> u64 {
    (implementation().prefix_xor)(x)
}

/// [`prefix_xor`], inverted when `carry` is set: the block-boundary carry.
#[inline]
#[must_use]
pub fn prefix_xor_with_carry(x: u64, carry: bool) -> u64 {
    let p = prefix_xor(x);
    if carry { !p } else { p }
}

/// Target features selected at runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstructionSets {
    pub avx512: bool,
    pub avx2: bool,
    pub pclmulqdq: bool,
    pub neon: bool,
}

/// Runtime-selected features of the current CPU.
#[must_use]
pub fn instruction_sets() -> InstructionSets {
    let k = implementation();
    InstructionSets {
        avx512: k.avx512,
        avx2: k.avx2,
        pclmulqdq: k.pclmulqdq,
        neon: k.neon,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert_eq!(
                f.avx512,
                std::is_x86_feature_detected!("avx512f")
                    && std::is_x86_feature_detected!("avx512bw")
            );
            assert_eq!(f.avx2, std::is_x86_feature_detected!("avx2"));
            assert_eq!(f.pclmulqdq, std::is_x86_feature_detected!("pclmulqdq"));
            assert!(!f.neon);
        }
        #[cfg(target_arch = "aarch64")]
        {
            assert!(f.neon);
            assert!(!f.avx512);
            assert!(!f.avx2);
            assert!(!f.pclmulqdq);
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            assert!(!f.avx512);
            assert!(!f.avx2);
            assert!(!f.pclmulqdq);
            assert!(!f.neon);
        }
    }
}
