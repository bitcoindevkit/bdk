use alloc::sync::Arc;
use alloc::vec::Vec;
use bitcoin::{block::Header, BlockHash};
use core::fmt;
use core::ops::RangeBounds;

use crate::{BlockId, CheckPointEntry, CheckPointEntryIter};

/// Returns the checkpoint index that index `i` should hold a skip pointer to.
///
/// Mirrors Bitcoin Core's `GetSkipHeight` (operating on checkpoint indices rather than block
/// heights, so sparse chains work). The chosen targets give skip distances that grow
/// exponentially as you walk back, yielding `O(log n)` traversal.
///
/// For `i < 2` returns `0` — `prev` already covers those distances trivially.
fn skip_index(i: u32) -> u32 {
    // Clears the lowest set bit of `n`. This is what unlocks the exponential skip range:
    // each call strips one trailing 1-bit, so the result lands at a power-of-two-aligned
    // index below `n`.
    fn invert_lowest_one(n: u32) -> u32 {
        // Wrapping sub so that `invert_lowest_one(0) == 0` (matches Bitcoin Core's signed-int
        // `n & (n-1)` semantics). The odd-i branch below relies on `invert_lowest_one(0) == 0`
        // when `i == 3`.
        n & n.wrapping_sub(1)
    }
    if i < 2 {
        return 0;
    }
    if i & 1 == 0 {
        return invert_lowest_one(i);
    }
    invert_lowest_one(invert_lowest_one(i - 1)) + 1
}

/// Walks back from `start` to the ancestor at `target` index, riding skip pointers in `O(log n)`.
///
/// Equivalent to Bitcoin Core's `CBlockIndex::GetAncestor`.
fn ancestor_by_index<D>(start: &Arc<CPInner<D>>, target: u32) -> Arc<CPInner<D>> {
    debug_assert!(target <= start.index);
    let mut curr = start.clone();
    while curr.index > target {
        let skip_i = skip_index(curr.index);
        let skip_i_prev = skip_index(curr.index.saturating_sub(1));

        // Prev's skip is "strictly better" when it lands more than 2 indices below current's
        // skip AND still reaches `target`. In that case we'd rather take one step back via
        // `prev` and ride its longer skip next iteration.
        let prev_skip_strictly_better =
            skip_i > skip_i_prev.saturating_add(2) && skip_i_prev >= target;
        let use_skip = curr.skip.is_some()
            && (skip_i == target || (skip_i > target && !prev_skip_strictly_better));

        curr = if use_skip {
            curr.skip.clone().expect("checked above")
        } else {
            curr.prev
                .clone()
                .expect("walking toward smaller target requires prev")
        };
    }

    curr
}

/// Walks back from `current` to the highest checkpoint at or below `target_height`.
///
/// Internally rides skip pointers using Bitcoin Core's `GetAncestor` skip-vs-prev heuristic,
/// adapted to operate on heights so it works on sparse checkpoint chains.
///
/// Returns `None` only when the chain's base is itself above `target_height` (no ancestor exists
/// at or below the target).
fn walk_to_floor<D>(current: &CheckPoint<D>, target_height: u32) -> Option<CheckPoint<D>> {
    let mut curr = current.clone();
    while curr.height() > target_height {
        let skip = curr.skip();
        let take_skip = match &skip {
            Some(skip_cp) if skip_cp.height() < target_height => false,
            Some(skip_cp) if skip_cp.height() == target_height => true,
            // Skip lands above target. Prefer prev's skip if it's a strictly bigger jump (lands
            // more than 2 heights lower than current's skip) that still reaches target.
            Some(skip_cp) => match curr.prev().and_then(|p| p.skip()) {
                Some(prev_skip_cp) => {
                    let prev_skip_h = prev_skip_cp.height();
                    let skip_gap = skip_cp.height().saturating_sub(prev_skip_h);
                    !(skip_gap > 2 && prev_skip_h >= target_height)
                }
                None => true,
            },
            None => false,
        };
        curr = if take_skip { skip? } else { curr.prev()? };
    }
    Some(curr)
}

/// A checkpoint is a node of a reference-counted linked list of [`BlockId`]s.
///
/// Checkpoints are cheaply cloneable and are useful to find the agreement point between two sparse
/// block chains.
#[derive(Debug)]
pub struct CheckPoint<D = BlockHash>(Arc<CPInner<D>>);

