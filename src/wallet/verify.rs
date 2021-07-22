// Bitcoin Dev Kit
// Written in 2021 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

//! Verify transactions against the consensus rules

use std::collections::HashMap;
use std::fmt;

use bitcoin::consensus::serialize;
use bitcoin::{OutPoint, Transaction, Txid};

use crate::blockchain::Blockchain;
use crate::database::Database;
use crate::error::Error;

/// Verify a transaction against the consensus rules
///
/// This function uses [`bitcoinconsensus`] to verify transactions by fetching the required data
/// either from the [`Database`] or using the [`Blockchain`].
///
/// Depending on the [capabilities](crate::blockchain::Blockchain::get_capabilities) of the
/// [`Blockchain`] backend, the method could fail when called with old "historical" transactions or
/// with unconfirmed transactions that have been evicted from the backend's memory.
pub fn verify_tx<D: Database, B: Blockchain>(
    tx: &Transaction,
    database: &D,
    blockchain: &B,
) -> Result<(), VerifyError> {
    log::debug!("Verifying {}", tx.txid());

    let serialized_tx = serialize(tx);
    let mut tx_cache = HashMap::<_, Transaction>::new();

    for (index, input) in tx.input.iter().enumerate() {
        let prev_tx = if let Some(prev_tx) = tx_cache.get(&input.previous_output.txid) {
            prev_tx.clone()
        } else if let Some(prev_tx) = database.get_raw_tx(&input.previous_output.txid)? {
            prev_tx
        } else if let Some(prev_tx) = blockchain.get_tx(&input.previous_output.txid)? {
            prev_tx
        } else {
            return Err(VerifyError::MissingInputTx(input.previous_output.txid));
        };

        let spent_output = prev_tx
            .output
            .get(input.previous_output.vout as usize)
            .ok_or(VerifyError::InvalidInput(input.previous_output))?;

        bitcoinconsensus::verify(
            &spent_output.script_pubkey.to_bytes(),
            spent_output.value,
            &serialized_tx,
            index,
        )?;

        // Since we have a local cache we might as well cache stuff from the db, as it will very
        // likely decrease latency compared to reading from disk or performing an SQL query.
        tx_cache.insert(prev_tx.txid(), prev_tx);
    }

    Ok(())
}

/// Error during validation of a tx agains the consensus rules
#[derive(Debug)]
pub enum VerifyError {
    /// The transaction being spent is not available in the database or the blockchain client
    MissingInputTx(Txid),
    /// The transaction being spent doesn't have the requested output
    InvalidInput(OutPoint),

    /// Consensus error
    Consensus(bitcoinconsensus::Error),

    /// Generic error
    ///
    /// It has to be wrapped in a `Box` since `Error` has a variant that contains this enum
    Global(Box<Error>),
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for VerifyError {}

impl From<Error> for VerifyError {
    fn from(other: Error) -> Self {
        VerifyError::Global(Box::new(other))
    }
}
impl_error!(bitcoinconsensus::Error, Consensus, VerifyError);

#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use bitcoin::consensus::encode::deserialize;
    use bitcoin::hashes::hex::FromHex;
    use bitcoin::{Transaction, Txid};

    use crate::blockchain::{Blockchain, Capability, Progress};
    use crate::database::{BatchDatabase, BatchOperations, MemoryDatabase};
    use crate::FeeRate;

    use super::*;

    struct DummyBlockchain;

    impl Blockchain for DummyBlockchain {
        fn get_capabilities(&self) -> HashSet<Capability> {
            Default::default()
        }
        fn setup<D: BatchDatabase, P: 'static + Progress>(
            &self,
            _database: &mut D,
            _progress_update: P,
        ) -> Result<(), Error> {
            Ok(())
        }
        fn get_tx(&self, _txid: &Txid) -> Result<Option<Transaction>, Error> {
            Ok(None)
        }
        fn broadcast(&self, _tx: &Transaction) -> Result<(), Error> {
            Ok(())
        }
        fn get_height(&self) -> Result<u32, Error> {
            Ok(42)
        }
        fn estimate_fee(&self, _target: usize) -> Result<FeeRate, Error> {
            Ok(FeeRate::default_min_relay_fee())
        }
    }

    #[test]
    fn test_verify_fail_unsigned_tx() {
        // https://blockstream.info/tx/95da344585fcf2e5f7d6cbf2c3df2dcce84f9196f7a7bb901a43275cd6eb7c3f
        let prev_tx: Transaction = deserialize(&Vec::<u8>::from_hex("020000000101192dea5e66d444380e106f8e53acb171703f00d43fb6b3ae88ca5644bdb7e1000000006b48304502210098328d026ce138411f957966c1cf7f7597ccbb170f5d5655ee3e9f47b18f6999022017c3526fc9147830e1340e04934476a3d1521af5b4de4e98baf49ec4c072079e01210276f847f77ec8dd66d78affd3c318a0ed26d89dab33fa143333c207402fcec352feffffff023d0ac203000000001976a9144bfbaf6afb76cc5771bc6404810d1cc041a6933988aca4b956050000000017a91494d5543c74a3ee98e0cf8e8caef5dc813a0f34b48768cb0700").unwrap()).unwrap();
        // https://blockstream.info/tx/aca326a724eda9a461c10a876534ecd5ae7b27f10f26c3862fb996f80ea2d45d
        let signed_tx: Transaction = deserialize(&Vec::<u8>::from_hex("02000000013f7cebd65c27431a90bba7f796914fe8cc2ddfc3f2cbd6f7e5f2fc854534da95000000006b483045022100de1ac3bcdfb0332207c4a91f3832bd2c2915840165f876ab47c5f8996b971c3602201c6c053d750fadde599e6f5c4e1963df0f01fc0d97815e8157e3d59fe09ca30d012103699b464d1d8bc9e47d4fb1cdaa89a1c5783d68363c4dbc4b524ed3d857148617feffffff02836d3c01000000001976a914fc25d6d5c94003bf5b0c7b640a248e2c637fcfb088ac7ada8202000000001976a914fbed3d9b11183209a57999d54d59f67c019e756c88ac6acb0700").unwrap()).unwrap();

        let mut database = MemoryDatabase::new();
        let blockchain = DummyBlockchain;

        let mut unsigned_tx = signed_tx.clone();
        for input in &mut unsigned_tx.input {
            input.script_sig = Default::default();
            input.witness = Default::default();
        }

        let result = verify_tx(&signed_tx, &database, &blockchain);
        assert!(result.is_err(), "Should fail with missing input tx");
        assert!(
            matches!(result, Err(VerifyError::MissingInputTx(txid)) if txid == prev_tx.txid()),
            "Error should be a `MissingInputTx` error"
        );

        // insert the prev_tx
        database.set_raw_tx(&prev_tx).unwrap();

        let result = verify_tx(&unsigned_tx, &database, &blockchain);
        assert!(result.is_err(), "Should fail since the TX is unsigned");
        assert!(
            matches!(result, Err(VerifyError::Consensus(_))),
            "Error should be a `Consensus` error"
        );

        let result = verify_tx(&signed_tx, &database, &blockchain);
        assert!(
            result.is_ok(),
            "Should work since the TX is correctly signed"
        );
    }
}
