#![cfg(feature = "miniscript")]

//! Integration test for median-time-past (MTP) computation via
//! [`LocalChain::canonicalize_with_mtp`].

use std::collections::BTreeMap;

use bdk_chain::{local_chain::LocalChain, ChainPosition, ConfirmationBlockTime, TxGraph};
use bdk_testenv::{hash, utils::new_tx};
use bitcoin::{
    block::Header, hashes::Hash, Amount, BlockHash, OutPoint, ScriptBuf, Transaction, TxIn, TxOut,
};

/// Build a block header with an explicit `time`.
fn header(prev_blockhash: BlockHash, time: u32) -> Header {
    Header {
        version: bitcoin::block::Version::default(),
        prev_blockhash,
        merkle_root: bitcoin::hash_types::TxMerkleNode::all_zeros(),
        time,
        bits: bitcoin::CompactTarget::default(),
        nonce: 0,
    }
}

/// Build a connected chain of blocks `0..=tip_height` where each block's timestamp equals its
/// height (the block at height `h` has `time == h`).
///
/// With timestamps equal to heights, the median-time-past at height `h` — the median of the 11
/// timestamps in `h-10..=h` — is simply the middle height, `h - 5`.
fn chain_with_time_equal_to_height(tip_height: u32) -> LocalChain<Header> {
    let mut headers = vec![header(BlockHash::all_zeros(), 0)]; // genesis at height 0
    for h in 1..=tip_height {
        let prev = headers[(h - 1) as usize].block_hash();
        headers.push(header(prev, h));
    }
    let blocks: BTreeMap<u32, Header> = headers
        .into_iter()
        .enumerate()
        .map(|(height, hdr)| (height as u32, hdr))
        .collect();
    LocalChain::from_blocks(blocks).expect("chain connects from genesis")
}

/// Insert a transaction into `tx_graph` spending a distinct parent (identified by `seed`) and
/// anchor it at `height`.
///
/// Returns the txid of the inserted transaction.
fn confirm_tx_at(
    tx_graph: &mut TxGraph<ConfirmationBlockTime>,
    chain: &LocalChain<Header>,
    seed: u8,
    height: u32,
) -> bitcoin::Txid {
    let tx = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(bitcoin::Txid::from_byte_array([seed; 32]), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(1)
    };
    let txid = tx.compute_txid();
    let _ = tx_graph.insert_tx(tx);
    let _ = tx_graph.insert_anchor(
        txid,
        ConfirmationBlockTime {
            block_id: chain.get(height).unwrap().block_id(),
            confirmation_time: 0,
        },
    );
    txid
}

#[test]
fn canonicalize_with_mtp_computes_median_time_past() {
    // A 13-block chain (heights 0..=12) where each block's time equals its height.
    let chain = chain_with_time_equal_to_height(12);

    // A single transaction confirmed at height 11.
    let mut tx_graph = TxGraph::default();
    let tx = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint::new(hash!("parent"), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: ScriptBuf::new(),
        }],
        ..new_tx(1)
    };
    let txid = tx.compute_txid();
    let _ = tx_graph.insert_tx(tx);
    let _ = tx_graph.insert_anchor(
        txid,
        ConfirmationBlockTime {
            block_id: chain.get(11).unwrap().block_id(),
            confirmation_time: 0,
        },
    );

    // Canonicalize with MTP enabled.
    let view = chain.canonicalize_with_mtp(&tx_graph, chain.tip().block_id(), Default::default());

    // Tip is at height 12, so its MTP is the median of heights 2..=12, which is 7.
    assert_eq!(view.tip_mtp(), Some(7));

    // The transaction is confirmed at height 11. Its output's `prev_mtp` is the MTP of the
    // block *preceding* the confirmation height (BIP-68), i.e. MTP at height 10 = median of
    // heights 0..=10 == 5. (Was 6 when the buggy code medianed heights 1..=11 at height 11.)
    let canonical_tx = view
        .txs()
        .find(|c| c.txid == txid)
        .expect("tx is canonical");
    assert!(matches!(canonical_tx.pos, ChainPosition::Confirmed { .. }));
    let txo = view.txout(OutPoint::new(txid, 0)).expect("output exists");
    assert_eq!(txo.prev_mtp, Some(5));
}

