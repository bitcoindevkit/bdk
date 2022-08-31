// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.
#![allow(missing_docs)]
#![allow(unused)]

use std::str::FromStr;

use bitcoin::{Address, Network, OutPoint, Transaction, TxIn, TxOut, Txid};

use crate::{
    database::{AnyDatabase, BatchDatabase, BatchOperations, Database, MemoryDatabase, SyncTime},
    testutils, BlockTime, KeychainKind, LocalUtxo, TransactionDetails, Wallet,
};

use super::TestIncomingTx;

#[doc(hidden)]
/// Populate a test database with a `TestIncomingTx`, as if we had found the tx with a `sync`.
/// This is a hidden function, only useful for `Database` unit testing.
/// Setting current_height to None, creates an unsynced database.
pub fn populate_test_db(
    db: &mut impl Database,
    tx_meta: TestIncomingTx,
    current_height: Option<u32>,
    is_coinbase: bool,
) -> Txid {
    let mut input = vec![bitcoin::TxIn::default()];
    if !is_coinbase {
        input[0].previous_output.vout = 0;
    }
    let tx = bitcoin::Transaction {
        version: 1,
        lock_time: 0,
        input,
        output: tx_meta
            .output
            .iter()
            .map(|out_meta| bitcoin::TxOut {
                value: out_meta.value,
                script_pubkey: bitcoin::Address::from_str(&out_meta.to_address)
                    .unwrap()
                    .script_pubkey(),
            })
            .collect(),
    };

    let txid = tx.txid();
    // Set Confirmation time only if current height is provided.
    // panics if `tx_meta.min_confirmation` is Some, and current_height is None.
    let confirmation_time = tx_meta
        .min_confirmations
        .and_then(|v| if v == 0 { None } else { Some(v) })
        .map(|conf| BlockTime {
            height: current_height
                .expect(
                    "Current height is needed for testing transaction with min-confirmation values",
                )
                .checked_sub(conf as u32)
                .unwrap()
                + 1,
            timestamp: 0,
        });

    // Set the database sync_time.
    // Check if the current_height is less than already known sync height, apply the max
    // If any of them is None, the other will be applied instead.
    // If both are None, this will not be set.
    if let Some(height) = db
        .get_sync_time()
        .unwrap()
        .map(|sync_time| sync_time.block_time.height)
        .max(current_height)
    {
        let sync_time = SyncTime {
            block_time: BlockTime {
                height,
                timestamp: 0,
            },
        };
        db.set_sync_time(sync_time).unwrap();
    }

    let tx_details = TransactionDetails {
        transaction: Some(tx.clone()),
        txid,
        fee: Some(0),
        received: 0,
        sent: 0,
        confirmation_time,
    };

    db.set_tx(&tx_details).unwrap();
    for (vout, out) in tx.output.iter().enumerate() {
        db.set_utxo(&LocalUtxo {
            txout: out.clone(),
            outpoint: bitcoin::OutPoint {
                txid,
                vout: vout as u32,
            },
            keychain: KeychainKind::External,
            is_spent: false,
        })
        .unwrap();
    }
    txid
}

#[doc(hidden)]
#[cfg(test)]
/// Return a fake wallet that appears to be funded for testing.
pub(crate) fn get_funded_wallet(
    descriptor: &str,
) -> (Wallet<AnyDatabase>, (String, Option<String>), bitcoin::Txid) {
    let descriptors = testutils!(@descriptors (descriptor));
    let wallet = Wallet::new(
        &descriptors.0,
        None,
        Network::Regtest,
        AnyDatabase::Memory(MemoryDatabase::new()),
    )
    .unwrap();

    let funding_address_kix = 0;

    let tx_meta = testutils! {
            @tx ( (@external descriptors, funding_address_kix) => 50_000 ) (@confirmations 1)
    };

    wallet
        .database_mut()
        .set_script_pubkey(
            &bitcoin::Address::from_str(&tx_meta.output.get(0).unwrap().to_address)
                .unwrap()
                .script_pubkey(),
            KeychainKind::External,
            funding_address_kix,
        )
        .unwrap();
    wallet
        .database_mut()
        .set_last_index(KeychainKind::External, funding_address_kix)
        .unwrap();

    let txid = populate_test_db(&mut *wallet.database_mut(), tx_meta, Some(100), false);

    (wallet, descriptors, txid)
}

#[macro_export]
#[doc(hidden)]
macro_rules! run_tests_with_init {
(@init $fn_name:ident(), @tests ( $($x:tt) , + $(,)? )) => {
    $(
        #[test]
        fn $x()
        {
        $crate::database::test::$x($fn_name());
        }
    )+
    };
}

#[macro_export]
#[doc(hidden)]
/// Macro for getting a wallet for use in a doctest
macro_rules! doctest_wallet {
    () => {{
        use $crate::testutils::helpers::populate_test_db;
        use $crate::bitcoin::Network;
        use $crate::database::MemoryDatabase;
        use $crate::testutils;
        let descriptor = "wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)";
        let descriptors = testutils!(@descriptors (descriptor) (descriptor));

        let mut db = MemoryDatabase::new();
        populate_test_db(
            &mut db,
            testutils! {
                @tx ( (@external descriptors, 0) => 500_000 ) (@confirmations 1)
            },
            Some(100),
            false
        );

        $crate::Wallet::new(
            &descriptors.0,
            descriptors.1.as_ref(),
            Network::Regtest,
            db
        )
        .unwrap()
    }}
}
