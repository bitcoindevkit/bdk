//! The [`PoWChain`] is an implementation of [`ChainOracle`] that verifies proof of work.

use crate::{BlockId, ChainOracle, CheckPoint, Merge};
use bdk_core::ToBlockHash;
use bitcoin::{block::Header, BlockHash, Target};
use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::vec::Vec;

//Iterator<Item = (height, BlockHash, Target)>,
/// Apply `changeset` to the checkpoint.
fn apply_changeset_to_checkpoint(
    mut init_cp: CheckPoint<Header>,
    changeset: &ChangeSet,
    difficulty_map: HashMap<BlockHash, Target>,
) -> Result<CheckPoint<Header>, MissingGenesisError> {
    if let Some(start_height) = changeset.blocks.keys().next().cloned() {
        // changes after point of agreement
        let mut extension = BTreeMap::default();
        // point of agreement
        let mut base: Option<CheckPoint<Header>> = None;

        for cp in init_cp.iter() {
            if cp.height() >= start_height {
                extension.insert(cp.height(), (*cp.data(), difficulty_map.get(&cp.hash())));
            } else {
                base = Some(cp);
                break;
            }
        }

        for (&height, &data) in &changeset.blocks {
            match data {
                Some(data) => {
                    extension.insert(height, data);
                }
                None => {
                    extension.remove(&height);
                }
            };
        }

        let new_tip = match base {
            Some(base) => base
                .extend_data(extension)
                .expect("extension is strictly greater than base"),
            None => PoWChain::from_data(extension, extension.get(&0).unwrap().0)?.tip(),
        };
        init_cp = new_tip;
    }

    Ok(init_cp)
}

/// This is an implementation of [`ChainOracle`] that verifies proof of work.
#[derive(Debug, Clone, PartialEq)]
pub struct PoWChain {
    // Stores the difficulty of each block.
    difficulty_map: HashMap<BlockHash, Target>,
    // List of trusted blocks.
    trusted_blocks: Vec<BlockId>,
    // A vector which keeps track of all chain tips.
    all_tips: Vec<CheckPoint<Header>>,
    // Current best chain.
    tip: CheckPoint<Header>,
}

impl ChainOracle for PoWChain {
    type Error = Infallible;

    fn is_block_in_chain(
        &self,
        block: bdk_core::BlockId,
        chain_tip: bdk_core::BlockId,
    ) -> Result<Option<bool>, Self::Error> {
        let chain_tip_cp = match self.tip.get(chain_tip.height) {
            // we can only determine whether `block` is in chain of `chain_tip` if `chain_tip` can
            // be identified in chain
            Some(cp) if cp.hash() == chain_tip.hash => cp,
            _ => return Ok(None),
        };
        match chain_tip_cp.get(block.height) {
            Some(cp) => Ok(Some(cp.hash() == block.hash)),
            None => Ok(None),
        }
    }

    fn get_chain_tip(&self) -> Result<BlockId, Self::Error> {
        Ok(self.tip.block_id())
    }
}

impl PoWChain {
    /// Get the genesis hash.
    pub fn genesis_hash(&self) -> BlockHash {
        self.tip.get(0).expect("genesis must exist").hash()
    }

    /// Construct [`PoWChain`] from genesis `Header`.
    #[must_use]
    pub fn from_genesis_header(header: Header) -> (Self, ChangeSet) {
        let tip = CheckPoint::from_data(0, header);
        let chain = Self {
            difficulty_map: HashMap::from([(header.to_blockhash(), Target::MAX)]),
            trusted_blocks: Vec::default(),
            all_tips: vec![tip.clone()],
            tip,
        };
        let changeset = chain.initial_changeset();
        (chain, changeset)
    }

    /// Construct a [`PoWChain`] from an initial `changeset`.
    pub fn from_changeset(changeset: ChangeSet) -> Result<Self, MissingGenesisError> {
        let genesis_entry = changeset.blocks.get(&0).copied().flatten();
        let genesis_header = match genesis_entry {
            Some(header) => header,
            None => return Err(MissingGenesisError),
        };

        let (mut chain, _) = Self::from_genesis_header(genesis_header);
        chain.apply_changeset(&changeset)?;

        debug_assert!(chain._check_changeset_is_applied(&changeset));

        Ok(chain)
    }

