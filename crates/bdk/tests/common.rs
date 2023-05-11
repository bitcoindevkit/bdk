#![allow(unused)]
use bdk::{wallet::AddressIndex, Wallet};
use bdk_chain::{BlockId, ConfirmationTime};
use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, Network, Transaction, TxOut};

/// Return a fake wallet that appears to be funded for testing.
pub fn get_funded_wallet_with_change(
    descriptor: &str,
    change: Option<&str>,
) -> (Wallet, bitcoin::Txid) {
    let mut wallet = Wallet::new_no_persist(descriptor, change, Network::Regtest).unwrap();
    let address = wallet.get_address(AddressIndex::New).address;

    let tx = Transaction {
        version: 1,
        lock_time: bitcoin::PackedLockTime(0),
        input: vec![],
        output: vec![TxOut {
            value: 50_000,
            script_pubkey: address.script_pubkey(),
        }],
    };

    wallet
        .insert_checkpoint(BlockId {
            height: 1_000,
            hash: BlockHash::all_zeros(),
        })
        .unwrap();
    wallet
        .insert_tx(
            tx.clone(),
            ConfirmationTime::Confirmed {
                height: 1_000,
                time: 100,
            },
        )
        .unwrap();

    (wallet, tx.txid())
}

pub fn get_funded_wallet(descriptor: &str) -> (Wallet, bitcoin::Txid) {
    get_funded_wallet_with_change(descriptor, None)
}

pub fn get_test_wpkh() -> &'static str {
    "wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)"
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

pub fn get_test_tr_with_taptree_xprv() -> &'static str {
    "tr(cNJmN3fH9DDbDt131fQNkVakkpzawJBSeybCUNmP1BovpmGQ45xG,{pk(tprv8ZgxMBicQKsPdDArR4xSAECuVxeX1jwwSXR4ApKbkYgZiziDc4LdBy2WvJeGDfUSE4UT4hHhbgEwbdq8ajjUHiKDegkwrNU6V55CxcxonVN/*),pk(8aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642)})"
}

pub fn get_test_tr_dup_keys() -> &'static str {
    "tr(cNJmN3fH9DDbDt131fQNkVakkpzawJBSeybCUNmP1BovpmGQ45xG,{pk(8aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642),pk(8aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642)})"
}
