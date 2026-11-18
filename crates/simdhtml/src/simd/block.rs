//! 64-byte block reader.
//!
//! The stage-1 scanner processes 64-byte blocks.  The final partial block is
//! *not* zero-padded here; callers that need a full block (for `vpshufb` /
//! AVX2 loads) pad it explicitly and record the valid length.  Keeping the
//! reader allocation-free and copy-free is important: it is called once per
//! 64 bytes of input.
//!
//! Stage-1 scanner 按照 64 字节处理。
//!
//! 最终的 partial block 不是 zero padded, 需要手动记录全 block 和有效长度
//!
//! AVX2 实际宽度是 32

/// Block size used by the scanner.
pub const BLOCK_SIZE: usize = 64;

/// Iterates over full 64-byte blocks and exposes the remainder separately.
#[derive(Clone, Debug)]
pub struct BlockReader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> BlockReader<'a> {
    /// Creates a reader over `data`.
    #[must_use]
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    /// Byte offset of the next full block.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    /// Bytes not yet returned as a full block.
    #[must_use]
    pub fn remaining(&self) -> &'a [u8] {
        &self.data[self.offset..]
    }

    /// Number of remaining bytes.
    #[must_use]
    pub fn remaining_len(&self) -> usize {
        self.data.len() - self.offset
    }

    /// Returns the next full 64-byte block, or `None` at the tail.
    ///
    /// The returned reference points into the original input; no copy is made.
    pub fn next_block(&mut self) -> Option<&'a [u8; BLOCK_SIZE]> {
        if self.remaining_len() < BLOCK_SIZE {
            return None;
        }
        let bytes = &self.data[self.offset..self.offset + BLOCK_SIZE];
        let block: &[u8; BLOCK_SIZE] = bytes.try_into().expect("block length checked");
        self.offset += BLOCK_SIZE;
        Some(block)
    }

    /// Returns a zero-padded copy of the tail, plus its valid length.
    ///
    /// Returns `None` when no bytes remain.
    #[must_use]
    pub fn tail_padded(&self) -> Option<([u8; BLOCK_SIZE], usize)> {
        let tail = self.remaining();
        if tail.is_empty() {
            return None;
        }
        let mut block = [0u8; BLOCK_SIZE];
        block[..tail.len()].copy_from_slice(tail);
        Some((block, tail.len()))
    }
}

impl<'a> Iterator for BlockReader<'a> {
    type Item = &'a [u8; BLOCK_SIZE];

    fn next(&mut self) -> Option<Self::Item> {
        self.next_block()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_multiple() {
        let data = vec![7u8; BLOCK_SIZE * 3];
        let mut reader = BlockReader::new(&data);
        assert_eq!(reader.next_block().unwrap()[0], 7);
        assert_eq!(reader.next_block().unwrap()[0], 7);
        assert_eq!(reader.next_block().unwrap()[0], 7);
        assert!(reader.next_block().is_none());
        assert_eq!(reader.remaining_len(), 0);
        assert!(reader.tail_padded().is_none());
    }

    #[test]
    fn tail_is_exposed() {
        let data = vec![1u8; BLOCK_SIZE + 5];
        let mut reader = BlockReader::new(&data);
        assert!(reader.next_block().is_some());
        assert_eq!(reader.remaining_len(), 5);
        let (tail, valid) = reader.tail_padded().unwrap();
        assert_eq!(valid, 5);
        assert_eq!(&tail[..5], &[1, 1, 1, 1, 1]);
        assert_eq!(&tail[5..], &[0; BLOCK_SIZE - 5]);
        assert_eq!(reader.offset(), BLOCK_SIZE);
    }

    #[test]
    fn empty_input() {
        let data: [u8; 0] = [];
        let mut reader = BlockReader::new(&data);
        assert!(reader.next_block().is_none());
        assert_eq!(reader.remaining_len(), 0);
        assert!(reader.tail_padded().is_none());
    }
}
