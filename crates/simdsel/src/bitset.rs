use simdhtml::document::id::NodeId;

/// A bitset whose bit `i` corresponds to node id `i`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BitSet {
    words: Vec<u64>,
    len: usize,
}

impl BitSet {
    pub fn new(len: usize) -> Self {
        Self {
            words: vec![0; len.div_ceil(64)],
            len,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|&w| w == 0)
    }

    pub fn set(&mut self, id: NodeId) {
        let id = id.index();
        debug_assert!(id < self.len);
        self.words[id / 64] |= 1u64 << (id % 64);
    }

    pub fn clear(&mut self, id: NodeId) {
        let id = id.index();
        debug_assert!(id < self.len);
        self.words[id / 64] &= !(1u64 << (id % 64));
    }

    pub fn get(&self, id: NodeId) -> bool {
        let id = id.index();
        debug_assert!(id < self.len);
        (self.words[id / 64] >> (id % 64)) & 1 != 0
    }

    pub fn and_assign(&mut self, other: &Self) {
        for (a, b) in self.words.iter_mut().zip(&other.words) {
            *a &= *b;
        }
    }

    pub fn or_assign(&mut self, other: &Self) {
        for (a, b) in self.words.iter_mut().zip(&other.words) {
            *a |= *b;
        }
    }

    pub fn and_not_assign(&mut self, other: &Self) {
        for (a, b) in self.words.iter_mut().zip(&other.words) {
            *a &= !*b;
        }
    }

    pub fn not_assign(&mut self) {
        for word in &mut self.words {
            *word = !*word;
        }
        self.mask_tail();
    }

    pub fn clear_all(&mut self) {
        self.words.fill(0);
    }

    pub fn fill(&mut self) {
        self.words.fill(u64::MAX);
        self.mask_tail();
    }

    pub fn count_ones(&self) -> u32 {
        self.words.iter().map(|w| w.count_ones()).sum()
    }

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
                        Some(NodeId::new(base + offset))
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

    fn id(index: u32) -> NodeId {
        NodeId::new(index)
    }

    #[test]
    fn set_get_iter() {
        let mut b = BitSet::new(130);
        b.set(id(0));
        b.set(id(63));
        b.set(id(64));
        b.set(id(129));
        assert_eq!(
            b.iter_set().collect::<Vec<_>>(),
            vec![id(0), id(63), id(64), id(129)]
        );
        assert_eq!(b.count_ones(), 4);
        assert!(b.get(id(64)));
        assert!(!b.get(id(65)));
    }

    #[test]
    fn boolean_ops() {
        let mut a = BitSet::new(70);
        let mut b = BitSet::new(70);
        a.set(id(1));
        a.set(id(65));
        b.set(id(65));
        a.and_assign(&b);
        assert_eq!(a.iter_set().collect::<Vec<_>>(), vec![id(65)]);
        a.or_assign(&b);
        assert_eq!(a.count_ones(), 1);
        a.not_assign();
        assert_eq!(a.count_ones(), 69);
        assert!(!a.get(id(65)));
        assert!(a.get(id(0)) && a.get(id(69)));
    }
}
