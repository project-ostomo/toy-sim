use std::{collections::hash_set, hash::Hash, slice};

use ahash::AHashSet;
use arrayvec::ArrayVec;

// Keep singleton cells allocation-free without putting a large array in every
// cell table entry. Promote when the fixed-capacity vector is full.
const INLINE_ITEMS: usize = if cfg!(feature = "hybrid-cells-32") {
    32
} else if cfg!(feature = "hybrid-cells-16") {
    16
} else if cfg!(feature = "hybrid-cells-8") {
    8
} else if cfg!(feature = "hybrid-cells-4") {
    4
} else if cfg!(feature = "hybrid-cells-2") {
    2
} else {
    1
};

pub(crate) enum Cell<T> {
    Small(ArrayVec<T, INLINE_ITEMS>),
    Large(Box<AHashSet<T>>),
}

impl<T> Default for Cell<T> {
    fn default() -> Self {
        Self::Small(ArrayVec::new())
    }
}

impl<T: Eq + Hash> Cell<T> {
    pub(crate) fn insert(&mut self, item: T) -> bool {
        match self {
            Self::Large(items) => items.insert(item),
            Self::Small(items) => {
                if items.contains(&item) {
                    return false;
                }
                if items.len() < INLINE_ITEMS {
                    items.push(item);
                } else {
                    let mut large = AHashSet::with_capacity(INLINE_ITEMS + 1);
                    large.extend(items.drain(..));
                    large.insert(item);
                    *self = Self::Large(Box::new(large));
                }
                true
            }
        }
    }

    pub(crate) fn remove(&mut self, item: &T) -> bool {
        match self {
            Self::Large(items) => items.remove(item),
            Self::Small(items) => {
                if let Some(index) = items.iter().position(|existing| existing == item) {
                    items.swap_remove(index);
                    true
                } else {
                    false
                }
            }
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        match self {
            Self::Small(items) => items.is_empty(),
            Self::Large(items) => items.is_empty(),
        }
    }

    pub(crate) fn iter(&self) -> Iter<'_, T> {
        match self {
            Self::Small(items) => Iter::Small(items.iter()),
            Self::Large(items) => Iter::Large(items.iter()),
        }
    }
}

pub(crate) enum Iter<'a, T> {
    Small(slice::Iter<'a, T>),
    Large(hash_set::Iter<'a, T>),
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Small(items) => items.next(),
            Self::Large(items) => items.next(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promotion_preserves_set_semantics_and_removal() {
        let mut cell = Cell::default();
        for item in 0..INLINE_ITEMS {
            assert!(cell.insert(item));
            assert!(!cell.insert(item));
        }
        assert!(matches!(cell, Cell::Small(_)));
        assert!(cell.remove(&0));
        assert!(!cell.remove(&0));
        assert!(cell.insert(0));
        assert!(cell.insert(INLINE_ITEMS));
        assert!(matches!(cell, Cell::Large(_)));
        let mut contents: Vec<_> = cell.iter().copied().collect();
        contents.sort_unstable();
        assert_eq!(contents, (0..=INLINE_ITEMS).collect::<Vec<_>>());
        for item in 0..=INLINE_ITEMS {
            assert!(!cell.insert(item));
            assert!(cell.remove(&item));
        }
        assert!(cell.is_empty());
    }
}
