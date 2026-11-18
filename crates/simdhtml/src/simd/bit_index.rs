//! Flattening a bit mask into ascending indices.
//!
//! Stage-1 uses this to turn `tag_starts` / `tag_ends` masks into a compact
//! `Vec<u32>` of byte offsets.  The loop is intentionally simple: the mask is
//! usually sparse (tags are a few percent of bytes), so `trailing_zeros` +
//! `bits &= bits - 1` is the right primitive.

/// Calls `f(index)` for every set bit in `bits`, in ascending order.
///
/// `base` is added to the bit position, so a block mask can be flattened
/// directly into absolute input offsets.
#[inline]
pub fn for_each_set_bit(mut bits: u64, base: u32, mut f: impl FnMut(u32)) {
    while bits != 0 {
        let offset = bits.trailing_zeros();
        f(base + offset);
        bits &= bits - 1;
    }
}

/// Appends every set bit in `bits` (plus `base`) to `out`.
#[inline]
pub fn collect_indices(bits: u64, base: u32, out: &mut Vec<u32>) {
    for_each_set_bit(bits, base, |i| out.push(i));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty() {
        let mut out = Vec::new();
        collect_indices(0, 100, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn ascending_order() {
        let bits = (1u64 << 0) | (1u64 << 3) | (1u64 << 63);
        let mut out = Vec::new();
        collect_indices(bits, 64, &mut out);
        assert_eq!(out, vec![64, 67, 127]);
    }

    #[test]
    fn all_bits() {
        let mut out = Vec::new();
        collect_indices(u64::MAX, 0, &mut out);
        assert_eq!(out.len(), 64);
        assert!(out.iter().copied().eq(0..64));
    }

    #[test]
    fn closure_sees_each_bit_once() {
        let mut seen = 0u64;
        for_each_set_bit(0b1010_0110, 0, |i| {
            assert_eq!(seen & (1 << i), 0);
            seen |= 1 << i;
        });
        assert_eq!(seen, 0b1010_0110);
    }
}
