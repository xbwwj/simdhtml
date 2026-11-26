use super::{BLOCK_SIZE, StructuralMasks};

/// Scalar reference for [`super::classify`].
pub fn classify(block: &[u8; BLOCK_SIZE]) -> StructuralMasks {
    let mut m = StructuralMasks::default();
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

/// Scalar reference for [`super::x86::probe_lt`].
pub fn probe_lt(block: &[u8; BLOCK_SIZE]) -> Option<StructuralMasks> {
    let m = classify(block);
    if m.lt == 0 { None } else { Some(m) }
}

/// Portable [`super::prefix_xor`], five shift steps.
pub fn prefix_xor(mut x: u64) -> u64 {
    x ^= x << 1;
    x ^= x << 2;
    x ^= x << 4;
    x ^= x << 8;
    x ^= x << 16;
    x ^= x << 32;
    x
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
    fn prefix_xor_known_values() {
        assert_eq!(prefix_xor(0), 0);
        assert_eq!(prefix_xor(1), u64::MAX);
        assert_eq!(prefix_xor(0b0010_0100), 0b0001_1100);
        assert_eq!(prefix_xor(u64::MAX), 0x5555_5555_5555_5555);
    }

    #[test]
    fn prefix_xor_matches_naive() {
        let mut state = 0x1234_5678_9abc_def0u64;
        for _ in 0..10_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            assert_eq!(prefix_xor(state), naive(state));
        }
    }

    #[test]
    fn classify_finds_only_structural_bytes() {
        let mut block = [b'a'; BLOCK_SIZE];
        block[0] = b'<';
        block[1] = b'>';
        block[63] = b'"';
        let m = classify(&block);
        assert_eq!(m.lt, 1);
        assert_eq!(m.gt, 1 << 1);
        assert_eq!(m.dq, 1 << 63);
        assert_eq!(m.sq, 0);
    }
}
