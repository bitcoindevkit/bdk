//! `bdk_wallet` test utilities

use alloc::string::ToString;
use alloc::sync::Arc;
use core::str::FromStr;

use bdk_chain::{BlockId, ConfirmationBlockTime};
use bitcoin::{
    absolute, hashes::Hash, transaction, Address, Amount, BlockHash, FeeRate, Network, OutPoint,
    Transaction, TxIn, TxOut, Txid,
};

use crate::{KeychainKind, Update, Wallet};

/// Return a fake wallet that appears to be funded for testing.
///
/// The funded wallet contains a tx with a 76_000 sats input and two outputs, one spending 25_000
/// to a foreign address and one returning 50_000 back to the wallet. The remaining 1000
/// sats are the transaction fee.
pub fn get_funded_wallet(descriptor: &str, change_descriptor: &str) -> (Wallet, Txid) {
    new_funded_wallet(descriptor, Some(change_descriptor))
}

fn new_funded_wallet(descriptor: &str, change_descriptor: Option<&str>) -> (Wallet, Txid) {
    let params = if let Some(change_desc) = change_descriptor {
        Wallet::create(descriptor.to_string(), change_desc.to_string())
    } else {
        Wallet::create_single(descriptor.to_string())
    };

    let mut wallet = params
        .network(Network::Regtest)
        .create_wallet_no_persist()
        .expect("descriptors must be valid");

    let receive_address = wallet.peek_address(KeychainKind::External, 0).address;
    let sendto_address = Address::from_str("bcrt1q3qtze4ys45tgdvguj66zrk4fu6hq3a3v9pfly5")
        .expect("address")
        .require_network(Network::Regtest)
        .unwrap();

    let tx0 = Transaction {
        output: vec![TxOut {
            value: Amount::from_sat(76_000),
            script_pubkey: receive_address.script_pubkey(),
        }],
        ..new_tx(0)
    };

    let tx1 = Transaction {
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: tx0.compute_txid(),
                vout: 0,
            },
            ..Default::default()
        }],
        output: vec![
            TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: receive_address.script_pubkey(),
            },
            TxOut {
                value: Amount::from_sat(25_000),
                script_pubkey: sendto_address.script_pubkey(),
            },
        ],
        ..new_tx(0)
    };

    insert_checkpoint(
        &mut wallet,
        BlockId {
            height: 42,
            hash: BlockHash::all_zeros(),
        },
    );
    insert_checkpoint(
        &mut wallet,
        BlockId {
            height: 1_000,
            hash: BlockHash::all_zeros(),
        },
    );
    insert_checkpoint(
        &mut wallet,
        BlockId {
            height: 2_000,
            hash: BlockHash::all_zeros(),
        },
    );

    insert_tx(&mut wallet, tx0.clone());
    insert_anchor(
        &mut wallet,
        tx0.compute_txid(),
        ConfirmationBlockTime {
            block_id: BlockId {
                height: 1_000,
                hash: BlockHash::all_zeros(),
            },
            confirmation_time: 100,
        },
    );

    insert_tx(&mut wallet, tx1.clone());
    insert_anchor(
        &mut wallet,
        tx1.compute_txid(),
        ConfirmationBlockTime {
            block_id: BlockId {
                height: 2_000,
                hash: BlockHash::all_zeros(),
            },
            confirmation_time: 200,
        },
    );

    (wallet, tx1.compute_txid())
}

/// Return a fake wallet that appears to be funded for testing.
///
/// The funded wallet contains a tx with a 76_000 sats input and two outputs, one spending 25_000
/// to a foreign address and one returning 50_000 back to the wallet. The remaining 1000
/// sats are the transaction fee.
pub fn get_funded_wallet_single(descriptor: &str) -> (Wallet, Txid) {
    new_funded_wallet(descriptor, None)
}

/// Get funded segwit wallet
pub fn get_funded_wallet_wpkh() -> (Wallet, Txid) {
    let (desc, change_desc) = get_test_wpkh_and_change_desc();
    get_funded_wallet(desc, change_desc)
}

/// `wpkh` single key descriptor
pub fn get_test_wpkh() -> &'static str {
    "wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)"
}

/// `wpkh` xpriv and change descriptor
pub fn get_test_wpkh_and_change_desc() -> (&'static str, &'static str) {
    ("wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)",
    "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/*)")
}

/// `wsh` descriptor with policy `and(pk(A),older(6))`
pub fn get_test_single_sig_csv() -> &'static str {
    "wsh(and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),older(6)))"
}