impl<D> Clone for CheckPoint<D> {
    fn clone(&self) -> Self {
        CheckPoint(Arc::clone(&self.0))
    }
}

/// The internal contents of [`CheckPoint`].
#[derive(Debug)]
struct CPInner<D> {
    /// Block id
    block_id: BlockId,
    /// Data.
    data: D,
    /// Previous checkpoint (if any).
    prev: Option<Arc<CPInner<D>>>,
    /// Skip pointer for fast traversals.
    skip: Option<Arc<CPInner<D>>>,
    /// Index of this checkpoint (number of checkpoints from the first).
    index: u32,
}

/// When a `CPInner` is dropped we need to go back down the chain and manually remove any
/// no-longer referenced checkpoints. Letting the default rust dropping mechanism handle this
/// leads to recursive logic and stack overflows
///
/// https://github.com/bitcoindevkit/bdk/issues/1634
impl<D> Drop for CPInner<D> {
    fn drop(&mut self) {
        // Take out `prev` so its `drop` won't be called when this drop is finished.
        let mut current = self.prev.take();
        // Collect nodes to drop later so we avoid recursive drop calls while not leaking memory.
        while let Some(arc_node) = current {
            // Get rid of the `Arc` around `prev` if we're the only one holding a reference so the
            // `drop` on it won't be called when the `Arc` is dropped.
            let arc_inner = Arc::into_inner(arc_node);

            match arc_inner {
                Some(mut node) => {
                    node.skip.take(); // We don't want to recursively drop `node.skip`.
                    current = node.prev.take();
                }
                None => break,
            }
        }
    }
}

/// Trait that converts [`CheckPoint`] `data` to [`BlockHash`].
///
/// Implementations of [`ToBlockHash`] must always return the block's consensus-defined hash. If
/// your type contains extra fields (timestamps, metadata, etc.), these must be ignored. For
/// example, [`BlockHash`] trivially returns itself, [`Header`] calls its `block_hash()`, and a
/// wrapper type around a [`Header`] should delegate to the header's hash rather than derive one
/// from other fields.
pub trait ToBlockHash {
    /// Returns the [`BlockHash`] for the associated [`CheckPoint`] `data` type.
    fn to_blockhash(&self) -> BlockHash;

    /// Returns `None` if the type has no knowledge of the previous [`BlockHash`].
    fn prev_blockhash(&self) -> Option<BlockHash> {
        None
    }
}

impl ToBlockHash for BlockHash {
    fn to_blockhash(&self) -> BlockHash {
        *self
    }
}

impl ToBlockHash for Header {
    fn to_blockhash(&self) -> BlockHash {
        self.block_hash()
    }

    fn prev_blockhash(&self) -> Option<BlockHash> {
        Some(self.prev_blockhash)
    }
}

/// Trait that extracts a block time from [`CheckPoint`] `data`.
///
/// `data` types that contain a block time should implement this.
pub trait ToBlockTime {
    /// Returns the block time from the [`CheckPoint`] `data`.
    fn to_blocktime(&self) -> u32;
}

impl ToBlockTime for Header {
    fn to_blocktime(&self) -> u32 {
        self.time
    }
}

impl<D> PartialEq for CheckPoint<D> {
    fn eq(&self, other: &Self) -> bool {
        let self_cps = self.iter().map(|cp| cp.block_id());
        let other_cps = other.iter().map(|cp| cp.block_id());
        self_cps.eq(other_cps)
    }
}

// Methods for any `D`
impl<D> CheckPoint<D> {
    /// Get a reference of the `data` of the checkpoint.
    pub fn data_ref(&self) -> &D {
        &self.0.data
    }

    /// Get the `data` of a the checkpoint.
    pub fn data(&self) -> D
    where
        D: Clone,
    {
        self.0.data.clone()
    }

    /// Get the [`BlockId`] of the checkpoint.
    pub fn block_id(&self) -> BlockId {
        self.0.block_id
    }

    /// Get the `height` of the checkpoint.
    pub fn height(&self) -> u32 {
        self.block_id().height
    }

    /// Get the block hash of the checkpoint.
    pub fn hash(&self) -> BlockHash {
        self.block_id().hash
    }

    /// Get the previous checkpoint in the chain.
    pub fn prev(&self) -> Option<CheckPoint<D>> {
        self.0.prev.clone().map(CheckPoint)
    }

