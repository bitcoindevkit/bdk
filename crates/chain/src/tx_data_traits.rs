use crate::collections::BTreeMap;
use crate::collections::BTreeSet;
use crate::BlockId;
use alloc::vec::Vec;
use bitcoin::Txid;

/// TODO: New [`Anchor`] docs
pub type Anchor = (Txid, BlockId);

/// TODO: [`BlockTime`] docs
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(crate = "serde_crate",)
)]
pub struct BlockTime(u32);

impl BlockTime {
    /// TODO: new() docs
    pub fn new(confirmation_time: u32) -> Self {
        BlockTime(confirmation_time)
    }
}

impl AsRef<u32> for BlockTime {
    fn as_ref(&self) -> &u32 {
        &self.0
    }
}

impl AnchorMetaFromBlock for BlockTime {
    fn from_block(block: &bitcoin::Block, _block_id: BlockId, _tx_pos: usize) -> Self {
        BlockTime(block.header.time)
    }
}

/// [`Anchor`] metadata that can be constructed from a given block, block height and transaction
/// position within the block.
pub trait AnchorMetaFromBlock {
    /// Construct the anchor metadata from a given `block`, block height and `tx_pos` within the block.
    fn from_block(cblock: &bitcoin::Block, block_id: BlockId, tx_pos: usize) -> Self;
}

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
