//! Low-level SIMD / bit-parallel primitives for the stage-1 scanner.
//!
//! This module is deliberately small and dependency-light.  It is the
//! foundation for the HTML stage-1 scanner described in `ARCHITECTURE.zh.md`:
//!
//! * 64-byte block reading ([`block`]);
//! * SIMD byte classification into 64-bit masks ([`classify`]);
//! * `prefix_xor` via PCLMULQDQ (or a portable shift/xor fallback)
//!   ([`prefix_xor()`]);
//! * bit-set iteration for flattening masks into indices ([`bit_index`]);
//! * UTF-8 validation ([`utf8`]).
//!
//! Everything has a scalar reference implementation.  x86_64 gets an AVX2
//! classifier and a PCLMULQDQ `prefix_xor`, selected at runtime; other
//! targets simply use the scalar versions.

pub mod bit_index;
pub mod block;
pub mod classify;
pub mod prefix_xor;
pub mod utf8;

pub use bit_index::{collect_indices, for_each_set_bit};
pub use block::{BLOCK_SIZE, BlockReader};
pub use classify::{HotMasks, classify_stage1};
pub use prefix_xor::{prefix_xor, prefix_xor_with_carry};
pub use utf8::{Utf8Error, validate_utf8};

/// Target features selected at runtime by this build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Features {
    /// AVX2 classifier is available and selected.
    pub avx2: bool,
    /// PCLMULQDQ `prefix_xor` is available and selected.
    pub pclmulqdq: bool,
}

/// Returns the runtime-selected features of the current CPU.
#[must_use]
pub fn features() -> Features {
    #[cfg(target_arch = "x86_64")]
    {
        Features {
            avx2: std::is_x86_feature_detected!("avx2"),
            pclmulqdq: std::is_x86_feature_detected!("pclmulqdq"),
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        Features {
            avx2: false,
            pclmulqdq: false,
        }
    }
}