/// `wsh` descriptor with policy `or(pk(A),and(pk(B),older(144)))`
pub fn get_test_a_or_b_plus_csv() -> &'static str {
    "wsh(or_d(pk(cRjo6jqfVNP33HhSS76UhXETZsGTZYx8FMFvR9kpbtCSV1PmdZdu),and_v(v:pk(cMnkdebixpXMPfkcNEjjGin7s94hiehAH4mLbYkZoh9KSiNNmqC8),older(144))))"
}

/// `wsh` descriptor with policy `and(pk(A),after(100000))`
pub fn get_test_single_sig_cltv() -> &'static str {
    "wsh(and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),after(100000)))"
}

/// `wsh` descriptor with policy `and(pk(A),after(1_734_230_218))`
// the parameter passed to miniscript fragment `after` has to equal or greater than 500_000_000
// in order to use a lock based on unix time
pub fn get_test_single_sig_cltv_timestamp() -> &'static str {
    "wsh(and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),after(1734230218)))"
}

/// taproot single key descriptor
pub fn get_test_tr_single_sig() -> &'static str {
    "tr(cNJmN3fH9DDbDt131fQNkVakkpzawJBSeybCUNmP1BovpmGQ45xG)"
}

/// taproot descriptor with taptree
pub fn get_test_tr_with_taptree() -> &'static str {
    "tr(b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55,{pk(cPZzKuNmpuUjD1e8jUU4PVzy2b5LngbSip8mBsxf4e7rSFZVb4Uh),pk(8aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642)})"
}

/// taproot descriptor with private key taptree
pub fn get_test_tr_with_taptree_both_priv() -> &'static str {
    "tr(b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55,{pk(cPZzKuNmpuUjD1e8jUU4PVzy2b5LngbSip8mBsxf4e7rSFZVb4Uh),pk(cNaQCDwmmh4dS9LzCgVtyy1e1xjCJ21GUDHe9K98nzb689JvinGV)})"
}

/// taproot descriptor where one key appears in two script paths
pub fn get_test_tr_repeated_key() -> &'static str {
    "tr(b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55,{and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),after(100)),and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),after(200))})"
}

/// taproot xpriv descriptor
pub fn get_test_tr_single_sig_xprv() -> &'static str {
    "tr(tprv8ZgxMBicQKsPdDArR4xSAECuVxeX1jwwSXR4ApKbkYgZiziDc4LdBy2WvJeGDfUSE4UT4hHhbgEwbdq8ajjUHiKDegkwrNU6V55CxcxonVN/*)"
}

/// taproot xpriv and change descriptor
pub fn get_test_tr_single_sig_xprv_and_change_desc() -> (&'static str, &'static str) {
    ("tr(tprv8ZgxMBicQKsPdDArR4xSAECuVxeX1jwwSXR4ApKbkYgZiziDc4LdBy2WvJeGDfUSE4UT4hHhbgEwbdq8ajjUHiKDegkwrNU6V55CxcxonVN/0/*)",
    "tr(tprv8ZgxMBicQKsPdDArR4xSAECuVxeX1jwwSXR4ApKbkYgZiziDc4LdBy2WvJeGDfUSE4UT4hHhbgEwbdq8ajjUHiKDegkwrNU6V55CxcxonVN/1/*)")
}

/// taproot descriptor with taptree
pub fn get_test_tr_with_taptree_xprv() -> &'static str {
    "tr(cNJmN3fH9DDbDt131fQNkVakkpzawJBSeybCUNmP1BovpmGQ45xG,{pk(tprv8ZgxMBicQKsPdDArR4xSAECuVxeX1jwwSXR4ApKbkYgZiziDc4LdBy2WvJeGDfUSE4UT4hHhbgEwbdq8ajjUHiKDegkwrNU6V55CxcxonVN/*),pk(8aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642)})"
}

/// taproot descriptor with duplicate script paths
pub fn get_test_tr_dup_keys() -> &'static str {
    "tr(cNJmN3fH9DDbDt131fQNkVakkpzawJBSeybCUNmP1BovpmGQ45xG,{pk(8aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642),pk(8aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642)})"
}

/// A new empty transaction with the given locktime
pub fn new_tx(locktime: u32) -> Transaction {
    Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::from_consensus(locktime),
        input: vec![],
        output: vec![],
    }
}

