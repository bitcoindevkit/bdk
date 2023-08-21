use crate::collections::BTreeMap;
use crate::collections::BTreeSet;
use crate::BlockId;
use alloc::vec::Vec;

/// Trait that "anchors" blockchain data to a specific block of height and hash.
///
/// [`Anchor`] implementations must be [`Ord`] by the anchor block's [`BlockId`] first.
///
/// I.e. If transaction A is anchored in block B, then if block B is in the best chain, we can
/// assume that transaction A is also confirmed in the best chain. This does not necessarily mean
/// that transaction A is confirmed in block B. It could also mean transaction A is confirmed in a
/// parent block of B.
pub trait Anchor: core::fmt::Debug + Clone + Eq + PartialOrd + Ord + core::hash::Hash {
    /// Returns the [`BlockId`] that the associated blockchain data is "anchored" in.
    fn anchor_block(&self) -> BlockId;

    /// Get the upper bound of the chain data's confirmation height.
    ///
    /// The default definition gives a pessimistic answer. This can be overridden by the `Anchor`
    /// implementation for a more accurate value.
    fn confirmation_height_upper_bound(&self) -> u32 {
        self.anchor_block().height
    }
}

impl<A: Anchor> Anchor for &'static A {
    fn anchor_block(&self) -> BlockId {
        <A as Anchor>::anchor_block(self)
    }
}

/// Trait that makes an object appendable.
pub trait Append {
    /// Append another object of the same type onto `self`.
    fn append(&mut self, other: Self);

    /// Returns whether the structure is considered empty.
    fn is_empty(&self) -> bool;
}

impl Append for () {
    fn append(&mut self, _other: Self) {}

    fn is_empty(&self) -> bool {
        true
    }
}

impl<K: Ord, V> Append for BTreeMap<K, V> {
    fn append(&mut self, mut other: Self) {
        BTreeMap::append(self, &mut other)
    }

    fn is_empty(&self) -> bool {
        BTreeMap::is_empty(self)
    }
}

impl<T: Ord> Append for BTreeSet<T> {
    fn append(&mut self, mut other: Self) {
        BTreeSet::append(self, &mut other)
    }

    fn is_empty(&self) -> bool {
        BTreeSet::is_empty(self)
    }
}

impl<T> Append for Vec<T> {
    fn append(&mut self, mut other: Self) {
        Vec::append(self, &mut other)
    }

    fn is_empty(&self) -> bool {
        Vec::is_empty(self)
    }
}

impl<A: Append, B: Append> Append for (A, B) {
    fn append(&mut self, other: Self) {
        Append::append(&mut self.0, other.0);
        Append::append(&mut self.1, other.1);
    }

    fn is_empty(&self) -> bool {
        Append::is_empty(&self.0) && Append::is_empty(&self.1)
    }
}