    /// Get the index of this checkpoint (number of checkpoints from the first).
    pub fn index(&self) -> u32 {
        self.0.index
    }

    /// Get this checkpoint's pskip ancestor, if one exists.
    ///
    /// Returns the ancestor at the [skip index](Self::index) — a deterministically chosen
    /// checkpoint that lets traversals cover exponentially-growing distances per hop. Returns
    /// `None` for the genesis checkpoint (index `0`); for index `1`, the skip ancestor is the same
    /// checkpoint as `prev`.
    ///
    /// This accessor exposes the internal pskip topology and is intended for diagnostics and
    /// benchmarks, not for general-purpose traversal — use [`get`](Self::get),
    /// [`range`](Self::range), or [`floor_at`](Self::floor_at) instead.
    pub fn skip(&self) -> Option<CheckPoint<D>> {
        self.0.skip.clone().map(CheckPoint)
    }

    /// Iterate from this checkpoint in descending height.
    pub fn iter(&self) -> CheckPointIter<D> {
        self.clone().into_iter()
    }

    /// Get checkpoint at `height`.
    ///
    /// Returns `None` if checkpoint at `height` does not exist`.
    pub fn get(&self, height: u32) -> Option<Self> {
        if self.height() < height {
            return None;
        }
        let floor = walk_to_floor(self, height)?;
        if floor.height() == height {
            Some(floor)
        } else {
            None
        }
    }

    /// Iterate checkpoints over a height range.
    ///
    /// Note that we always iterate checkpoints in reverse height order (iteration starts at tip
    /// height).
    pub fn range<R>(&self, range: R) -> impl Iterator<Item = CheckPoint<D>>
    where
        R: RangeBounds<u32>,
    {
        let start_bound = range.start_bound().cloned();
        let end_bound = range.end_bound().cloned();

        // Find the highest checkpoint at or below the upper bound in `O(log n)`. For unbounded
        // upper, start at `self`; for an excluded `0`, the range is empty.
        let start = match end_bound {
            core::ops::Bound::Included(b) => walk_to_floor(self, b),
            core::ops::Bound::Excluded(b) => b.checked_sub(1).and_then(|b| walk_to_floor(self, b)),
            core::ops::Bound::Unbounded => Some(self.clone()),
        };

        start
            .into_iter()
            .flat_map(IntoIterator::into_iter)
            .take_while(move |cp| match start_bound {
                core::ops::Bound::Included(inc_bound) => cp.height() >= inc_bound,
                core::ops::Bound::Excluded(exc_bound) => cp.height() > exc_bound,
                core::ops::Bound::Unbounded => true,
            })
    }

    /// Returns the checkpoint at `height` if one exists, otherwise the nearest checkpoint at a
    /// lower height.
    ///
    /// This is equivalent to taking the "floor" of `height` over this checkpoint chain.
    ///
    /// Returns `None` if no checkpoint exists at or below the given height.
    pub fn floor_at(&self, height: u32) -> Option<Self> {
        self.range(..=height).next()
    }

    /// Returns the checkpoint located a number of heights below this one.
    ///
    /// This is a convenience wrapper for [`CheckPoint::floor_at`], subtracting `to_subtract` from
    /// the current height.
    ///
    /// - If a checkpoint exists exactly `offset` heights below, it is returned.
    /// - Otherwise, the nearest checkpoint *below that target height* is returned.
    ///
    /// Returns `None` if `to_subtract` is greater than the current height, or if there is no
    /// checkpoint at or below the target height.
    pub fn floor_below(&self, offset: u32) -> Option<Self> {
        self.floor_at(self.height().checked_sub(offset)?)
    }

    /// This method tests for `self` and `other` to have equal internal pointers.
    pub fn eq_ptr(&self, other: &Self) -> bool {
        Arc::as_ptr(&self.0) == Arc::as_ptr(&other.0)
    }
}

impl<D> CheckPoint<D>
where
    D: ToBlockHash,
{
    /// Iterate entries from this checkpoint in descending height.
    pub fn entry_iter(&self) -> CheckPointEntryIter<D> {
        self.to_entry().into_iter()
    }

    /// Transforms this checkpoint into a [`CheckPointEntry`].
    pub fn into_entry(self) -> CheckPointEntry<D> {
        CheckPointEntry::Occupied(self)
    }

    /// Creates a [`CheckPointEntry`].
    pub fn to_entry(&self) -> CheckPointEntry<D> {
        CheckPointEntry::Occupied(self.clone())
    }
}

