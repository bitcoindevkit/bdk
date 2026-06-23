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

    // The transaction is confirmed at height 11, so its MTP is the median of heights 1..=11 == 6.
    let canonical_tx = view
        .txs()
        .find(|c| c.txid == txid)
        .expect("tx is canonical");
    assert!(matches!(canonical_tx.pos, ChainPosition::Confirmed { .. }));
    assert_eq!(canonical_tx.mtp, Some(6));
}
