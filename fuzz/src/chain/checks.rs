//! Invariant checks the harnesses run after each operation.

use bdk_chain::local_chain::{ApplyBlockError, ChangeSet, LocalChain};
use bdk_chain::{BlockId, ToBlockHash};

/// Checks the [`ChangeSet`] returned by an operation against the [`LocalChain`] states
/// surrounding it.
///
/// On success, applying the returned [`ChangeSet`] to `chain_before` must reconstruct
/// `chain_after`. On failure, the operation must not have modified the chain, so
/// `chain_before` and `chain_after` must be equal.
pub fn assert_changeset_against_chains<D, E>(
    chain_before: LocalChain<D>,
    chain_after: &LocalChain<D>,
    op_result: &Result<ChangeSet<D>, E>,
) where
    D: ToBlockHash + std::fmt::Debug + Clone,
{
    match op_result {
        Ok(changeset) => {
            let mut reconstructed = chain_before;
            reconstructed
                .apply_changeset(changeset)
                .expect("applying an op's changeset to the pre-state must succeed");
            assert_eq!(&reconstructed, chain_after);
        }
        Err(_) => assert_eq!(
            &chain_before, chain_after,
            "a failed op must not modify the chain"
        ),
    }
}

/// Checks an `apply_changeset` call against the [`LocalChain`] states surrounding it.
///
/// On success, every insertion must be in `chain_after` at its height, every removal must be
/// gone, the checkpoints below the changeset must be untouched, and re-applying must be a no-op
/// (`apply_changeset` is idempotent: the second call recomputes the same extension). On failure,
/// the chain must be unmodified.
pub fn assert_changeset_applied<D>(
    chain_before: &LocalChain<D>,
    chain_after: &LocalChain<D>,
    changeset: &ChangeSet<D>,
    result: &Result<(), ApplyBlockError>,
) where
    D: ToBlockHash + std::fmt::Debug + Clone,
{
    if result.is_err() {
        assert_eq!(
            chain_before, chain_after,
            "a failed apply must not modify the chain"
        );
        return;
    }

    for (&height, data) in &changeset.blocks {
        let cp_hash = chain_after.get(height).map(|cp| cp.hash());
        match data {
            Some(data) => assert_eq!(
                cp_hash,
                Some(data.to_blockhash()),
                "an inserted block must be in the chain at its height"
            ),
            None => assert_eq!(cp_hash, None, "a removed block must not be in the chain"),
        }
    }

    // The changeset's lowest height is the point of agreement: nothing below it can move.
    if let Some(&start_height) = changeset.blocks.keys().next() {
        let block_ids = |chain: &LocalChain<D>| -> Vec<BlockId> {
            chain
                .range(..start_height)
                .map(|cp| cp.block_id())
                .collect()
        };
        assert_eq!(
            block_ids(chain_before),
            block_ids(chain_after),
            "blocks below the changeset must be untouched"
        );
    }

    let mut reapplied = chain_after.clone();
    reapplied
        .apply_changeset(changeset)
        .expect("re-applying an applied changeset must succeed");
    assert_eq!(
        &reapplied, chain_after,
        "applying a changeset twice must equal applying it once"
    );
}

/// Checks that the [`LocalChain`] is recoverable from its own initial [`ChangeSet`].
pub fn assert_initial_changeset_roundtrip<D>(chain: &LocalChain<D>)
where
    D: ToBlockHash + std::fmt::Debug + Clone,
{
    let recovered = LocalChain::from_changeset(chain.initial_changeset())
        .expect("a chain's initial changeset must rebuild it");
    assert_eq!(&recovered, chain);
}

/// Checks that `CheckPoint` heights strictly decrease from tip and genesis is present.
pub fn assert_checkpoint_order<D>(chain: &LocalChain<D>)
where
    D: ToBlockHash + std::fmt::Debug + Clone,
{
    let heights: Vec<u32> = chain.iter_checkpoints().map(|cp| cp.height()).collect();
    assert!(
        heights.windows(2).all(|w| w[0] > w[1]),
        "checkpoint heights must be strictly decreasing from tip"
    );
    assert_eq!(heights.last(), Some(&0), "genesis must be present");
}
