#![allow(unused)]
use bdk_chain::{BlockId, BlockTime, ConfirmationTime, TxGraph};
use bdk_wallet::{
    wallet::{Update, Wallet},
    KeychainKind, LocalOutput,
};
use bitcoin::{
    hashes::Hash, transaction, Address, Amount, BlockHash, FeeRate, Network, OutPoint, Transaction,
    TxIn, TxOut, Txid,
};
use std::str::FromStr;

/// Return a fake wallet that appears to be funded for testing.
///
/// The funded wallet contains a tx with a 76_000 sats input and two outputs, one spending 25_000
/// to a foreign address and one returning 50_000 back to the wallet. The remaining 1000
/// sats are the transaction fee.
pub fn get_funded_wallet_with_change(descriptor: &str, change: &str) -> (Wallet, bitcoin::Txid) {
    let mut wallet = Wallet::new(descriptor, change, Network::Regtest).unwrap();
    let receive_address = wallet.peek_address(KeychainKind::External, 0).address;
    let sendto_address = Address::from_str("bcrt1q3qtze4ys45tgdvguj66zrk4fu6hq3a3v9pfly5")
        .expect("address")
        .require_network(Network::Regtest)
        .unwrap();

    let tx0 = Transaction {
        version: transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: Txid::all_zeros(),
                vout: 0,
            },
            script_sig: Default::default(),
            sequence: Default::default(),
            witness: Default::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(76_000),
            script_pubkey: receive_address.script_pubkey(),
        }],
    };

    let tx1 = Transaction {
        version: transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: tx0.compute_txid(),
                vout: 0,
            },
            script_sig: Default::default(),
            sequence: Default::default(),
            witness: Default::default(),
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
    };

    wallet
        .insert_checkpoint(BlockId {
            height: 42,
            hash: BlockHash::all_zeros(),
        })
        .unwrap();
    wallet
        .insert_checkpoint(BlockId {
            height: 1_000,
            hash: BlockHash::all_zeros(),
        })
        .unwrap();
    wallet
        .insert_checkpoint(BlockId {
            height: 2_000,
            hash: BlockHash::all_zeros(),
        })
        .unwrap();

    wallet.insert_tx(tx0.clone());
    insert_anchor_from_conf(
        &mut wallet,
        tx0.compute_txid(),
        ConfirmationTime::Confirmed {
            height: 1_000,
            time: 100,
        },
    );

    wallet.insert_tx(tx1.clone());
    insert_anchor_from_conf(
        &mut wallet,
        tx1.compute_txid(),
        ConfirmationTime::Confirmed {
            height: 2_000,
            time: 200,
        },
    );

    (wallet, tx1.compute_txid())
}

/// Return a fake wallet that appears to be funded for testing.
///
/// The funded wallet contains a tx with a 76_000 sats input and two outputs, one spending 25_000
/// to a foreign address and one returning 50_000 back to the wallet. The remaining 1000
/// sats are the transaction fee.
///
/// Note: the change descriptor will have script type `p2wpkh`. If passing some other script type
/// as argument, make sure you're ok with getting a wallet where the keychains have potentially
/// different script types. Otherwise, use `get_funded_wallet_with_change`.
pub fn get_funded_wallet(descriptor: &str) -> (Wallet, bitcoin::Txid) {
    let change = get_test_wpkh_change();
    get_funded_wallet_with_change(descriptor, change)
}

pub fn get_funded_wallet_wpkh() -> (Wallet, bitcoin::Txid) {
    get_funded_wallet_with_change(get_test_wpkh(), get_test_wpkh_change())
}

pub fn get_test_wpkh() -> &'static str {
    "wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)"
}

pub fn get_test_wpkh_with_change_desc() -> (&'static str, &'static str) {
    (
        "wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)",
        get_test_wpkh_change(),
    )
}

fn get_test_wpkh_change() -> &'static str {
    "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/0)"
}

pub fn get_test_single_sig_csv() -> &'static str {
    // and(pk(Alice),older(6))
    "wsh(and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),older(6)))"
}