/// Construct a new [`FeeRate`] from the given raw `sat_vb` feerate. This is
/// useful in cases where we want to create a feerate from a `f64`, as the
/// traditional [`FeeRate::from_sat_per_vb`] method will only accept an integer.
///
/// **Note** this 'quick and dirty' conversion should only be used when the input
/// parameter has units of `satoshis/vbyte` **AND** is not expected to overflow,
/// or else the resulting value will be inaccurate.
pub fn feerate_unchecked(sat_vb: f64) -> FeeRate {
    // 1 sat_vb / 4wu_vb * 1000kwu_wu = 250 sat_kwu
    let sat_kwu = (sat_vb * 250.0).ceil() as u64;
    FeeRate::from_sat_per_kwu(sat_kwu)
}

/// Input parameter for [`receive_output`].
pub enum ReceiveTo {
    /// Receive tx to mempool at this `last_seen` timestamp.
    Mempool(u64),
    /// Receive tx to block with this anchor.
    Block(ConfirmationBlockTime),
}

impl From<ConfirmationBlockTime> for ReceiveTo {
    fn from(value: ConfirmationBlockTime) -> Self {
        Self::Block(value)
    }
}

/// Receive a tx output with the given value in the latest block
pub fn receive_output_in_latest_block(wallet: &mut Wallet, value: u64) -> OutPoint {
    let latest_cp = wallet.latest_checkpoint();
    let height = latest_cp.height();
    assert!(height > 0, "cannot receive tx into genesis block");
    receive_output(
        wallet,
        value,
        ConfirmationBlockTime {
            block_id: latest_cp.block_id(),
            confirmation_time: 0,
        },
    )
}

/// Receive a tx output with the given value and chain position
pub fn receive_output(
    wallet: &mut Wallet,
    value: u64,
    receive_to: impl Into<ReceiveTo>,
) -> OutPoint {
    let addr = wallet.next_unused_address(KeychainKind::External).address;
    receive_output_to_address(wallet, addr, value, receive_to)
}

/// Receive a tx output to an address with the given value and chain position
pub fn receive_output_to_address(
    wallet: &mut Wallet,
    addr: Address,
    value: u64,
    receive_to: impl Into<ReceiveTo>,
) -> OutPoint {
    let tx = Transaction {
        version: transaction::Version::ONE,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            script_pubkey: addr.script_pubkey(),
            value: Amount::from_sat(value),
        }],
    };

    let txid = tx.compute_txid();
    insert_tx(wallet, tx);

    match receive_to.into() {
        ReceiveTo::Block(anchor) => insert_anchor(wallet, txid, anchor),
        ReceiveTo::Mempool(last_seen) => insert_seen_at(wallet, txid, last_seen),
    }

    OutPoint { txid, vout: 0 }
}

/// Insert a checkpoint into the wallet. This can be used to extend the wallet's local chain
/// or to insert a block that did not exist previously. Note that if replacing a block with
/// a different one at the same height, then all later blocks are evicted as well.
pub fn insert_checkpoint(wallet: &mut Wallet, block: BlockId) {
    let mut cp = wallet.latest_checkpoint();
    cp = cp.insert(block);
    wallet
        .apply_update(Update {
            chain: Some(cp),
            ..Default::default()
        })
        .unwrap();
}

/// Insert transaction
pub fn insert_tx(wallet: &mut Wallet, tx: Transaction) {
    let mut tx_update = bdk_chain::TxUpdate::default();
    tx_update.txs.push(Arc::new(tx));
    wallet
        .apply_update(Update {
            tx_update,
            ..Default::default()
        })
        .unwrap();
}

/// Simulates confirming a tx with `txid` by applying an update to the wallet containing
/// the given `anchor`. Note: to be considered confirmed the anchor block must exist in
/// the current active chain.
pub fn insert_anchor(wallet: &mut Wallet, txid: Txid, anchor: ConfirmationBlockTime) {
    let mut tx_update = bdk_chain::TxUpdate::default();
    tx_update.anchors.insert((anchor, txid));
    wallet
        .apply_update(Update {
            tx_update,
            ..Default::default()
        })
        .unwrap();
}

/// Marks the given `txid` seen as unconfirmed at `seen_at`
pub fn insert_seen_at(wallet: &mut Wallet, txid: Txid, seen_at: u64) {
    let mut tx_update = bdk_chain::TxUpdate::default();
    tx_update.seen_ats.insert(txid, seen_at);
    wallet
        .apply_update(crate::Update {
            tx_update,
            ..Default::default()
        })
        .unwrap();
}
