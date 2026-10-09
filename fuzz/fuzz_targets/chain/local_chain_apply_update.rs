#![cfg_attr(feature = "libfuzzer_fuzz", no_main)]

use bdk_chain::bitcoin::hashes::Hash;
use bdk_chain::bitcoin::BlockHash;
use bdk_chain::local_chain::{LocalChain, MissingGenesisError};
use bdk_chain::BlockId;
use bdk_fuzz::chain::arbitrary::{self, Arbitrary, Unstructured};
use bdk_fuzz::chain::checks::{
    assert_changeset_against_chains, assert_changeset_applied, assert_checkpoint_order,
    assert_initial_changeset_roundtrip,
};

/// An operation to perform against the chain under test.
#[derive(Arbitrary, Debug, Clone, Copy)]
enum Op {
    /// `apply_update` with an independently constructed chain as the update.
    ApplyUpdate,
    /// `insert_block` with an arbitrary height and hash.
    InsertBlock,
    /// `disconnect_from` an existing checkpoint or an arbitrary block id.
    DisconnectFrom,
    /// `apply_header` with a header that usually connects to an existing checkpoint.
    ApplyHeader,
    /// `apply_header_connected_to` with an arbitrarily picked connection point.
    ApplyHeaderConnectedTo,
    /// `apply_update` with an update derived by mutating the chain's own tip, so the
    /// update shares `Arc` nodes with the original and exercises `merge_chains`'
    /// `eq_ptr` fast path.
    ApplyDerivedUpdate,
    /// `apply_changeset` with an arbitrary mix of insertions and removals.
    ApplyChangeSet,
}

fn assert_chain(chain: &LocalChain) {
    assert_checkpoint_order(chain);
    assert_initial_changeset_roundtrip(chain);

    let tip = chain.chain_tip();
    assert_eq!(tip, chain.tip().block_id());

    for cp in chain.iter_checkpoints() {
        assert_eq!(
            chain.is_block_in_chain(cp.block_id(), tip),
            Some(true),
            "every checkpoint must be in the chain of its own tip"
        );

        let mut flipped = cp.hash().to_byte_array();
        flipped[0] ^= 1;

        let wrong = BlockId {
            height: cp.height(),
            hash: BlockHash::from_byte_array(flipped),
        };

        assert_eq!(
            chain.is_block_in_chain(wrong, tip),
            Some(false),
            "a conflicting hash at an occupied height must not be in chain"
        );
    }
}

