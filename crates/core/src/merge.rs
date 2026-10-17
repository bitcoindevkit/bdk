use crate::alloc::vec::Vec;
use crate::collections::{BTreeMap, BTreeSet};

/// Trait that makes an object mergeable.
pub trait Merge: Default {
    /// Merge another object of the same type onto `self`.
    fn merge(&mut self, other: Self);

    /// Returns whether the structure is considered empty.
    fn is_empty(&self) -> bool;

    /// Take the value, replacing it with the default value.
    fn take(&mut self) -> Option<Self> {
        if self.is_empty() {
            None
        } else {
            Some(core::mem::take(self))
        }
    }
}

impl<K: Ord, V> Merge for BTreeMap<K, V> {
    fn merge(&mut self, other: Self) {
        // We use `extend` instead of `BTreeMap::append` due to performance issues with `append`.
        // Refer to https://github.com/rust-lang/rust/issues/34666#issuecomment-675658420
        BTreeMap::extend(self, other)
    }

    fn is_empty(&self) -> bool {
        BTreeMap::is_empty(self)
    }
}

impl<T: Ord> Merge for BTreeSet<T> {
    fn merge(&mut self, other: Self) {
        // We use `extend` instead of `BTreeMap::append` due to performance issues with `append`.
        // Refer to https://github.com/rust-lang/rust/issues/34666#issuecomment-675658420
        BTreeSet::extend(self, other)
    }

    fn is_empty(&self) -> bool {
        BTreeSet::is_empty(self)
    }
}

impl<T> Merge for Vec<T> {
    fn merge(&mut self, mut other: Self) {
        Vec::append(self, &mut other)
    }

    fn is_empty(&self) -> bool {
        Vec::is_empty(self)
    }
}

macro_rules! impl_merge_for_tuple {
    ($($a:ident $b:tt)*) => {
        impl<$($a),*> Merge for ($($a,)*) where $($a: Merge),* {

            fn merge(&mut self, _other: Self) {
                $(Merge::merge(&mut self.$b, _other.$b) );*
            }

            fn is_empty(&self) -> bool {
                $(Merge::is_empty(&self.$b) && )* true
            }
        }
    }
}

impl_merge_for_tuple!();
impl_merge_for_tuple!(T0 0);
impl_merge_for_tuple!(T0 0 T1 1);
impl_merge_for_tuple!(T0 0 T1 1 T2 2);
impl_merge_for_tuple!(T0 0 T1 1 T2 2 T3 3);
impl_merge_for_tuple!(T0 0 T1 1 T2 2 T3 3 T4 4);
impl_merge_for_tuple!(T0 0 T1 1 T2 2 T3 3 T4 4 T5 5);
impl_merge_for_tuple!(T0 0 T1 1 T2 2 T3 3 T4 4 T5 5 T6 6);
impl_merge_for_tuple!(T0 0 T1 1 T2 2 T3 3 T4 4 T5 5 T6 6 T7 7);
impl_merge_for_tuple!(T0 0 T1 1 T2 2 T3 3 T4 4 T5 5 T6 6 T7 7 T8 8);
impl_merge_for_tuple!(T0 0 T1 1 T2 2 T3 3 T4 4 T5 5 T6 6 T7 7 T8 8 T9 9);
impl_merge_for_tuple!(T0 0 T1 1 T2 2 T3 3 T4 4 T5 5 T6 6 T7 7 T8 8 T9 9 T10 10);
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_btreemap() {
        let mut map1 = BTreeMap::new();
        map1.insert(1, "a");
        let mut map2 = BTreeMap::new();
        map2.insert(2, "b");

        Merge::merge(&mut map1, map2);
        assert_eq!(map1.len(), 2);
        assert_eq!(map1.get(&1), Some(&"a"));
        assert_eq!(map1.get(&2), Some(&"b"));
    }

    #[test]
    fn test_merge_btreeset() {
        let mut set1 = BTreeSet::new();
        set1.insert(1);
        let mut set2 = BTreeSet::new();
        set2.insert(2);

        Merge::merge(&mut set1, set2);
        assert_eq!(set1.len(), 2);
        assert!(set1.contains(&1));
        assert!(set1.contains(&2));
    }

    #[test]
    fn test_merge_vec() {
        let mut vec1 = vec![1, 2];
        let vec2 = vec![3, 4];

        Merge::merge(&mut vec1, vec2);
        assert_eq!(vec1, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_merge_tuple() {
        let mut t1 = (vec![1], vec![2]);
        let t2 = (vec![3], vec![4]);

        Merge::merge(&mut t1, t2);
        assert_eq!(t1, (vec![1, 3], vec![2, 4]));
    }

    #[test]
    fn test_take() {
        let mut vec1: Vec<i32> = vec![];
        assert_eq!(Merge::take(&mut vec1), None);

        let mut vec2 = vec![1, 2, 3];
        assert_eq!(Merge::take(&mut vec2), Some(vec![1, 2, 3]));
        assert!(Merge::is_empty(&vec2));
    }
}