    /// Initialize [`PoWChain`] with specified data.
    pub fn from_data(
        input: impl Iterator<Item = (u32, BlockHash, Target)>,
        genesis_header: Header,
    ) -> Result<Self, std::boxed::Box<dyn std::error::Error>> {
        let tip = CheckPoint::<Header>::from_data(0, genesis_header);

        let mut difficulty_map = HashMap::new();
        let mut trusted_blocks = Vec::new();

        for (height, hash, target) in input {
            difficulty_map.insert(hash, Target::from_hex(&format!("{:x}", target))?);
            trusted_blocks.push(BlockId { height, hash });
        }

        Ok(PoWChain {
            difficulty_map,
            trusted_blocks,
            all_tips: vec![tip.clone()],
            tip,
        })
    }

    /// Load known checkpoints from external `checkpoints.json` file.
    pub fn from_electrum_json(
        &self,
        path: &str,
        genesis_header: Header,
    ) -> Result<Self, std::boxed::Box<dyn std::error::Error>> {
        let tip = CheckPoint::<Header>::from_data(0, genesis_header);

        let mut difficulty_map = HashMap::new();
        let mut trusted_blocks = Vec::new();

        // Populate a vector of (blockhash, difficulty target) from external `checkpoints.json`
        // file.
        let file = std::fs::File::open(path)?;
        let checkpoint_entries: Vec<(BlockHash, u64)> = serde_json::from_reader(file)?;

        // Calculate heights based on the starting height of first block in `checkpoints.json`,
        // which is block 2015. Subsequent blocks have height gaps of 2016, which is the number of
        // blocks between each difficulty adjustment.
        let mut height = 2015;

        for (hash, target) in checkpoint_entries {
            difficulty_map.insert(hash, Target::from_hex(&format!("{:x}", target))?);
            trusted_blocks.push(BlockId { height, hash });

            // Increment height by 2016, which is the number of blocks between each difficulty
            // adjustment.
            height += 2016;
        }

        Ok(PoWChain {
            difficulty_map,
            trusted_blocks,
            all_tips: vec![tip.clone()],
            tip,
        })
    }

    /// Get the highest checkpoint.
    pub fn tip(&self) -> CheckPoint<Header> {
        self.tip.clone()
    }

    /// Apply the given `changeset`.
    pub fn apply_changeset(&mut self, changeset: &ChangeSet) -> Result<(), MissingGenesisError> {
        let old_tip = self.tip.clone();
        let new_tip =
            apply_changeset_to_checkpoint(old_tip, changeset, self.difficulty_map.clone())?;
        self.tip = new_tip;
        debug_assert!(self._check_changeset_is_applied(changeset));
        Ok(())
    }

    /// Derives an initial [`ChangeSet`], meaning that it can be applied to an empty chain to
    /// recover the current chain.
    pub fn initial_changeset(&self) -> ChangeSet {
        ChangeSet {
            blocks: self
                .tip
                .iter()
                .map(|cp| (cp.height(), Some(*cp.data())))
                .collect(),
        }
    }

    fn _check_changeset_is_applied(&self, changeset: &ChangeSet) -> bool {
        let mut curr_cp = self.tip.clone();
        for (height, exp_header) in changeset.blocks.iter().rev() {
            match curr_cp.get(*height) {
                Some(query_cp) => {
                    if query_cp.height() != *height || Some(*query_cp.data()) != *exp_header {
                        return false;
                    }
                    curr_cp = query_cp;
                }
                None => {
                    if exp_header.is_some() {
                        return false;
                    }
                }
            }
        }
        true
    }