fn do_test(data: &[u8]) {
    let mut u = Unstructured::new(data);

    // TODO: (@oleonardolima) I think we should definitely increase this to do more operations.
    let op_count = match u.int_in_range(1..=16) {
        Ok(count) => count,
        Err(_) => return,
    };

    let mut chain: Option<LocalChain> = None;
    for _ in 0..op_count {
        if chain.is_none() {
            match arbitrary::blockhash_chain(&mut u) {
                Ok(Some(initial)) => chain = Some(initial),
                Ok(None) => continue,
                Err(_) => break,
            }
            continue;
        }

        let chain = chain.as_mut().expect("It SHOULD be initialized above!");
        let prev_chain = chain.clone();
        let genesis = chain.genesis_hash();

        let op = match Op::arbitrary(&mut u) {
            Ok(op) => op,
            Err(_) => break,
        };

        match op {
            Op::ApplyUpdate => {
                let update = match arbitrary::blockhash_chain(&mut u) {
                    Ok(Some(update)) => update,
                    Ok(None) => continue,
                    Err(_) => break,
                };
                let result = chain.apply_update(update.tip());
                assert_changeset_against_chains(prev_chain, chain, &result);
            }
            Op::InsertBlock => {
                let (height, hash) = match u32::arbitrary(&mut u)
                    .and_then(|height| arbitrary::block_hash(&mut u).map(|hash| (height, hash)))
                {
                    Ok(block) => block,
                    Err(_) => break,
                };

                let result = chain.insert_block(height, hash);
                assert_changeset_against_chains(prev_chain, chain, &result);

                match &result {
                    Ok(_) => {
                        assert_eq!(chain.get(height).map(|cp| cp.hash()), Some(hash));
                    }
                    Err(err) => {
                        assert_eq!(
                            chain.get(err.height).map(|cp| cp.hash()),
                            Some(err.original_hash),
                            "insert conflict must report the existing checkpoint"
                        );
                    }
                }
            }
            Op::DisconnectFrom => {
                let block_id = match arbitrary::block_id(&mut u, chain, &[]) {
                    Ok(block_id) => block_id,
                    Err(_) => break,
                };

                let result = chain.disconnect_from(block_id);
                assert_changeset_against_chains(prev_chain, chain, &result);

                match &result {
                    Ok(changeset) if !changeset.blocks.is_empty() => {
                        assert!(chain.tip().height() < block_id.height);
                    }
                    Ok(_) => {}
                    Err(MissingGenesisError) => {
                        assert_eq!(block_id.height, 0);
                        assert_eq!(block_id.hash, chain.genesis_hash());
                    }
                }
            }
            Op::ApplyHeader => {
                let (header, height) = match arbitrary::connectable_header(&mut u, chain) {
                    Ok(header) => header,
                    Err(_) => break,
                };

                let result = chain.apply_header(&header, height);
                assert_changeset_against_chains(prev_chain, chain, &result);

                if result.is_ok() {
                    assert_eq!(
                        chain.get(height).map(|cp| cp.hash()),
                        Some(header.block_hash())
                    );
                }
            }
            Op::ApplyHeaderConnectedTo => {
                let params: arbitrary::Result<_> = (|| {
                    let (header, height) = arbitrary::connectable_header(&mut u, chain)?;
                    let connected_to = arbitrary::block_id(&mut u, chain, &[])?;
                    Ok((header, height, connected_to))
                })();

                let (header, height, connected_to) = match params {
                    Ok(params) => params,
                    Err(_) => break,
                };

                let result = chain.apply_header_connected_to(&header, height, connected_to);
                assert_changeset_against_chains(prev_chain, chain, &result);

                if result.is_ok() {
                    assert_eq!(
                        chain.get(height).map(|cp| cp.hash()),
                        Some(header.block_hash())
                    );
                }
            }
            Op::ApplyDerivedUpdate => {
                let params: arbitrary::Result<_> = (|| {
                    let insert = bool::arbitrary(&mut u)?;
                    let height = u32::arbitrary(&mut u)?;
                    let hash = arbitrary::block_hash(&mut u)?;
                    Ok((insert, height, hash))
                })();

                let (insert, height, hash) = match params {
                    Ok(params) => params,
                    Err(_) => break,
                };

                let (update_tip, height) = match insert {
                    true => {
                        // Height 0 would panic (genesis is immutable in `CheckPoint::insert`).
                        let height = height.max(1);
                        (chain.tip().insert(height, hash), height)
                    }
                    false => {
                        let height = match chain.tip().height().checked_add(1 + height % 4) {
                            Some(height) => height,
                            None => continue,
                        };
                        match chain.tip().extend([(height, hash)]) {
                            Ok(tip) => (tip, height),
                            Err(_) => continue,
                        }
                    }
                };

                let result = chain.apply_update(update_tip);
                assert_changeset_against_chains(prev_chain, chain, &result);

                if result.is_ok() {
                    assert_eq!(chain.get(height).map(|cp| cp.hash()), Some(hash));
                }
            }
            Op::ApplyChangeSet => {
                let changeset = match arbitrary::changeset(&mut u, chain, |u, _height| {
                    arbitrary::block_hash(u)
                }) {
                    Ok(changeset) => changeset,
                    Err(_) => break,
                };

                let result = chain.apply_changeset(&changeset);
                assert_changeset_applied(&prev_chain, chain, &changeset, &result);
            }
        }

        assert_eq!(
            genesis,
            chain.genesis_hash(),
            "NO operation SHOULD replace the genesis block!"
        );

        assert_chain(chain);
    }

    if let Some(chain) = chain {
        assert_chain(&chain);
    }
}

bdk_fuzz::fuzz_main!(do_test);