#[test]
fn prev_mtp_saturates_for_low_confirmation_heights() {
    // A 13-block chain (heights 0..=12) where each block's time equals its height.
    let chain = chain_with_time_equal_to_height(12);
    let mut tx_graph = TxGraph::default();

    // Confirmed at genesis (height 0): `confirmation_height - 1` saturates to 0, matching
    // Core's `std::max(nCoinHeight - 1, 0)`. prev_mtp = MTP(0) = median of heights 0..=0 == 0.
    let genesis_txid = confirm_tx_at(&mut tx_graph, &chain, 1, 0);

    // Confirmed at height 3: prev_mtp = MTP(2) = median of heights 0..=2 == 1. The MTP window
    // start (`2 - 10`) also saturates to 0. (The buggy code would have medianed heights 0..=3
    // at height 3 == 2.)
    let low_txid = confirm_tx_at(&mut tx_graph, &chain, 2, 3);

    let view = chain.canonicalize_with_mtp(&tx_graph, chain.tip().block_id(), Default::default());

    let genesis_txo = view
        .txout(OutPoint::new(genesis_txid, 0))
        .expect("output exists");
    assert_eq!(genesis_txo.prev_mtp, Some(0));

    let low_txo = view
        .txout(OutPoint::new(low_txid, 0))
        .expect("output exists");
    assert_eq!(low_txo.prev_mtp, Some(1));
}

#[test]
fn prev_mtp_differs_from_tip_mtp() {
    // A 13-block chain (heights 0..=12) where each block's time equals its height.
    let chain = chain_with_time_equal_to_height(12);
    let mut tx_graph = TxGraph::default();

    // Confirmed at height 11, with the tip at height 12: prev_mtp = MTP(10) = 5, while
    // tip_mtp = MTP(12) = 7. These are distinguishable, so `prev_mtp` (BIP-68, the block before
    // the coin) is clearly not the same reference as `tip_mtp` (BIP-113, the tip).
    let txid = confirm_tx_at(&mut tx_graph, &chain, 1, 11);

    let view = chain.canonicalize_with_mtp(&tx_graph, chain.tip().block_id(), Default::default());

    let txo = view.txout(OutPoint::new(txid, 0)).expect("output exists");
    assert_eq!(txo.prev_mtp, Some(5));
    assert_eq!(view.tip_mtp(), Some(7));
    assert_ne!(txo.prev_mtp, view.tip_mtp());
}

#[test]
fn sparse_chain_yields_no_mtp_but_still_canonicalizes() {
    // A chain holding only heights {0, 50, 100} — the shape a synced `LocalChain` has, since
    // electrum/esplora insert a handful of checkpoints rather than every height.
    let blocks: BTreeMap<u32, Header> = {
        let genesis = header(BlockHash::all_zeros(), 0);
        // The heights are not adjacent, so `prev_blockhash` linkage between them is arbitrary;
        // `LocalChain` only requires that it connects from genesis.
        let at_50 = header(genesis.block_hash(), 50);
        let at_100 = header(at_50.block_hash(), 100);
        [(0, genesis), (50, at_50), (100, at_100)].into()
    };
    let chain = LocalChain::from_blocks(blocks).expect("chain connects from genesis");
    let mut tx_graph = TxGraph::default();

    let txid = confirm_tx_at(&mut tx_graph, &chain, 1, 50);

    let view = chain.canonicalize_with_mtp(&tx_graph, chain.tip().block_id(), Default::default());

    // Canonicalization itself is unaffected by the gaps.
    let canonical_tx = view
        .txs()
        .find(|c| c.txid == txid)
        .expect("tx is canonical");
    assert!(matches!(canonical_tx.pos, ChainPosition::Confirmed { .. }));

    // But MTP needs *every* height in an 11-block window: `prev_mtp` would need 39..=49 and
    // `tip_mtp` would need 90..=100, none of which this chain holds. Both are silently `None`
    // rather than an error — a gap is indistinguishable from a reorged-out block.
    let txo = view.txout(OutPoint::new(txid, 0)).expect("output exists");
    assert_eq!(txo.prev_mtp, None);
    assert_eq!(view.tip_mtp(), None);
}
