//! Fixed-length bitset over node ids.
//!
//! One `u64` word per 64 nodes.  The selector evaluates every compound into a
//! bitset and every combinator with a bitset transform; there is no
//! per-element parent/prev backtracking on the main path.

use simdhtml::NodeId;

/// A bitset whose bit `i` corresponds to node id `i`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BitSet {
    words: Vec<u64>,
    len: usize,
}

impl BitSet {
    /// Creates an empty bitset with capacity for `len` nodes.
    #[must_use]
    pub fn new(len: usize) -> Self {
        Self {
            words: vec![0; len.div_ceil(64)],
            len,
        }
    }

    /// Number of addressable bits.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether no bits are set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|&w| w == 0)
    }

    /// Sets bit `id`.
    pub fn set(&mut self, id: NodeId) {
        let id = id as usize;
        debug_assert!(id < self.len);
        self.words[id / 64] |= 1u64 << (id % 64);
    }

    /// Clears bit `id`.
    pub fn clear(&mut self, id: NodeId) {
        let id = id as usize;
        debug_assert!(id < self.len);
        self.words[id / 64] &= !(1u64 << (id % 64));
    }

    /// Returns bit `id`.
    #[must_use]
    pub fn get(&self, id: NodeId) -> bool {
        let id = id as usize;
        debug_assert!(id < self.len);
        (self.words[id / 64] >> (id % 64)) & 1 != 0
    }

    /// In-place AND.
    pub fn and_assign(&mut self, other: &Self) {
        for (a, b) in self.words.iter_mut().zip(&other.words) {
            *a &= *b;
        }
    }

    /// In-place OR.
    pub fn or_assign(&mut self, other: &Self) {
        for (a, b) in self.words.iter_mut().zip(&other.words) {
            *a |= *b;
        }
    }

    /// In-place `self &= !other`.
    pub fn and_not_assign(&mut self, other: &Self) {
        for (a, b) in self.words.iter_mut().zip(&other.words) {
            *a &= !*b;
        }
    }

    /// In-place complement within the addressable length.
    pub fn not_assign(&mut self) {
        for word in &mut self.words {
            *word = !*word;
        }
        self.mask_tail();
    }

    /// Clears all bits without changing the addressable length.
    pub fn clear_all(&mut self) {
        self.words.fill(0);
    }

    /// Sets all bits.
    pub fn fill(&mut self) {
        self.words.fill(u64::MAX);
        self.mask_tail();
    }

    /// Number of set bits.
    #[must_use]
    pub fn count_ones(&self) -> u32 {
        self.words.iter().map(|w| w.count_ones()).sum()
    }

    /// Iterates set node ids in ascending order.
    pub fn iter_set(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.words
            .iter()
            .enumerate()
            .flat_map(|(word_index, &word)| {
                let base = (word_index * 64) as u32;
                let mut bits = word;
                std::iter::from_fn(move || {
                    if bits == 0 {
                        None
                    } else {
                        let offset = bits.trailing_zeros();
                        bits &= bits - 1;
                        Some(base + offset)
                    }
                })
            })
    }

    fn mask_tail(&mut self) {
        let rem = self.len % 64;
        if rem != 0
            && let Some(last) = self.words.last_mut()
        {
            *last &= (1u64 << rem) - 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_iter() {
        let mut b = BitSet::new(130);
        b.set(0);
        b.set(63);
        b.set(64);
        b.set(129);
        assert_eq!(b.iter_set().collect::<Vec<_>>(), vec![0, 63, 64, 129]);
        assert_eq!(b.count_ones(), 4);
        assert!(b.get(64));
        assert!(!b.get(65));
    }

    #[test]
    fn boolean_ops() {
        let mut a = BitSet::new(70);
        let mut b = BitSet::new(70);
        a.set(1);
        a.set(65);
        b.set(65);
        a.and_assign(&b);
        assert_eq!(a.iter_set().collect::<Vec<_>>(), vec![65]);
        a.or_assign(&b);
        assert_eq!(a.count_ones(), 1);
        a.not_assign();
        assert_eq!(a.count_ones(), 69);
        assert!(!a.get(65));
        assert!(a.get(0) && a.get(69));
    }
}