pub fn get_test_a_or_b_plus_csv() -> &'static str {
    // or(pk(Alice),and(pk(Bob),older(144)))
    "wsh(or_d(pk(cRjo6jqfVNP33HhSS76UhXETZsGTZYx8FMFvR9kpbtCSV1PmdZdu),and_v(v:pk(cMnkdebixpXMPfkcNEjjGin7s94hiehAH4mLbYkZoh9KSiNNmqC8),older(144))))"
}

pub fn get_test_single_sig_cltv() -> &'static str {
    // and(pk(Alice),after(100000))
    "wsh(and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),after(100000)))"
}

pub fn get_test_tr_single_sig() -> &'static str {
    "tr(cNJmN3fH9DDbDt131fQNkVakkpzawJBSeybCUNmP1BovpmGQ45xG)"
}

pub fn get_test_tr_with_taptree() -> &'static str {
    "tr(b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55,{pk(cPZzKuNmpuUjD1e8jUU4PVzy2b5LngbSip8mBsxf4e7rSFZVb4Uh),pk(8aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642)})"
}

pub fn get_test_tr_with_taptree_both_priv() -> &'static str {
    "tr(b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55,{pk(cPZzKuNmpuUjD1e8jUU4PVzy2b5LngbSip8mBsxf4e7rSFZVb4Uh),pk(cNaQCDwmmh4dS9LzCgVtyy1e1xjCJ21GUDHe9K98nzb689JvinGV)})"
}

pub fn get_test_tr_repeated_key() -> &'static str {
    "tr(b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55,{and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),after(100)),and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),after(200))})"
}

pub fn get_test_tr_single_sig_xprv() -> &'static str {
    "tr(tprv8ZgxMBicQKsPdDArR4xSAECuVxeX1jwwSXR4ApKbkYgZiziDc4LdBy2WvJeGDfUSE4UT4hHhbgEwbdq8ajjUHiKDegkwrNU6V55CxcxonVN/*)"
}

pub fn get_test_tr_single_sig_xprv_with_change_desc() -> (&'static str, &'static str) {
    ("tr(tprv8ZgxMBicQKsPdDArR4xSAECuVxeX1jwwSXR4ApKbkYgZiziDc4LdBy2WvJeGDfUSE4UT4hHhbgEwbdq8ajjUHiKDegkwrNU6V55CxcxonVN/0/*)",
    "tr(tprv8ZgxMBicQKsPdDArR4xSAECuVxeX1jwwSXR4ApKbkYgZiziDc4LdBy2WvJeGDfUSE4UT4hHhbgEwbdq8ajjUHiKDegkwrNU6V55CxcxonVN/1/*)")
}

pub fn get_test_tr_with_taptree_xprv() -> &'static str {
    "tr(cNJmN3fH9DDbDt131fQNkVakkpzawJBSeybCUNmP1BovpmGQ45xG,{pk(tprv8ZgxMBicQKsPdDArR4xSAECuVxeX1jwwSXR4ApKbkYgZiziDc4LdBy2WvJeGDfUSE4UT4hHhbgEwbdq8ajjUHiKDegkwrNU6V55CxcxonVN/*),pk(8aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642)})"
}

pub fn get_test_tr_dup_keys() -> &'static str {
    "tr(cNJmN3fH9DDbDt131fQNkVakkpzawJBSeybCUNmP1BovpmGQ45xG,{pk(8aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642),pk(8aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642)})"
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

/// Simulates confirming a tx with `txid` at the specified `position` by inserting an anchor
/// at the lowest height in local chain that is greater or equal to `position`'s height,
/// assuming the confirmation time matches `ConfirmationTime::Confirmed`.
pub fn insert_anchor_from_conf(wallet: &mut Wallet, txid: Txid, position: ConfirmationTime) {
    if let ConfirmationTime::Confirmed { height, time } = position {
        // anchor tx to checkpoint with lowest height that is >= position's height
        let (anchor, anchor_meta) = wallet
            .local_chain()
            .range(height..)
            .last()
            .map(|anchor_cp| ((txid, anchor_cp.block_id()), BlockTime::new(time as u32)))
            .expect("confirmation height cannot be greater than tip");

        let mut graph = TxGraph::default();
        let _ = graph.insert_anchor(anchor, anchor_meta);
        wallet
            .apply_update(Update {
                graph,
                ..Default::default()
            })
            .unwrap();
    }
}