// Methods where `D: ToBlockHash`
impl<D> CheckPoint<D>
where
    D: ToBlockHash + fmt::Debug + Clone,
{
    const MTP_BLOCK_COUNT: u32 = 11;

    /// Construct a new base [`CheckPoint`] from given `height` and `data` at the front of a linked
    /// list.
    pub fn new(height: u32, data: D) -> Self {
        Self(Arc::new(CPInner {
            block_id: BlockId {
                height,
                hash: data.to_blockhash(),
            },
            data,
            prev: None,
            skip: None,
            index: 0,
        }))
    }

    /// Calculate the median time past (MTP) for this checkpoint.
    ///
    /// Uses 11 blocks (heights h-10 through h, where h is the current height) to compute the MTP
    /// for the current block. This is used in Bitcoin's consensus rules for time-based validations
    /// (BIP-0113).
    ///
    /// Note: This is a pseudo-median that doesn't average the two middle values.
    ///
    /// Returns `None` if the data type doesn't support block times or if any of the required
    /// 11 sequential blocks are missing.
    pub fn median_time_past(&self) -> Option<u32>
    where
        D: ToBlockTime,
    {
        let current_height = self.height();
        let earliest_height = current_height.saturating_sub(Self::MTP_BLOCK_COUNT - 1);

        let mut timestamps = (earliest_height..=current_height)
            .map(|height| {
                // Return `None` for missing blocks or missing block times
                let cp = self.get(height)?;
                let block_time = cp.data_ref().to_blocktime();
                Some(block_time)
            })
            .collect::<Option<Vec<u32>>>()?;
        timestamps.sort_unstable();

        // If there are more than 1 middle values, use the higher middle value.
        // This is mathematically incorrect, but this is the BIP-0113 specification.
        Some(timestamps[timestamps.len() / 2])
    }

    /// Construct from an iterator of block data.
    ///
    /// # Returns
    ///
    /// Returns the checkpoint chain tip on success.
    ///
    /// # Errors
    ///
    /// Returns `Err(None)` if `blocks` doesn't yield any data. If the blocks are not in ascending
    /// height order, or there are any `prev_blockhash` mismatches, then returns `Err(Some(..))`
    /// containing the last checkpoint that was successfully extended.
    pub fn from_blocks(blocks: impl IntoIterator<Item = (u32, D)>) -> Result<Self, Option<Self>> {
        let mut blocks = blocks.into_iter();
        let (height, data) = blocks.next().ok_or(None)?;
        let mut cp = CheckPoint::new(height, data);
        cp = cp.extend(blocks).map_err(Some)?;

        Ok(cp)
    }

    /// Extends the checkpoint linked list by a iterator containing `height` and `data`.
    ///
    /// Returns an `Err(self)` if there is a block which does not have a greater height than the
    /// previous one, or doesn't properly link to an adjacent block via its `prev_blockhash`.
    /// See docs for [`CheckPoint::push`].
    pub fn extend(self, blockdata: impl IntoIterator<Item = (u32, D)>) -> Result<Self, Self> {
        let mut cp = self.clone();
        for (height, data) in blockdata {
            cp = cp.push(height, data)?;
        }
        Ok(cp)
    }

    /// Inserts `data` at its `height` within the chain.
    ///
    /// If a checkpoint already exists at `height` with a matching hash, returns `self` unchanged.
    /// Otherwise, if the insertion conflicts — either with an existing checkpoint at `height` (by
    /// hash), or with the checkpoint at `height - 1` (via `data.prev_blockhash`) — every
    /// checkpoint at or above `height` is removed.
    ///
    /// # Panics
    ///
    /// Panics if the insertion would replace (or omit) the checkpoint at height 0 (a.k.a
    /// "genesis"). Although [`CheckPoint`] isn't structurally required to contain a genesis
    /// block, if one is present, it stays immutable and can't be replaced.
    #[must_use]
    pub fn insert(self, height: u32, data: D) -> Self {
        let mut cp = self.clone();
        let mut tail = vec![];

        // Traverse from tip to base, looking for where to insert.
        let base = loop {
            // Genesis (height 0) must remain immutable.
            if cp.height() == 0 {
                let implied_genesis = match height {
                    0 => Some(data.to_blockhash()),
                    1 => data.prev_blockhash(),
                    _ => None,
                };
                if let Some(hash) = implied_genesis {
                    assert_eq!(hash, cp.hash(), "inserted data implies different genesis");
                }
            }

            // Above insertion: collect for potential re-insertion later.
            // No need to check data.prev_blockhash here since that points below insertion. The
            // reverse relationship (cp.prev_blockhash vs data.hash) is validated during rebuild.
            if cp.height() > height {
                tail.push((cp.height(), cp.data()));

            // At insertion: determine whether we need to clear tail, or early return.
            } else if cp.height() == height {
                if cp.hash() == data.to_blockhash() {
                    return self;
                }
                tail.clear();

            // Displacement: data's prev_blockhash conflicts with this checkpoint,
            // so skip it and invalidate everything above.
            } else if cp.height() + 1 == height
                && data.prev_blockhash().is_some_and(|h| h != cp.hash())
            {
                tail.clear();

            // Below insertion: this is our base (since data's prev_blockhash does not conflict).
            } else if cp.height() < height {
                break Some(cp);
            }

            // Continue traversing down (if possible).
            match cp.prev() {
                Some(prev) => cp = prev,
                None => break None,
            }
        };

        tail.push((height, data));
        let tail = tail.into_iter().rev();

        // Reconstruct the chain: If a block above insertion has a prev_blockhash that doesn't match
        // the inserted data's hash, that block and everything above it are evicted.
        let (Ok(cp) | Err(cp)) = match base {
            Some(base_cp) => base_cp.extend(tail),
            None => CheckPoint::from_blocks(tail).map_err(|err| err.expect("tail is non-empty")),
        };
        cp
    }

    /// Puts another checkpoint onto the linked list representing the blockchain.
    ///
    /// Returns an `Err(self)` if:
    /// * The block you are pushing on is not at a greater height that the one you are pushing on
    ///   to.
    /// * The `prev_blockhash` does not match.
    pub fn push(self, height: u32, data: D) -> Result<Self, Self> {
        // Reject if trying to push at or below current height - chain must grow forward.
        if height <= self.height() {
            return Err(self);
        }

        // For contiguous height, ensure prev_blockhash does not conflict.
        if let Some(prev_blockhash) = data.prev_blockhash() {
            if self.height() + 1 == height && self.hash() != prev_blockhash {
                return Err(self);
            }
        }

        let new_index = self.0.index + 1;

        // Wire the pskip pointer to the ancestor at skip_index(new_index). This is what makes
        // traversal O(log n): each checkpoint carries one skip Arc to a deterministically chosen
        // ancestor, and the chosen distances grow exponentially as you walk back.
        let skip = Some(ancestor_by_index(&self.0, skip_index(new_index)));

        Ok(Self(Arc::new(CPInner {
            block_id: BlockId {
                height,
                hash: data.to_blockhash(),
            },
            data,
            prev: Some(self.0),
            skip,
            index: new_index,
        })))
    }
}