    // based on current height we should know if we already have current difficulty or not
    // account for possibility of reorged chunk of blocks containing difficulty adjustment block
    // def get_target(self, index: int) -> int:
    // # compute target from chunk x, used in chunk x+1
    // if constants.net.TESTNET:
    //     return 0
    // if index == -1:
    //     return MAX_TARGET
    // if index < len(self.checkpoints):
    //     h, t = self.checkpoints[index]
    //     return t
    // # new target
    // first = self.read_header(index * 2016)
    // last = self.read_header(index * 2016 + 2015)
    // if not first or not last:
    //     raise MissingHeader()
    // bits = last.get('bits')
    // target = self.bits_to_target(bits)
    // nActualTimespan = last.get('timestamp') - first.get('timestamp')
    // nTargetTimespan = 14 * 24 * 60 * 60
    // nActualTimespan = max(nActualTimespan, nTargetTimespan // 4)
    // nActualTimespan = min(nActualTimespan, nTargetTimespan * 4)
    // new_target = min(MAX_TARGET, (target * nActualTimespan) // nTargetTimespan)
    // # not any target can be represented in 32 bits:
    // new_target = self.bits_to_target(self.target_to_bits(new_target))
    // return new_target

    /// Verifies the proof of work for a given `Header`. If the block does not meet the difficulty
    /// target, it is discarded.
    pub fn verify_pow(&self, header: Header) -> Result<BlockHash, ()> {
        self.difficulty_map
            .get(&header.block_hash())
            .and_then(|&target| header.validate_pow(target).ok())
            .ok_or(())
    }

    /// Calculates and sets the best chain from all current chain tips.
    pub fn calculate_best_chain(&mut self) {
        if let Some(best_tip) = self.all_tips.iter().min_by(|&cp1, &cp2| {
            // If no difficulty target is found, target is set to maximum to represent minimum
            // difficulty.
            let difficulty1 = self.difficulty_map.get(&cp1.hash()).unwrap_or(&Target::MAX);
            let difficulty2 = self.difficulty_map.get(&cp2.hash()).unwrap_or(&Target::MAX);

            // Compare chain tips by difficulty. Return the chain with lower target which represents
            // the chain with higher difficulty.
            let difficulty_cmp = difficulty1.cmp(difficulty2);

            // If chain tips have the same difficulty, compare by height. Since we are using
            // `min_by`, `cp1` and `cp2` ordering is reversed to return the higher height.
            if difficulty_cmp == std::cmp::Ordering::Equal {
                cp2.height().cmp(&cp1.height())
            } else {
                difficulty_cmp
            }
        }) {
            self.tip = best_tip.clone();
        }
    }

    // Applies an update tip to its pertinent chain in `all_tips`. Returns the best chain and
    // corresponding `ChangeSet`.
    pub fn merge_update(
        &mut self,
        update_tip: CheckPoint<Header>,
    ) -> Result<(CheckPoint<Header>, ChangeSet), CannotConnectError> {
        let mut changeset = ChangeSet::default();
        let mut tip: Option<CheckPoint<Header>> = None;

        // Attempt to merge update with all known tips
        for original_tip in self.all_tips.iter_mut() {
            match merge_chains(original_tip.clone(), update_tip.clone()) {
                // Update the particular tip if merge is successful.
                Ok((new_tip, new_changeset)) => {
                    *original_tip = new_tip.clone();
                    tip = Some(new_tip);
                    changeset.merge(new_changeset);
                    break;
                }
                Err(_) => continue,
            }
        }

        // TODO: ChangeSet needs to be checked for missing heights, if new
        // best chain has higher starting height.
        if let Some(tip) = tip {
            // Purge subsets after attempting to merge
            self.purge_subsets();
            
            self.calculate_best_chain();
            // If merged tip is not new best chain, do not return a `ChangeSet`.
            if self.tip() != tip {
                return Ok((self.tip(), ChangeSet::default()));
            }
            return Ok((tip, changeset));
        }

        // If update tip is not merged, return old best tip and empty `ChangeSet`.
        Ok((self.tip(), ChangeSet::default()))
    }