/// Iterates over checkpoints backwards.
pub struct CheckPointIter<D> {
    next: Option<Arc<CPInner<D>>>,
}

impl<D> Iterator for CheckPointIter<D> {
    type Item = CheckPoint<D>;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.next.clone()?;
        self.next.clone_from(&current.prev);
        Some(CheckPoint(current))
    }

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        // Take `self.next` since if the `n`th is not found, `.next` should return `None`.
        let current = self.next.take()?;

        let target_index = current.index.checked_sub(n.try_into().ok()?)?;
        let inner = ancestor_by_index(&current, target_index);

        // Advance `self.next`.
        self.next.clone_from(&inner.prev);

        Some(CheckPoint(inner))
    }

    fn last(self) -> Option<Self::Item>
    where
        Self: Sized,
    {
        Some(CheckPoint(ancestor_by_index(&self.next?, 0)))
    }

    fn count(self) -> usize
    where
        Self: Sized,
    {
        self.next
            .map_or(0, |cp_inner| (cp_inner.index as usize).saturating_add(1))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self
            .next
            .as_ref()
            .map_or(0, |cp_inner| (cp_inner.index as usize).saturating_add(1));
        (n, Some(n))
    }
}

impl<D> ExactSizeIterator for CheckPointIter<D> {}

impl<D> IntoIterator for CheckPoint<D> {
    type Item = CheckPoint<D>;
    type IntoIter = CheckPointIter<D>;

    fn into_iter(self) -> Self::IntoIter {
        CheckPointIter { next: Some(self.0) }
    }
}

/// Serializes a [`CheckPoint`] as a flat sequence of `(height, data)` pairs, ordered from tip to
/// base (descending height).
///
/// A [`CheckPoint`] is a reference-counted linked list, so a derived implementation would recurse
/// through `prev` (and `skip`) once per checkpoint and overflow the stack on long chains — the same
/// hazard that forced the hand-written [`Drop`] (see
/// <https://github.com/bitcoindevkit/bdk/issues/1634>). Iterating with [`CheckPoint::iter`] keeps
/// serialization flat, and the `skip`/`index` topology is omitted because it is rebuilt
/// deterministically on deserialization.
#[cfg(feature = "serde")]
impl<D> serde::Serialize for CheckPoint<D>
where
    D: serde::Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq;
        // `index` is the number of checkpoints below the tip, so the chain has `index + 1` nodes.
        let mut seq = serializer.serialize_seq(Some(self.0.index as usize + 1))?;
        for cp in self.iter() {
            seq.serialize_element(&(cp.height(), cp.data_ref()))?;
        }
        seq.end()
    }
}

/// Deserializes a [`CheckPoint`] from the flat sequence of `(height, data)` pairs produced by its
/// [`Serialize`](serde::Serialize) implementation.
///
/// The chain is rebuilt iteratively with [`CheckPoint::from_blocks`] (which re-derives the
/// `skip`/`index` topology), so deserialization never recurses regardless of chain length.
#[cfg(feature = "serde")]
impl<'de, D> serde::Deserialize<'de> for CheckPoint<D>
where
    D: serde::Deserialize<'de> + ToBlockHash + Clone + fmt::Debug,
{
    fn deserialize<De>(deserializer: De) -> Result<Self, De::Error>
    where
        De: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        // Serialized tip-first (descending height); `from_blocks` expects ascending order.
        let mut blocks = Vec::<(u32, D)>::deserialize(deserializer)?;
        blocks.reverse();
        CheckPoint::from_blocks(blocks).map_err(|_| {
            De::Error::custom(
                "invalid checkpoint chain: blocks must be non-empty, ascending and well-linked",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Make sure that dropping checkpoints does not result in recursion and stack overflow.
    #[test]
    fn checkpoint_drop_is_not_recursive() {
        let run = || {
            let mut cp = CheckPoint::new(0, bitcoin::hashes::Hash::hash(b"genesis"));

            for height in 1u32..=(1024 * 10) {
                let hash: BlockHash = bitcoin::hashes::Hash::hash(height.to_be_bytes().as_slice());
                cp = cp.push(height, hash).unwrap();
            }

            // `cp` would be dropped here.
        };
        std::thread::Builder::new()
            // Restrict stack size.
            .stack_size(32 * 1024)
            .spawn(run)
            .unwrap()
            .join()
            .unwrap();
    }

    /// Round-tripping a checkpoint through serde must preserve the block ids and rebuild an
    /// identical `index`/`skip` topology.
    #[cfg(feature = "serde")]
    #[test]
    fn checkpoint_serde_round_trip() {
        let mut cp = CheckPoint::new(0, bitcoin::hashes::Hash::hash(b"genesis"));
        for height in 1u32..=100 {
            let hash: BlockHash = bitcoin::hashes::Hash::hash(height.to_be_bytes().as_slice());
            cp = cp.push(height, hash).unwrap();
        }

        let json = serde_json::to_string(&cp).expect("serialization must succeed");
        let restored: CheckPoint =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(cp, restored);
        assert_eq!(cp.iter().count(), restored.iter().count());
        for (orig_cp, restored_cp) in cp.iter().zip(restored.iter()) {
            assert_eq!(orig_cp.block_id(), restored_cp.block_id());
            assert_eq!(orig_cp.index(), restored_cp.index());
            assert_eq!(
                orig_cp.skip().map(|cp| cp.block_id()),
                restored_cp.skip().map(|cp| cp.block_id()),
            );
        }
    }

    /// Serialization and deserialization must walk the linked list iteratively. A recursive
    /// implementation (e.g. a naive derive over `prev`) would overflow this deliberately small
    /// stack on a long chain — the same hazard guarded against in
    /// `checkpoint_drop_is_not_recursive`.
    #[cfg(feature = "serde")]
    #[test]
    fn checkpoint_serde_is_not_recursive() {
        let run = || {
            let mut cp = CheckPoint::new(0, bitcoin::hashes::Hash::hash(b"genesis"));
            for height in 1u32..=(1024 * 10) {
                let hash: BlockHash = bitcoin::hashes::Hash::hash(height.to_be_bytes().as_slice());
                cp = cp.push(height, hash).unwrap();
            }

            let json = serde_json::to_string(&cp).expect("serialization must not recurse");
            let restored: CheckPoint =
                serde_json::from_str(&json).expect("deserialization must not recurse");
            assert_eq!(cp, restored);
        };
        std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(run)
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn checkpoint_does_not_leak() {
        const CHAIN_LEN: u32 = 1000;

        let mut cp = CheckPoint::new(0, bitcoin::hashes::Hash::hash(b"genesis"));

        for height in 1u32..=CHAIN_LEN {
            let hash: BlockHash = bitcoin::hashes::Hash::hash(height.to_be_bytes().as_slice());
            cp = cp.push(height, hash).unwrap();
        }

        let genesis = cp.get(0).expect("genesis exists");
        let weak = Arc::downgrade(&genesis.0);

        // Expected strong references to genesis:
        //  - the `genesis` local variable
        //  - the chain `cp` via index 1's prev pointer
        //  - one skip Arc per node `i` in `1..=CHAIN_LEN` where `skip_index(i) == 0` (under pskip,
        //    those are i=1 and every power of 2 in [2, CHAIN_LEN]).
        let expected_skips_to_genesis = (1..=CHAIN_LEN).filter(|&i| skip_index(i) == 0).count();
        let expected_strong = 2 + expected_skips_to_genesis;

        assert_eq!(
            Arc::strong_count(&genesis.0),
            expected_strong,
            "`genesis`, the chain's prev to genesis, and every pskip ancestor pointing to genesis \
             should be the only strong references",
        );

        // Dropping the chain should leave `genesis` as the only remaining strong reference.
        drop(cp);
        assert_eq!(
            Arc::strong_count(&genesis.0),
            1,
            "`genesis` should be the last strong reference after `cp` is dropped",
        );

        // Dropping the final reference should deallocate the node, so the weak
        // reference cannot be upgraded.
        drop(genesis);
        assert!(
            weak.upgrade().is_none(),
            "the checkpoint node should be freed when all strong references are dropped",
        );
    }

    #[test]
    fn skip_index_formula_table() {
        // Hand-computed table of skip_index(i) for i in 0..=32, matching Bitcoin Core's
        // GetSkipHeight. This locks the bit-twiddling formula against silent breakage.
        let expected: [u32; 33] = [
            0,  // 0
            0,  // 1
            0,  // 2  -> 010 & 001 = 000
            1,  // 3  odd: ilo(ilo(2))+1 = ilo(0)+1 = 1
            0,  // 4  -> 100 & 011 = 000
            1,  // 5  odd: ilo(ilo(4))+1 = 1
            4,  // 6  -> 110 & 101 = 100
            1,  // 7  odd: ilo(ilo(6))+1 = ilo(4)+1 = 1
            0,  // 8
            1,  // 9
            8,  // 10 -> 1010 & 1001 = 1000
            1,  // 11
            8,  // 12 -> 1100 & 1011 = 1000
            1,  // 13
            12, // 14 -> 1110 & 1101 = 1100
            9,  // 15 odd: ilo(ilo(14))+1 = ilo(12)+1 = 8+1 = 9
            0,  // 16
            1,  // 17
            16, // 18
            1,  // 19
            16, // 20
            1,  // 21
            20, // 22 -> 10110 & 10101 = 10100
            17, // 23 odd: ilo(ilo(22))+1 = ilo(20)+1 = 16+1 = 17
            16, // 24
            1,  // 25
            24, // 26 -> 11010 & 11001 = 11000
            17, // 27 odd: ilo(ilo(26))+1 = ilo(24)+1 = 16+1 = 17
            24, // 28 -> 11100 & 11011 = 11000
            17, // 29 odd: ilo(ilo(28))+1 = ilo(24)+1 = 17
            28, // 30 -> 11110 & 11101 = 11100
            25, // 31 odd: ilo(ilo(30))+1 = ilo(28)+1 = 24+1 = 25
            0,  // 32
        ];
        for (i, want) in expected.iter().enumerate() {
            assert_eq!(skip_index(i as u32), *want, "skip_index({i}) mismatch");
        }
    }
}