    // Purge tips that are complete subsets of another tip from `all_tips`.
    fn purge_subsets(&mut self) {
        let tips: Vec<CheckPoint<Header>> = self.all_tips.clone();
    
        // Compare tips inside of `all_tips` and filter out any subsets.
        // TODO: Edge case where two CheckPoints have the same tip but different lengths?
        self.all_tips = tips
            .iter()
            .filter(|tip| {
                !tips.iter().any(|other_tip| *tip != other_tip && self.is_subset(tip, other_tip))
            })
            .cloned()
            .collect();
    }

    // Determine if a `CheckPoint` is a subset of another `CheckPoint`.
    fn is_subset(&self, cp1: &CheckPoint<Header>, cp2: &CheckPoint<Header>) -> bool {
        let subset = cp1.iter().collect::<Vec<_>>();
        let superset = cp2.iter().collect::<Vec<_>>();

        // Ensure the subset is smaller.
        if subset.len() >= superset.len() {
            return false;
        }

        // Check if all elements of subset are contained in superset.
        subset.iter().all(|elem| superset.contains(elem))
    }
}

/// The [`ChangeSet`] represents changes to [`PoWChain`].
#[derive(Debug, Default, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct ChangeSet {
    /// Changes to the [`PoWChain`] blocks.
    ///
    /// The key represents the block height, and the value either represents added a new [`CheckPoint`]
    /// (if [`Some`]), or removing a [`CheckPoint`] (if [`None`]).
    pub blocks: BTreeMap<u32, Option<Header>>,
}

impl Merge for ChangeSet {
    fn merge(&mut self, other: Self) {
        Merge::merge(&mut self.blocks, other.blocks)
    }

    fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// An error which occurs when a [`PoWChain`] is constructed without a genesis checkpoint.
#[derive(Clone, Debug, PartialEq)]
pub struct MissingGenesisError;

impl core::fmt::Display for MissingGenesisError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "cannot construct `LocalChain` without a genesis checkpoint"
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for MissingGenesisError {}

/// Occurs when an update does not have a common checkpoint with the original chain.
#[derive(Clone, Debug, PartialEq)]
pub struct CannotConnectError {
    /// The suggested checkpoint to include to connect the two chains.
    pub try_include_height: u32,
}

impl core::fmt::Display for CannotConnectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "introduced chain cannot connect with the original chain, try include height {}",
            self.try_include_height,
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for CannotConnectError {}

/// The error type for [`LocalChain::apply_header_connected_to`].
#[derive(Debug, Clone, PartialEq)]
pub enum ApplyHeaderError {
    /// Occurs when `connected_to` block conflicts with either the current block or previous block.
    InconsistentBlocks,
    /// Occurs when the update cannot connect with the original chain.
    CannotConnect(CannotConnectError),
}

impl core::fmt::Display for ApplyHeaderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ApplyHeaderError::InconsistentBlocks => write!(
                f,
                "the `connected_to` block conflicts with either the current or previous block"
            ),
            ApplyHeaderError::CannotConnect(err) => core::fmt::Display::fmt(err, f),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ApplyHeaderError {}

/// Applies `update_tip` onto `original_tip`.
///
/// On success, a tuple is returned `(changeset, can_replace)`. If `can_replace` is true, then the
/// `update_tip` can replace the `original_tip`.
fn merge_chains(
    original_tip: CheckPoint<Header>,
    update_tip: CheckPoint<Header>,
) -> Result<(CheckPoint<Header>, ChangeSet), CannotConnectError> {
    let mut changeset = ChangeSet::default();
    let mut orig = original_tip.iter();
    let mut update = update_tip.iter();
    let mut curr_orig = None;
    let mut curr_update = None;
    let mut prev_orig: Option<CheckPoint<Header>> = None;
    let mut prev_update: Option<CheckPoint<Header>> = None;
    let mut point_of_agreement_found = false;
    let mut prev_orig_was_invalidated = false;
    let mut potentially_invalidated_heights = vec![];

    // If we can, we want to return the update tip as the new tip because this allows checkpoints
    // in multiple locations to keep the same `Arc` pointers when they are being updated from each
    // other using this function. We can do this as long as long as the update contains every
    // block's height of the original chain.
    let mut is_update_height_superset_of_original = true;

    // To find the difference between the new chain and the original we iterate over both of them
    // from the tip backwards in tandem. We always dealing with the highest one from either chain
    // first and move to the next highest. The crucial logic is applied when they have blocks at the
    // same height.
    loop {
        if curr_orig.is_none() {
            curr_orig = orig.next();
        }
        if curr_update.is_none() {
            curr_update = update.next();
        }

        match (curr_orig.as_ref(), curr_update.as_ref()) {
            // Update block that doesn't exist in the original chain
            (o, Some(u)) if Some(u.height()) > o.map(|o| o.height()) => {
                changeset.blocks.insert(u.height(), Some(*u.data()));
                prev_update = curr_update.take();
            }
            // Original block that isn't in the update
            (Some(o), u) if Some(o.height()) > u.map(|u| u.height()) => {
                // this block might be gone if an earlier block gets invalidated
                potentially_invalidated_heights.push(o.height());
                prev_orig_was_invalidated = false;
                prev_orig = curr_orig.take();

                is_update_height_superset_of_original = false;

                // OPTIMIZATION: we have run out of update blocks so we don't need to continue
                // iterating because there's no possibility of adding anything to changeset.
                if u.is_none() {
                    break;
                }
            }
            (Some(o), Some(u)) => {
                if o.hash() == u.hash() {
                    // We have found our point of agreement 🎉 -- we require that the previous (i.e.
                    // higher because we are iterating backwards) block in the original chain was
                    // invalidated (if it exists). This ensures that there is an unambiguous point of
                    // connection to the original chain from the update chain (i.e. we know the
                    // precisely which original blocks are invalid).
                    if !prev_orig_was_invalidated && !point_of_agreement_found {
                        if let (Some(prev_orig), Some(_prev_update)) = (&prev_orig, &prev_update) {
                            return Err(CannotConnectError {
                                try_include_height: prev_orig.height(),
                            });
                        }
                    }
                    point_of_agreement_found = true;
                    prev_orig_was_invalidated = false;
                    // OPTIMIZATION 2 -- if we have the same underlying pointer at this point, we
                    // can guarantee that no older blocks are introduced.
                    if o.eq_ptr(u) {
                        if is_update_height_superset_of_original {
                            return Ok((update_tip, changeset));
                        } else {
                            let new_tip = apply_changeset_to_checkpoint(original_tip, &changeset)
                                .map_err(|_| CannotConnectError {
                                try_include_height: 0,
                            })?;
                            return Ok((new_tip, changeset));
                        }
                    }
                } else {
                    // We have an invalidation height so we set the height to the updated hash and
                    // also purge all the original chain block hashes above this block.
                    changeset.blocks.insert(u.height(), Some(*u.data()));
                    for invalidated_height in potentially_invalidated_heights.drain(..) {
                        changeset.blocks.insert(invalidated_height, None);
                    }
                    prev_orig_was_invalidated = true;
                }
                prev_update = curr_update.take();
                prev_orig = curr_orig.take();
            }
            (None, None) => {
                break;
            }
            _ => {
                unreachable!("compiler cannot tell that everything has been covered")
            }
        }
    }

    // When we don't have a point of agreement you can imagine it is implicitly the
    // genesis block so we need to do the final connectivity check which in this case
    // just means making sure the entire original chain was invalidated.
    if !prev_orig_was_invalidated && !point_of_agreement_found {
        if let Some(prev_orig) = prev_orig {
            return Err(CannotConnectError {
                try_include_height: prev_orig.height(),
            });
        }
    }

    let new_tip = apply_changeset_to_checkpoint(original_tip, &changeset).map_err(|_| {
        CannotConnectError {
            try_include_height: 0,
        }
    })?;
    Ok((new_tip, changeset))
}
