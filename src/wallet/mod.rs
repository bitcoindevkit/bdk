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

//! Wallet
//!
//! This module defines the [`Wallet`] structure.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use bitcoin::secp256k1::Secp256k1;

use bitcoin::consensus::encode::serialize;
use bitcoin::util::psbt;
use bitcoin::{
    Address, EcdsaSighashType, LockTime, Network, OutPoint, SchnorrSighashType, Script, Sequence,
    Transaction, TxOut, Txid, Witness,
};

use miniscript::psbt::{PsbtExt, PsbtInputExt, PsbtInputSatisfier};

#[allow(unused_imports)]
use log::{debug, error, info, trace};

pub mod coin_selection;
pub mod export;
pub mod signer;
pub mod time;
pub mod tx_builder;
pub(crate) mod utils;
#[cfg(feature = "verify")]
#[cfg_attr(docsrs, doc(cfg(feature = "verify")))]
pub mod verify;

#[cfg(feature = "hardware-signer")]
#[cfg_attr(docsrs, doc(cfg(feature = "hardware-signer")))]
pub mod hardwaresigner;

pub use utils::IsDust;

use coin_selection::DefaultCoinSelectionAlgorithm;
use signer::{SignOptions, SignerOrdering, SignersContainer, TransactionSigner};
use tx_builder::{BumpFee, CreateTx, FeePolicy, TxBuilder, TxParams};
use utils::{check_nsequence_rbf, After, Older, SecpCtx};

use crate::blockchain::{GetHeight, NoopProgress, Progress, WalletSync};
use crate::database::memory::MemoryDatabase;
use crate::database::{AnyDatabase, BatchDatabase, BatchOperations, DatabaseUtils, SyncTime};
use crate::descriptor::checksum::calc_checksum_bytes_internal;
use crate::descriptor::policy::BuildSatisfaction;
use crate::descriptor::{
    calc_checksum, into_wallet_descriptor_checked, DerivedDescriptor, DescriptorMeta,
    ExtendedDescriptor, ExtractPolicy, IntoWalletDescriptor, Policy, XKeyUtils,
};
use crate::error::{Error, MiniscriptPsbtError};
use crate::psbt::PsbtUtils;
use crate::signer::SignerError;
use crate::testutils;
use crate::types::*;
use crate::wallet::coin_selection::Excess::{Change, NoChange};

const CACHE_ADDR_BATCH_SIZE: u32 = 100;
const COINBASE_MATURITY: u32 = 100;

/// A Bitcoin wallet
///
/// The `Wallet` struct acts as a way of coherently interfacing with output descriptors and related transactions.
/// Its main components are:
///
/// 1. output *descriptors* from which it can derive addresses.
/// 2. A [`Database`] where it tracks transactions and utxos related to the descriptors.
/// 3. [`signer`]s that can contribute signatures to addresses instantiated from the descriptors.
///
/// [`Database`]: crate::database::Database
/// [`signer`]: crate::signer
#[derive(Debug)]
pub struct Wallet<D> {
    descriptor: ExtendedDescriptor,
    change_descriptor: Option<ExtendedDescriptor>,

    signers: Arc<SignersContainer>,
    change_signers: Arc<SignersContainer>,

    network: Network,

    database: RefCell<D>,

    secp: SecpCtx,
}

/// The address index selection strategy to use to derived an address from the wallet's external
/// descriptor. See [`Wallet::get_address`]. If you're unsure which one to use use `WalletIndex::New`.
#[derive(Debug)]
pub enum AddressIndex {
    /// Return a new address after incrementing the current descriptor index.
    New,
    /// Return the address for the current descriptor index if it has not been used in a received
    /// transaction. Otherwise return a new address as with [`AddressIndex::New`].
    ///
    /// Use with caution, if the wallet has not yet detected an address has been used it could
    /// return an already used address. This function is primarily meant for situations where the
    /// caller is untrusted; for example when deriving donation addresses on-demand for a public
    /// web page.
    LastUnused,
    /// Return the address for a specific descriptor index. Does not change the current descriptor
    /// index used by `AddressIndex::New` and `AddressIndex::LastUsed`.
    ///
    /// Use with caution, if an index is given that is less than the current descriptor index
    /// then the returned address may have already been used.
    Peek(u32),
    /// Return the address for a specific descriptor index and reset the current descriptor index
    /// used by `AddressIndex::New` and `AddressIndex::LastUsed` to this value.
    ///
    /// Use with caution, if an index is given that is less than the current descriptor index
    /// then the returned address and subsequent addresses returned by calls to `AddressIndex::New`
    /// and `AddressIndex::LastUsed` may have already been used. Also if the index is reset to a
    /// value earlier than the [`crate::blockchain::Blockchain`] stop_gap (default is 20) then a
    /// larger stop_gap should be used to monitor for all possibly used addresses.
    Reset(u32),
}

/// A derived address and the index it was found at.
/// For convenience this automatically derefs to `Address`
#[derive(Debug, PartialEq, Eq)]
pub struct AddressInfo {
    /// Child index of this address
    pub index: u32,
    /// Address
    pub address: Address,
    /// Type of keychain
    pub keychain: KeychainKind,
}

impl Deref for AddressInfo {
    type Target = Address;

    fn deref(&self) -> &Self::Target {
        &self.address
    }
}

impl fmt::Display for AddressInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.address)
    }
}

#[derive(Debug, Default)]
/// Options to a [`sync`].
///
/// [`sync`]: Wallet::sync
pub struct SyncOptions {
    /// The progress tracker which may be informed when progress is made.
    pub progress: Option<Box<dyn Progress>>,
}

impl<D> Wallet<D>
where
    D: BatchDatabase,
{
    #[deprecated = "Just use Wallet::new -- all wallets are offline now!"]
    /// Create a new "offline" wallet
    pub fn new_offline<E: IntoWalletDescriptor>(
        descriptor: E,
        change_descriptor: Option<E>,
        network: Network,
        database: D,
    ) -> Result<Self, Error> {
        Self::new(descriptor, change_descriptor, network, database)
    }

    /// Create a wallet.
    ///
    /// The only way this can fail is if the descriptors passed in do not match the checksums in `database`.
    pub fn new<E: IntoWalletDescriptor>(
        descriptor: E,
        change_descriptor: Option<E>,
        network: Network,
        mut database: D,
    ) -> Result<Self, Error> {
        let secp = Secp256k1::new();

        let (descriptor, keymap) = into_wallet_descriptor_checked(descriptor, &secp, network)?;
        Self::db_checksum(
            &mut database,
            &descriptor.to_string(),
            KeychainKind::External,
        )?;
        let signers = Arc::new(SignersContainer::build(keymap, &descriptor, &secp));
        let (change_descriptor, change_signers) = match change_descriptor {
            Some(desc) => {
                let (change_descriptor, change_keymap) =
                    into_wallet_descriptor_checked(desc, &secp, network)?;
                Self::db_checksum(
                    &mut database,
                    &change_descriptor.to_string(),
                    KeychainKind::Internal,
                )?;

                let change_signers = Arc::new(SignersContainer::build(
                    change_keymap,
                    &change_descriptor,
                    &secp,
                ));

                (Some(change_descriptor), change_signers)
            }
            None => (None, Arc::new(SignersContainer::new())),
        };

        Ok(Wallet {
            descriptor,
            change_descriptor,
            signers,
            change_signers,
            network,
            database: RefCell::new(database),
            secp,
        })
    }

    /// This checks the checksum within [`BatchDatabase`] twice (if needed). The first time with the
    /// actual checksum, and the second time with the checksum of `descriptor+checksum`. The second
    /// check is necessary for backwards compatibility of a checksum-inception bug.
    fn db_checksum(db: &mut D, desc: &str, kind: KeychainKind) -> Result<(), Error> {
        let checksum = calc_checksum_bytes_internal(desc, true)?;
        if db.check_descriptor_checksum(kind, checksum).is_ok() {
            return Ok(());
        }

        let checksum_inception = calc_checksum_bytes_internal(desc, false)?;
        db.check_descriptor_checksum(kind, checksum_inception)
    }

    /// Get the Bitcoin network the wallet is using.
    pub fn network(&self) -> Network {
        self.network
    }

    // Return a newly derived address for the specified `keychain`.
    fn get_new_address(&self, keychain: KeychainKind) -> Result<AddressInfo, Error> {
        let incremented_index = self.fetch_and_increment_index(keychain)?;

        let address_result = self
            .get_descriptor_for_keychain(keychain)
            .at_derivation_index(incremented_index)
            .address(self.network);

        address_result
            .map(|address| AddressInfo {
                address,
                index: incremented_index,
                keychain,
            })
            .map_err(|_| Error::ScriptDoesntHaveAddressForm)
    }

    // Return the the last previously derived address for `keychain` if it has not been used in a
    // received transaction. Otherwise return a new address using [`Wallet::get_new_address`].
    fn get_unused_address(&self, keychain: KeychainKind) -> Result<AddressInfo, Error> {
        let current_index = self.fetch_index(keychain)?;

        let derived_key = self
            .get_descriptor_for_keychain(keychain)
            .at_derivation_index(current_index);

        let script_pubkey = derived_key.script_pubkey();

        let found_used = self
            .list_transactions(true)?
            .iter()
            .flat_map(|tx_details| tx_details.transaction.as_ref())
            .flat_map(|tx| tx.output.iter())
            .any(|o| o.script_pubkey == script_pubkey);

        if found_used {
            self.get_new_address(keychain)
        } else {
            derived_key
                .address(self.network)
                .map(|address| AddressInfo {
                    address,
                    index: current_index,
                    keychain,
                })
                .map_err(|_| Error::ScriptDoesntHaveAddressForm)
        }
    }

    // Return derived address for the descriptor of given [`KeychainKind`] at a specific index
    fn peek_address(&self, index: u32, keychain: KeychainKind) -> Result<AddressInfo, Error> {
        self.get_descriptor_for_keychain(keychain)
            .at_derivation_index(index)
            .address(self.network)
            .map(|address| AddressInfo {
                index,
                address,
                keychain,
            })
            .map_err(|_| Error::ScriptDoesntHaveAddressForm)
    }

    // Return derived address for `keychain` at a specific index and reset current
    // address index
    fn reset_address(&self, index: u32, keychain: KeychainKind) -> Result<AddressInfo, Error> {
        self.set_index(keychain, index)?;

        self.get_descriptor_for_keychain(keychain)
            .at_derivation_index(index)
            .address(self.network)
            .map(|address| AddressInfo {
                index,
                address,
                keychain,
            })
            .map_err(|_| Error::ScriptDoesntHaveAddressForm)
    }

    /// Return a derived address using the external descriptor, see [`AddressIndex`] for
    /// available address index selection strategies. If none of the keys in the descriptor are derivable
    /// (i.e. does not end with /*) then the same address will always be returned for any [`AddressIndex`].
    pub fn get_address(&self, address_index: AddressIndex) -> Result<AddressInfo, Error> {
        self._get_address(address_index, KeychainKind::External)
    }

    /// Return a derived address using the internal (change) descriptor.
    ///
    /// If the wallet doesn't have an internal descriptor it will use the external descriptor.
    ///
    /// see [`AddressIndex`] for available address index selection strategies. If none of the keys
    /// in the descriptor are derivable (i.e. does not end with /*) then the same address will always
    /// be returned for any [`AddressIndex`].
    pub fn get_internal_address(&self, address_index: AddressIndex) -> Result<AddressInfo, Error> {
        self._get_address(address_index, KeychainKind::Internal)
    }

    fn _get_address(
        &self,
        address_index: AddressIndex,
        keychain: KeychainKind,
    ) -> Result<AddressInfo, Error> {
        match address_index {
            AddressIndex::New => self.get_new_address(keychain),
            AddressIndex::LastUnused => self.get_unused_address(keychain),
            AddressIndex::Peek(index) => self.peek_address(index, keychain),
            AddressIndex::Reset(index) => self.reset_address(index, keychain),
        }
    }

    /// Ensures that there are at least `max_addresses` addresses cached in the database if the
    /// descriptor is derivable, or 1 address if it is not.
    /// Will return `Ok(true)` if there are new addresses generated (either external or internal),
    /// and `Ok(false)` if all the required addresses are already cached. This function is useful to
    /// explicitly cache addresses in a wallet to do things like check [`Wallet::is_mine`] on
    /// transaction output scripts.
    pub fn ensure_addresses_cached(&self, max_addresses: u32) -> Result<bool, Error> {
        let mut new_addresses_cached = false;
        let max_address = match self.descriptor.has_wildcard() {
            false => 0,
            true => max_addresses,
        };
        debug!("max_address {}", max_address);
        if self
            .database
            .borrow()
            .get_script_pubkey_from_path(KeychainKind::External, max_address.saturating_sub(1))?
            .is_none()
        {
            debug!("caching external addresses");
            new_addresses_cached = true;
            self.cache_addresses(KeychainKind::External, 0, max_address)?;
        }

        if let Some(change_descriptor) = &self.change_descriptor {
            let max_address = match change_descriptor.has_wildcard() {
                false => 0,
                true => max_addresses,
            };

            if self
                .database
                .borrow()
                .get_script_pubkey_from_path(KeychainKind::Internal, max_address.saturating_sub(1))?
                .is_none()
            {
                debug!("caching internal addresses");
                new_addresses_cached = true;
                self.cache_addresses(KeychainKind::Internal, 0, max_address)?;
            }
        }
        Ok(new_addresses_cached)
    }

    /// Return whether or not a `script` is part of this wallet (either internal or external)
    pub fn is_mine(&self, script: &Script) -> Result<bool, Error> {
        self.database.borrow().is_mine(script)
    }

    /// Return the list of unspent outputs of this wallet
    ///
    /// Note that this method only operates on the internal database, which first needs to be
    /// [`Wallet::sync`] manually.
    pub fn list_unspent(&self) -> Result<Vec<LocalUtxo>, Error> {
        Ok(self
            .database
            .borrow()
            .iter_utxos()?
            .into_iter()
            .filter(|l| !l.is_spent)
            .collect())
    }

    /// Returns the `UTXO` owned by this wallet corresponding to `outpoint` if it exists in the
    /// wallet's database.
    pub fn get_utxo(&self, outpoint: OutPoint) -> Result<Option<LocalUtxo>, Error> {
        self.database.borrow().get_utxo(&outpoint)
    }

    /// Return a single transactions made and received by the wallet
    ///
    /// Optionally fill the [`TransactionDetails::transaction`] field with the raw transaction if
    /// `include_raw` is `true`.
    ///
    /// Note that this method only operates on the internal database, which first needs to be
    /// [`Wallet::sync`] manually.
    pub fn get_tx(
        &self,
        txid: &Txid,
        include_raw: bool,
    ) -> Result<Option<TransactionDetails>, Error> {
        self.database.borrow().get_tx(txid, include_raw)
    }

    /// Return an unsorted list of transactions made and received by the wallet
    ///
    /// Optionally fill the [`TransactionDetails::transaction`] field with the raw transaction if
    /// `include_raw` is `true`.
    ///
    /// To sort transactions, the following code can be used:
    /// ```no_run
    /// # let mut tx_list: Vec<bdk::TransactionDetails> = vec![];
    /// tx_list.sort_by(|a, b| {
    ///     b.confirmation_time
    ///         .as_ref()
    ///         .map(|t| t.height)
    ///         .cmp(&a.confirmation_time.as_ref().map(|t| t.height))
    /// });
    /// ```
    ///
    /// Note that this method only operates on the internal database, which first needs to be
    /// [`Wallet::sync`] manually.
    pub fn list_transactions(&self, include_raw: bool) -> Result<Vec<TransactionDetails>, Error> {
        self.database.borrow().iter_txs(include_raw)
    }

    /// Return the balance, separated into available, trusted-pending, untrusted-pending and immature
    /// values.
    ///
    /// Note that this method only operates on the internal database, which first needs to be
    /// [`Wallet::sync`] manually.
    pub fn get_balance(&self) -> Result<Balance, Error> {
        let mut immature = 0;
        let mut trusted_pending = 0;
        let mut untrusted_pending = 0;
        let mut confirmed = 0;
        let utxos = self.list_unspent()?;
        let database = self.database.borrow();
        let last_sync_height = match database
            .get_sync_time()?
            .map(|sync_time| sync_time.block_time.height)
        {
            Some(height) => height,
            // None means database was never synced
            None => return Ok(Balance::default()),
        };
        for u in utxos {
            // Unwrap used since utxo set is created from database
            let tx = database
                .get_tx(&u.outpoint.txid, true)?
                .expect("Transaction not found in database");
            if let Some(tx_conf_time) = &tx.confirmation_time {
                if tx.transaction.expect("No transaction").is_coin_base()
                    && (last_sync_height - tx_conf_time.height) < COINBASE_MATURITY
                {
                    immature += u.txout.value;
                } else {
                    confirmed += u.txout.value;
                }
            } else if u.keychain == KeychainKind::Internal {
                trusted_pending += u.txout.value;
            } else {
                untrusted_pending += u.txout.value;
            }
        }

        Ok(Balance {
            immature,
            trusted_pending,
            untrusted_pending,
            confirmed,
        })
    }

    /// Add an external signer
    ///
    /// See [the `signer` module](signer) for an example.
    pub fn add_signer(
        &mut self,
        keychain: KeychainKind,
        ordering: SignerOrdering,
        signer: Arc<dyn TransactionSigner>,
    ) {
        let signers = match keychain {
            KeychainKind::External => Arc::make_mut(&mut self.signers),
            KeychainKind::Internal => Arc::make_mut(&mut self.change_signers),
        };

        signers.add_external(signer.id(&self.secp), ordering, signer);
    }

    /// Get the signers
    ///
    /// ## Example
    ///
    /// ```
    /// # use bdk::{Wallet, KeychainKind};
    /// # use bdk::bitcoin::Network;
    /// # use bdk::database::MemoryDatabase;
    /// let wallet = Wallet::new("wpkh(tprv8ZgxMBicQKsPe73PBRSmNbTfbcsZnwWhz5eVmhHpi31HW29Z7mc9B4cWGRQzopNUzZUT391DeDJxL2PefNunWyLgqCKRMDkU1s2s8bAfoSk/84'/0'/0'/0/*)", None, Network::Testnet, MemoryDatabase::new())?;
    /// for secret_key in wallet.get_signers(KeychainKind::External).signers().iter().filter_map(|s| s.descriptor_secret_key()) {
    ///     // secret_key: tprv8ZgxMBicQKsPe73PBRSmNbTfbcsZnwWhz5eVmhHpi31HW29Z7mc9B4cWGRQzopNUzZUT391DeDJxL2PefNunWyLgqCKRMDkU1s2s8bAfoSk/84'/0'/0'/0/*
    ///     println!("secret_key: {}", secret_key);
    /// }
    ///
    /// Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn get_signers(&self, keychain: KeychainKind) -> Arc<SignersContainer> {
        match keychain {
            KeychainKind::External => Arc::clone(&self.signers),
            KeychainKind::Internal => Arc::clone(&self.change_signers),
        }
    }

    /// Start building a transaction.
    ///
    /// This returns a blank [`TxBuilder`] from which you can specify the parameters for the transaction.
    ///
    /// ## Example
    ///
    /// ```
    /// # use std::str::FromStr;
    /// # use bitcoin::*;
    /// # use bdk::*;
    /// # use bdk::database::*;
    /// # let descriptor = "wpkh(tpubD6NzVbkrYhZ4Xferm7Pz4VnjdcDPFyjVu5K4iZXQ4pVN8Cks4pHVowTBXBKRhX64pkRyJZJN5xAKj4UDNnLPb5p2sSKXhewoYx5GbTdUFWq/*)";
    /// # let wallet = doctest_wallet!();
    /// # let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap();
    /// let (psbt, details) = {
    ///    let mut builder =  wallet.build_tx();
    ///    builder
    ///        .add_recipient(to_address.script_pubkey(), 50_000);
    ///    builder.finish()?
    /// };
    ///
    /// // sign and broadcast ...
    /// # Ok::<(), bdk::Error>(())
    /// ```
    ///
    /// [`TxBuilder`]: crate::TxBuilder
    pub fn build_tx(&self) -> TxBuilder<'_, D, DefaultCoinSelectionAlgorithm, CreateTx> {
        TxBuilder {
            wallet: self,
            params: TxParams::default(),
            coin_selection: DefaultCoinSelectionAlgorithm::default(),
            phantom: core::marker::PhantomData,
        }
    }

    pub(crate) fn create_tx<Cs: coin_selection::CoinSelectionAlgorithm<D>>(
        &self,
        coin_selection: Cs,
        params: TxParams,
    ) -> Result<(psbt::PartiallySignedTransaction, TransactionDetails), Error> {
        let external_policy = self
            .descriptor
            .extract_policy(&self.signers, BuildSatisfaction::None, &self.secp)?
            .unwrap();
        let internal_policy = self
            .change_descriptor
            .as_ref()
            .map(|desc| {
                Ok::<_, Error>(
                    desc.extract_policy(&self.change_signers, BuildSatisfaction::None, &self.secp)?
                        .unwrap(),
                )
            })
            .transpose()?;

        // The policy allows spending external outputs, but it requires a policy path that hasn't been
        // provided
        if params.change_policy != tx_builder::ChangeSpendPolicy::OnlyChange
            && external_policy.requires_path()
            && params.external_policy_path.is_none()
        {
            return Err(Error::SpendingPolicyRequired(KeychainKind::External));
        };
        // Same for the internal_policy path, if present
        if let Some(internal_policy) = &internal_policy {
            if params.change_policy != tx_builder::ChangeSpendPolicy::ChangeForbidden
                && internal_policy.requires_path()
                && params.internal_policy_path.is_none()
            {
                return Err(Error::SpendingPolicyRequired(KeychainKind::Internal));
            };
        }

        let external_requirements = external_policy.get_condition(
            params
                .external_policy_path
                .as_ref()
                .unwrap_or(&BTreeMap::new()),
        )?;
        let internal_requirements = internal_policy
            .map(|policy| {
                Ok::<_, Error>(
                    policy.get_condition(
                        params
                            .internal_policy_path
                            .as_ref()
                            .unwrap_or(&BTreeMap::new()),
                    )?,
                )
            })
            .transpose()?;

        let requirements =
            external_requirements.merge(&internal_requirements.unwrap_or_default())?;
        debug!("Policy requirements: {:?}", requirements);

        let version = match params.version {
            Some(tx_builder::Version(0)) => {
                return Err(Error::Generic("Invalid version `0`".into()))
            }
            Some(tx_builder::Version(1)) if requirements.csv.is_some() => {
                return Err(Error::Generic(
                    "TxBuilder requested version `1`, but at least `2` is needed to use OP_CSV"
                        .into(),
                ))
            }
            Some(tx_builder::Version(x)) => x,
            None if requirements.csv.is_some() => 2,
            _ => 1,
        };

        // We use a match here instead of a map_or_else as it's way more readable :)
        let current_height = match params.current_height {
            // If they didn't tell us the current height, we assume it's the latest sync height.
            None => self.database().get_sync_time()?.map(|sync_time| {
                LockTime::from_height(sync_time.block_time.height).expect("Invalid height")
            }),
            h => h,
        };

        let lock_time = match params.locktime {
            // When no nLockTime is specified, we try to prevent fee sniping, if possible
            None => {
                // Fee sniping can be partially prevented by setting the timelock
                // to current_height. If we don't know the current_height,
                // we default to 0.
                let fee_sniping_height = current_height.unwrap_or(LockTime::ZERO);

                // We choose the biggest between the required nlocktime and the fee sniping
                // height
                match requirements.timelock {
                    // No requirement, just use the fee_sniping_height
                    None => fee_sniping_height,
                    // There's a block-based requirement, but the value is lower than the fee_sniping_height
                    Some(value @ LockTime::Blocks(_)) if value < fee_sniping_height => fee_sniping_height,
                    // There's a time-based requirement or a block-based requirement greater
                    // than the fee_sniping_height use that value
                    Some(value) => value,
                }
            }
            // Specific nLockTime required and we have no constraints, so just set to that value
            Some(x) if requirements.timelock.is_none() => x,
            // Specific nLockTime required and it's compatible with the constraints
            Some(x) if requirements.timelock.unwrap().is_same_unit(x) && x >= requirements.timelock.unwrap() => x,
            // Invalid nLockTime required
            Some(x) => return Err(Error::Generic(format!("TxBuilder requested timelock of `{:?}`, but at least `{:?}` is required to spend from this script", x, requirements.timelock.unwrap())))
        };

        let n_sequence = match (params.rbf, requirements.csv) {
            // No RBF or CSV but there's an nLockTime, so the nSequence cannot be final
            (None, None) if lock_time != LockTime::ZERO => Sequence::ENABLE_LOCKTIME_NO_RBF,
            // No RBF, CSV or nLockTime, make the transaction final
            (None, None) => Sequence::MAX,

            // No RBF requested, use the value from CSV. Note that this value is by definition
            // non-final, so even if a timelock is enabled this nSequence is fine, hence why we
            // don't bother checking for it here. The same is true for all the other branches below
            (None, Some(csv)) => csv,

            // RBF with a specific value but that value is too high
            (Some(tx_builder::RbfValue::Value(rbf)), _) if !rbf.is_rbf() => {
                return Err(Error::Generic(
                    "Cannot enable RBF with a nSequence >= 0xFFFFFFFE".into(),
                ))
            }
            // RBF with a specific value requested, but the value is incompatible with CSV
            (Some(tx_builder::RbfValue::Value(rbf)), Some(csv))
                if !check_nsequence_rbf(rbf, csv) =>
            {
                return Err(Error::Generic(format!(
                    "Cannot enable RBF with nSequence `{:?}` given a required OP_CSV of `{:?}`",
                    rbf, csv
                )))
            }

            // RBF enabled with the default value with CSV also enabled. CSV takes precedence
            (Some(tx_builder::RbfValue::Default), Some(csv)) => csv,
            // Valid RBF, either default or with a specific value. We ignore the `CSV` value
            // because we've already checked it before
            (Some(rbf), _) => rbf.get_value(),
        };

        let (fee_rate, mut fee_amount) = match params
            .fee_policy
            .as_ref()
            .unwrap_or(&FeePolicy::FeeRate(FeeRate::default()))
        {
            //FIXME: see https://github.com/bitcoindevkit/bdk/issues/256
            FeePolicy::FeeAmount(fee) => {
                if let Some(previous_fee) = params.bumping_fee {
                    if *fee < previous_fee.absolute {
                        return Err(Error::FeeTooLow {
                            required: previous_fee.absolute,
                        });
                    }
                }
                (FeeRate::from_sat_per_vb(0.0), *fee)
            }
            FeePolicy::FeeRate(rate) => {
                if let Some(previous_fee) = params.bumping_fee {
                    let required_feerate = FeeRate::from_sat_per_vb(previous_fee.rate + 1.0);
                    if *rate < required_feerate {
                        return Err(Error::FeeRateTooLow {
                            required: required_feerate,
                        });
                    }
                }
                (*rate, 0)
            }
        };

        let mut tx = Transaction {
            version,
            lock_time: lock_time.into(),
            input: vec![],
            output: vec![],
        };

        if params.manually_selected_only && params.utxos.is_empty() {
            return Err(Error::NoUtxosSelected);
        }

        // we keep it as a float while we accumulate it, and only round it at the end
        let mut outgoing: u64 = 0;
        let mut received: u64 = 0;

        let recipients = params.recipients.iter().map(|(r, v)| (r, *v));

        for (index, (script_pubkey, value)) in recipients.enumerate() {
            if !params.allow_dust
                && value.is_dust(script_pubkey)
                && !script_pubkey.is_provably_unspendable()
            {
                return Err(Error::OutputBelowDustLimit(index));
            }

            if self.is_mine(script_pubkey)? {
                received += value;
            }

            let new_out = TxOut {
                script_pubkey: script_pubkey.clone(),
                value,
            };

            tx.output.push(new_out);

            outgoing += value;
        }

        fee_amount += fee_rate.fee_wu(tx.weight());

        // Segwit transactions' header is 2WU larger than legacy txs' header,
        // as they contain a witness marker (1WU) and a witness flag (1WU) (see BIP144).
        // At this point we really don't know if the resulting transaction will be segwit
        // or legacy, so we just add this 2WU to the fee_amount - overshooting the fee amount
        // is better than undershooting it.
        // If we pass a fee_amount that is slightly higher than the final fee_amount, we
        // end up with a transaction with a slightly higher fee rate than the requested one.
        // If, instead, we undershoot, we may end up with a feerate lower than the requested one
        // - we might come up with non broadcastable txs!
        fee_amount += fee_rate.fee_wu(2);

        if params.change_policy != tx_builder::ChangeSpendPolicy::ChangeAllowed
            && self.change_descriptor.is_none()
        {
            return Err(Error::Generic(
                "The `change_policy` can be set only if the wallet has a change_descriptor".into(),
            ));
        }

        let (required_utxos, optional_utxos) = self.preselect_utxos(
            params.change_policy,
            &params.unspendable,
            params.utxos.clone(),
            params.drain_wallet,
            params.manually_selected_only,
            params.bumping_fee.is_some(), // we mandate confirmed transactions if we're bumping the fee
            current_height.map(LockTime::to_consensus_u32),
        )?;

        // get drain script
        let drain_script = match params.drain_to {
            Some(ref drain_recipient) => drain_recipient.clone(),
            None => self
                .get_internal_address(AddressIndex::New)?
                .address
                .script_pubkey(),
        };

        let coin_selection = coin_selection.coin_select(
            self.database.borrow().deref(),
            required_utxos,
            optional_utxos,
            fee_rate,
            outgoing + fee_amount,
            &drain_script,
        )?;
        fee_amount += coin_selection.fee_amount;
        let excess = &coin_selection.excess;

        tx.input = coin_selection
            .selected
            .iter()
            .map(|u| bitcoin::TxIn {
                previous_output: u.outpoint(),
                script_sig: Script::default(),
                sequence: n_sequence,
                witness: Witness::new(),
            })
            .collect();

        if tx.output.is_empty() {
            // Uh oh, our transaction has no outputs.
            // We allow this when:
            // - We have a drain_to address and the utxos we must spend (this happens,
            // for example, when we RBF)
            // - We have a drain_to address and drain_wallet set
            // Otherwise, we don't know who we should send the funds to, and how much
            // we should send!
            if params.drain_to.is_some() && (params.drain_wallet || !params.utxos.is_empty()) {
                if let NoChange {
                    dust_threshold,
                    remaining_amount,
                    change_fee,
                } = excess
                {
                    return Err(Error::InsufficientFunds {
                        needed: *dust_threshold,
                        available: remaining_amount.saturating_sub(*change_fee),
                    });
                }
            } else {
                return Err(Error::NoRecipients);
            }
        }

        match excess {
            NoChange {
                remaining_amount, ..
            } => fee_amount += remaining_amount,
            Change { amount, fee } => {
                if self.is_mine(&drain_script)? {
                    received += amount;
                }
                fee_amount += fee;

                // create drain output
                let drain_output = TxOut {
                    value: *amount,
                    script_pubkey: drain_script,
                };

                // TODO: We should pay attention when adding a new output: this might increase
                // the lenght of the "number of vouts" parameter by 2 bytes, potentially making
                // our feerate too low
                tx.output.push(drain_output);
            }
        };

        // sort input/outputs according to the chosen algorithm
        params.ordering.sort_tx(&mut tx);

        let txid = tx.txid();
        let sent = coin_selection.local_selected_amount();
        let psbt = self.complete_transaction(tx, coin_selection.selected, params)?;

        let transaction_details = TransactionDetails {
            transaction: None,
            txid,
            confirmation_time: None,
            received,
            sent,
            fee: Some(fee_amount),
        };

        Ok((psbt, transaction_details))
    }

    /// Bump the fee of a transaction previously created with this wallet.
    ///
    /// Returns an error if the transaction is already confirmed or doesn't explicitly signal
    /// *replace by fee* (RBF). If the transaction can be fee bumped then it returns a [`TxBuilder`]
    /// pre-populated with the inputs and outputs of the original transaction.
    ///
    /// ## Example
    ///
    /// ```no_run
    /// # // TODO: remove norun -- bumping fee seems to need the tx in the wallet database first.
    /// # use std::str::FromStr;
    /// # use bitcoin::*;
    /// # use bdk::*;
    /// # use bdk::database::*;
    /// # let descriptor = "wpkh(tpubD6NzVbkrYhZ4Xferm7Pz4VnjdcDPFyjVu5K4iZXQ4pVN8Cks4pHVowTBXBKRhX64pkRyJZJN5xAKj4UDNnLPb5p2sSKXhewoYx5GbTdUFWq/*)";
    /// # let wallet = doctest_wallet!();
    /// # let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap();
    /// let (mut psbt, _) = {
    ///     let mut builder = wallet.build_tx();
    ///     builder
    ///         .add_recipient(to_address.script_pubkey(), 50_000)
    ///         .enable_rbf();
    ///     builder.finish()?
    /// };
    /// let _ = wallet.sign(&mut psbt, SignOptions::default())?;
    /// let tx = psbt.extract_tx();
    /// // broadcast tx but it's taking too long to confirm so we want to bump the fee
    /// let (mut psbt, _) =  {
    ///     let mut builder = wallet.build_fee_bump(tx.txid())?;
    ///     builder
    ///         .fee_rate(FeeRate::from_sat_per_vb(5.0));
    ///     builder.finish()?
    /// };
    ///
    /// let _ = wallet.sign(&mut psbt, SignOptions::default())?;
    /// let fee_bumped_tx = psbt.extract_tx();
    /// // broadcast fee_bumped_tx to replace original
    /// # Ok::<(), bdk::Error>(())
    /// ```
    // TODO: support for merging multiple transactions while bumping the fees
    pub fn build_fee_bump(
        &self,
        txid: Txid,
    ) -> Result<TxBuilder<'_, D, DefaultCoinSelectionAlgorithm, BumpFee>, Error> {
        let mut details = match self.database.borrow().get_tx(&txid, true)? {
            None => return Err(Error::TransactionNotFound),
            Some(tx) if tx.transaction.is_none() => return Err(Error::TransactionNotFound),
            Some(tx) if tx.confirmation_time.is_some() => return Err(Error::TransactionConfirmed),
            Some(tx) => tx,
        };
        let mut tx = details.transaction.take().unwrap();
        if !tx
            .input
            .iter()
            .any(|txin| txin.sequence.to_consensus_u32() <= 0xFFFFFFFD)
        {
            return Err(Error::IrreplaceableTransaction);
        }

        let feerate = FeeRate::from_wu(details.fee.ok_or(Error::FeeRateUnavailable)?, tx.weight());

        // remove the inputs from the tx and process them
        let original_txin = tx.input.drain(..).collect::<Vec<_>>();
        let original_utxos = original_txin
            .iter()
            .map(|txin| -> Result<_, Error> {
                let txout = self
                    .database
                    .borrow()
                    .get_previous_output(&txin.previous_output)?
                    .ok_or(Error::UnknownUtxo)?;

                let (weight, keychain) = match self
                    .database
                    .borrow()
                    .get_path_from_script_pubkey(&txout.script_pubkey)?
                {
                    Some((keychain, _)) => (
                        self._get_descriptor_for_keychain(keychain)
                            .0
                            .max_satisfaction_weight()
                            .unwrap(),
                        keychain,
                    ),
                    None => {
                        // estimate the weight based on the scriptsig/witness size present in the
                        // original transaction
                        let weight =
                            serialize(&txin.script_sig).len() * 4 + serialize(&txin.witness).len();
                        (weight, KeychainKind::External)
                    }
                };

                let utxo = LocalUtxo {
                    outpoint: txin.previous_output,
                    txout,
                    keychain,
                    is_spent: true,
                };

                Ok(WeightedUtxo {
                    satisfaction_weight: weight,
                    utxo: Utxo::Local(utxo),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        if tx.output.len() > 1 {
            let mut change_index = None;
            for (index, txout) in tx.output.iter().enumerate() {
                let (_, change_type) = self._get_descriptor_for_keychain(KeychainKind::Internal);
                match self
                    .database
                    .borrow()
                    .get_path_from_script_pubkey(&txout.script_pubkey)?
                {
                    Some((keychain, _)) if keychain == change_type => change_index = Some(index),
                    _ => {}
                }
            }

            if let Some(change_index) = change_index {
                tx.output.remove(change_index);
            }
        }

        let params = TxParams {
            // TODO: figure out what rbf option should be?
            version: Some(tx_builder::Version(tx.version)),
            recipients: tx
                .output
                .into_iter()
                .map(|txout| (txout.script_pubkey, txout.value))
                .collect(),
            utxos: original_utxos,
            bumping_fee: Some(tx_builder::PreviousFee {
                absolute: details.fee.ok_or(Error::FeeRateUnavailable)?,
                rate: feerate.as_sat_per_vb(),
            }),
            ..Default::default()
        };

        Ok(TxBuilder {
            wallet: self,
            params,
            coin_selection: DefaultCoinSelectionAlgorithm::default(),
            phantom: core::marker::PhantomData,
        })
    }

    /// Sign a transaction with all the wallet's signers, in the order specified by every signer's
    /// [`SignerOrdering`]. This function returns the `Result` type with an encapsulated `bool` that has the value true if the PSBT was finalized, or false otherwise.
    ///
    /// The [`SignOptions`] can be used to tweak the behavior of the software signers, and the way
    /// the transaction is finalized at the end. Note that it can't be guaranteed that *every*
    /// signers will follow the options, but the "software signers" (WIF keys and `xprv`) defined
    /// in this library will.
    ///
    /// ## Example
    ///
    /// ```
    /// # use std::str::FromStr;
    /// # use bitcoin::*;
    /// # use bdk::*;
    /// # use bdk::database::*;
    /// # let descriptor = "wpkh(tpubD6NzVbkrYhZ4Xferm7Pz4VnjdcDPFyjVu5K4iZXQ4pVN8Cks4pHVowTBXBKRhX64pkRyJZJN5xAKj4UDNnLPb5p2sSKXhewoYx5GbTdUFWq/*)";
    /// # let wallet = doctest_wallet!();
    /// # let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap();
    /// let (mut psbt, _) = {
    ///     let mut builder = wallet.build_tx();
    ///     builder.add_recipient(to_address.script_pubkey(), 50_000);
    ///     builder.finish()?
    /// };
    /// let  finalized = wallet.sign(&mut psbt, SignOptions::default())?;
    /// assert!(finalized, "we should have signed all the inputs");
    /// # Ok::<(), bdk::Error>(())
    pub fn sign(
        &self,
        psbt: &mut psbt::PartiallySignedTransaction,
        sign_options: SignOptions,
    ) -> Result<bool, Error> {
        // This adds all the PSBT metadata for the inputs, which will help us later figure out how
        // to derive our keys
        self.update_psbt_with_descriptor(psbt)?;

        // If we aren't allowed to use `witness_utxo`, ensure that every input (except p2tr and finalized ones)
        // has the `non_witness_utxo`
        if !sign_options.trust_witness_utxo
            && psbt
                .inputs
                .iter()
                .filter(|i| i.final_script_witness.is_none() && i.final_script_sig.is_none())
                .filter(|i| i.tap_internal_key.is_none() && i.tap_merkle_root.is_none())
                .any(|i| i.non_witness_utxo.is_none())
        {
            return Err(Error::Signer(signer::SignerError::MissingNonWitnessUtxo));
        }

        // If the user hasn't explicitly opted-in, refuse to sign the transaction unless every input
        // is using `SIGHASH_ALL` or `SIGHASH_DEFAULT` for taproot
        if !sign_options.allow_all_sighashes
            && !psbt.inputs.iter().all(|i| {
                i.sighash_type.is_none()
                    || i.sighash_type == Some(EcdsaSighashType::All.into())
                    || i.sighash_type == Some(SchnorrSighashType::All.into())
                    || i.sighash_type == Some(SchnorrSighashType::Default.into())
            })
        {
            return Err(Error::Signer(signer::SignerError::NonStandardSighash));
        }

        for signer in self
            .signers
            .signers()
            .iter()
            .chain(self.change_signers.signers().iter())
        {
            signer.sign_transaction(psbt, &sign_options, &self.secp)?;
        }

        // attempt to finalize
        if sign_options.try_finalize {
            self.finalize_psbt(psbt, sign_options)
        } else {
            Ok(false)
        }
    }

    /// Return the spending policies for the wallet's descriptor
    pub fn policies(&self, keychain: KeychainKind) -> Result<Option<Policy>, Error> {
        match (keychain, self.change_descriptor.as_ref()) {
            (KeychainKind::External, _) => Ok(self.descriptor.extract_policy(
                &self.signers,
                BuildSatisfaction::None,
                &self.secp,
            )?),
            (KeychainKind::Internal, None) => Ok(None),
            (KeychainKind::Internal, Some(desc)) => Ok(desc.extract_policy(
                &self.change_signers,
                BuildSatisfaction::None,
                &self.secp,
            )?),
        }
    }

    /// Return the "public" version of the wallet's descriptor, meaning a new descriptor that has
    /// the same structure but with every secret key removed
    ///
    /// This can be used to build a watch-only version of a wallet
    pub fn public_descriptor(
        &self,
        keychain: KeychainKind,
    ) -> Result<Option<ExtendedDescriptor>, Error> {
        match (keychain, self.change_descriptor.as_ref()) {
            (KeychainKind::External, _) => Ok(Some(self.descriptor.clone())),
            (KeychainKind::Internal, None) => Ok(None),
            (KeychainKind::Internal, Some(desc)) => Ok(Some(desc.clone())),
        }
    }

    /// Finalize a PSBT, i.e., for each input determine if sufficient data is available to pass
    /// validation and construct the respective `scriptSig` or `scriptWitness`. Please refer to
    /// [BIP174](https://github.com/bitcoin/bips/blob/master/bip-0174.mediawiki#Input_Finalizer)
    /// for further information.
    ///
    /// Returns `true` if the PSBT could be finalized, and `false` otherwise.
    ///
    /// The [`SignOptions`] can be used to tweak the behavior of the finalizer.
    pub fn finalize_psbt(
        &self,
        psbt: &mut psbt::PartiallySignedTransaction,
        sign_options: SignOptions,
    ) -> Result<bool, Error> {
        let tx = &psbt.unsigned_tx;
        let mut finished = true;

        for (n, input) in tx.input.iter().enumerate() {
            let psbt_input = &psbt
                .inputs
                .get(n)
                .ok_or(Error::Signer(SignerError::InputIndexOutOfRange))?;
            if psbt_input.final_script_sig.is_some() || psbt_input.final_script_witness.is_some() {
                continue;
            }
            // if the height is None in the database it means it's still unconfirmed, so consider
            // that as a very high value
            let create_height = self
                .database
                .borrow()
                .get_tx(&input.previous_output.txid, false)?
                .map(|tx| tx.confirmation_time.map(|c| c.height).unwrap_or(u32::MAX));
            let last_sync_height = self
                .database()
                .get_sync_time()?
                .map(|sync_time| sync_time.block_time.height);
            let current_height = sign_options.assume_height.or(last_sync_height);

            debug!(
                "Input #{} - {}, using `create_height` = {:?}, `current_height` = {:?}",
                n, input.previous_output, create_height, current_height
            );

            // - Try to derive the descriptor by looking at the txout. If it's in our database, we
            //   know exactly which `keychain` to use, and which derivation index it is
            // - If that fails, try to derive it by looking at the psbt input: the complete logic
            //   is in `src/descriptor/mod.rs`, but it will basically look at `bip32_derivation`,
            //   `redeem_script` and `witness_script` to determine the right derivation
            // - If that also fails, it will try it on the internal descriptor, if present
            let desc = psbt
                .get_utxo_for(n)
                .map(|txout| self.get_descriptor_for_txout(&txout))
                .transpose()?
                .flatten()
                .or_else(|| {
                    self.descriptor.derive_from_psbt_input(
                        psbt_input,
                        psbt.get_utxo_for(n),
                        &self.secp,
                    )
                })
                .or_else(|| {
                    self.change_descriptor.as_ref().and_then(|desc| {
                        desc.derive_from_psbt_input(psbt_input, psbt.get_utxo_for(n), &self.secp)
                    })
                });

            match desc {
                Some(desc) => {
                    let mut tmp_input = bitcoin::TxIn::default();
                    match desc.satisfy(
                        &mut tmp_input,
                        (
                            PsbtInputSatisfier::new(psbt, n),
                            After::new(current_height, false),
                            Older::new(current_height, create_height, false),
                        ),
                    ) {
                        Ok(_) => {
                            let psbt_input = &mut psbt.inputs[n];
                            psbt_input.final_script_sig = Some(tmp_input.script_sig);
                            psbt_input.final_script_witness = Some(tmp_input.witness);
                            if sign_options.remove_partial_sigs {
                                psbt_input.partial_sigs.clear();
                            }
                        }
                        Err(e) => {
                            debug!("satisfy error {:?} for input {}", e, n);
                            finished = false
                        }
                    }
                }
                None => finished = false,
            }
        }

        Ok(finished)
    }

    /// Return the secp256k1 context used for all signing operations
    pub fn secp_ctx(&self) -> &SecpCtx {
        &self.secp
    }

    /// Returns the descriptor used to create addresses for a particular `keychain`.
    pub fn get_descriptor_for_keychain(&self, keychain: KeychainKind) -> &ExtendedDescriptor {
        let (descriptor, _) = self._get_descriptor_for_keychain(keychain);
        descriptor
    }

    // Internals

    fn _get_descriptor_for_keychain(
        &self,
        keychain: KeychainKind,
    ) -> (&ExtendedDescriptor, KeychainKind) {
        match keychain {
            KeychainKind::Internal if self.change_descriptor.is_some() => (
                self.change_descriptor.as_ref().unwrap(),
                KeychainKind::Internal,
            ),
            _ => (&self.descriptor, KeychainKind::External),
        }
    }

    fn get_descriptor_for_txout(&self, txout: &TxOut) -> Result<Option<DerivedDescriptor>, Error> {
        Ok(self
            .database
            .borrow()
            .get_path_from_script_pubkey(&txout.script_pubkey)?
            .map(|(keychain, child)| (self.get_descriptor_for_keychain(keychain), child))
            .map(|(desc, child)| desc.at_derivation_index(child)))
    }

    fn fetch_and_increment_index(&self, keychain: KeychainKind) -> Result<u32, Error> {
        let (descriptor, keychain) = self._get_descriptor_for_keychain(keychain);
        let index = match descriptor.has_wildcard() {
            false => 0,
            true => self.database.borrow_mut().increment_last_index(keychain)?,
        };

        if self
            .database
            .borrow()
            .get_script_pubkey_from_path(keychain, index)?
            .is_none()
        {
            self.cache_addresses(keychain, index, CACHE_ADDR_BATCH_SIZE)?;
        }

        Ok(index)
    }

    fn fetch_index(&self, keychain: KeychainKind) -> Result<u32, Error> {
        let (descriptor, keychain) = self._get_descriptor_for_keychain(keychain);
        let index = match descriptor.has_wildcard() {
            false => Some(0),
            true => self.database.borrow_mut().get_last_index(keychain)?,
        };

        if let Some(i) = index {
            Ok(i)
        } else {
            self.fetch_and_increment_index(keychain)
        }
    }

    fn set_index(&self, keychain: KeychainKind, index: u32) -> Result<(), Error> {
        self.database.borrow_mut().set_last_index(keychain, index)?;
        Ok(())
    }

    fn cache_addresses(
        &self,
        keychain: KeychainKind,
        from: u32,
        mut count: u32,
    ) -> Result<(), Error> {
        let (descriptor, keychain) = self._get_descriptor_for_keychain(keychain);
        if !descriptor.has_wildcard() {
            if from > 0 {
                return Ok(());
            }

            count = 1;
        }

        let mut address_batch = self.database.borrow().begin_batch();

        let start_time = time::Instant::new();
        for i in from..(from + count) {
            address_batch.set_script_pubkey(
                &descriptor.at_derivation_index(i).script_pubkey(),
                keychain,
                i,
            )?;
        }

        info!(
            "Derivation of {} addresses from {} took {} ms",
            count,
            from,
            start_time.elapsed().as_millis()
        );

        self.database.borrow_mut().commit_batch(address_batch)?;

        Ok(())
    }

    fn get_available_utxos(&self) -> Result<Vec<(LocalUtxo, usize)>, Error> {
        Ok(self
            .list_unspent()?
            .into_iter()
            .map(|utxo| {
                let keychain = utxo.keychain;
                (
                    utxo,
                    self.get_descriptor_for_keychain(keychain)
                        .max_satisfaction_weight()
                        .unwrap(),
                )
            })
            .collect())
    }

    /// Given the options returns the list of utxos that must be used to form the
    /// transaction and any further that may be used if needed.
    #[allow(clippy::type_complexity)]
    #[allow(clippy::too_many_arguments)]
    fn preselect_utxos(
        &self,
        change_policy: tx_builder::ChangeSpendPolicy,
        unspendable: &HashSet<OutPoint>,
        manually_selected: Vec<WeightedUtxo>,
        must_use_all_available: bool,
        manual_only: bool,
        must_only_use_confirmed_tx: bool,
        current_height: Option<u32>,
    ) -> Result<(Vec<WeightedUtxo>, Vec<WeightedUtxo>), Error> {
        //    must_spend <- manually selected utxos
        //    may_spend  <- all other available utxos
        let mut may_spend = self.get_available_utxos()?;

        may_spend.retain(|may_spend| {
            !manually_selected
                .iter()
                .any(|manually_selected| manually_selected.utxo.outpoint() == may_spend.0.outpoint)
        });
        let mut must_spend = manually_selected;

        // NOTE: we are intentionally ignoring `unspendable` here. i.e manual
        // selection overrides unspendable.
        if manual_only {
            return Ok((must_spend, vec![]));
        }

        let database = self.database.borrow();
        let satisfies_confirmed = may_spend
            .iter()
            .map(|u| {
                database
                    .get_tx(&u.0.outpoint.txid, true)
                    .map(|tx| match tx {
                        // We don't have the tx in the db for some reason,
                        // so we can't know for sure if it's mature or not.
                        // We prefer not to spend it.
                        None => false,
                        Some(tx) => {
                            // Whether the UTXO is mature and, if needed, confirmed
                            let mut spendable = true;
                            if must_only_use_confirmed_tx && tx.confirmation_time.is_none() {
                                return false;
                            }
                            if tx
                                .transaction
                                .expect("We specifically ask for the transaction above")
                                .is_coin_base()
                            {
                                if let Some(current_height) = current_height {
                                    match &tx.confirmation_time {
                                        Some(t) => {
                                            // https://github.com/bitcoin/bitcoin/blob/c5e67be03bb06a5d7885c55db1f016fbf2333fe3/src/validation.cpp#L373-L375
                                            spendable &= (current_height.saturating_sub(t.height))
                                                >= COINBASE_MATURITY;
                                        }
                                        None => spendable = false,
                                    }
                                }
                            }
                            spendable
                        }
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut i = 0;
        may_spend.retain(|u| {
            let retain = change_policy.is_satisfied_by(&u.0)
                && !unspendable.contains(&u.0.outpoint)
                && satisfies_confirmed[i];
            i += 1;
            retain
        });

        let mut may_spend = may_spend
            .into_iter()
            .map(|(local_utxo, satisfaction_weight)| WeightedUtxo {
                satisfaction_weight,
                utxo: Utxo::Local(local_utxo),
            })
            .collect();

        if must_use_all_available {
            must_spend.append(&mut may_spend);
        }

        Ok((must_spend, may_spend))
    }

    fn complete_transaction(
        &self,
        tx: Transaction,
        selected: Vec<Utxo>,
        params: TxParams,
    ) -> Result<psbt::PartiallySignedTransaction, Error> {
        let mut psbt = psbt::PartiallySignedTransaction::from_unsigned_tx(tx)?;

        if params.add_global_xpubs {
            let mut all_xpubs = self.descriptor.get_extended_keys()?;
            if let Some(change_descriptor) = &self.change_descriptor {
                all_xpubs.extend(change_descriptor.get_extended_keys()?);
            }

            for xpub in all_xpubs {
                let origin = match xpub.origin {
                    Some(origin) => origin,
                    None if xpub.xkey.depth == 0 => {
                        (xpub.root_fingerprint(&self.secp), vec![].into())
                    }
                    _ => return Err(Error::MissingKeyOrigin(xpub.xkey.to_string())),
                };

                psbt.xpub.insert(xpub.xkey, origin);
            }
        }

        let mut lookup_output = selected
            .into_iter()
            .map(|utxo| (utxo.outpoint(), utxo))
            .collect::<HashMap<_, _>>();

        // add metadata for the inputs
        for (psbt_input, input) in psbt.inputs.iter_mut().zip(psbt.unsigned_tx.input.iter()) {
            let utxo = match lookup_output.remove(&input.previous_output) {
                Some(utxo) => utxo,
                None => continue,
            };

            match utxo {
                Utxo::Local(utxo) => {
                    *psbt_input =
                        match self.get_psbt_input(utxo, params.sighash, params.only_witness_utxo) {
                            Ok(psbt_input) => psbt_input,
                            Err(e) => match e {
                                Error::UnknownUtxo => psbt::Input {
                                    sighash_type: params.sighash,
                                    ..psbt::Input::default()
                                },
                                _ => return Err(e),
                            },
                        }
                }
                Utxo::Foreign {
                    psbt_input: foreign_psbt_input,
                    outpoint,
                } => {
                    let is_taproot = foreign_psbt_input
                        .witness_utxo
                        .as_ref()
                        .map(|txout| txout.script_pubkey.is_v1_p2tr())
                        .unwrap_or(false);
                    if !is_taproot
                        && !params.only_witness_utxo
                        && foreign_psbt_input.non_witness_utxo.is_none()
                    {
                        return Err(Error::Generic(format!(
                            "Missing non_witness_utxo on foreign utxo {}",
                            outpoint
                        )));
                    }
                    *psbt_input = *foreign_psbt_input;
                }
            }
        }

        self.update_psbt_with_descriptor(&mut psbt)?;

        Ok(psbt)
    }

    /// get the corresponding PSBT Input for a LocalUtxo
    pub fn get_psbt_input(
        &self,
        utxo: LocalUtxo,
        sighash_type: Option<psbt::PsbtSighashType>,
        only_witness_utxo: bool,
    ) -> Result<psbt::Input, Error> {
        // Try to find the prev_script in our db to figure out if this is internal or external,
        // and the derivation index
        let (keychain, child) = self
            .database
            .borrow()
            .get_path_from_script_pubkey(&utxo.txout.script_pubkey)?
            .ok_or(Error::UnknownUtxo)?;

        let mut psbt_input = psbt::Input {
            sighash_type,
            ..psbt::Input::default()
        };

        let desc = self.get_descriptor_for_keychain(keychain);
        let derived_descriptor = desc.at_derivation_index(child);

        psbt_input
            .update_with_descriptor_unchecked(&derived_descriptor)
            .map_err(MiniscriptPsbtError::Conversion)?;

        let prev_output = utxo.outpoint;
        if let Some(prev_tx) = self.database.borrow().get_raw_tx(&prev_output.txid)? {
            if desc.is_witness() || desc.is_taproot() {
                psbt_input.witness_utxo = Some(prev_tx.output[prev_output.vout as usize].clone());
            }
            if !desc.is_taproot() && (!desc.is_witness() || !only_witness_utxo) {
                psbt_input.non_witness_utxo = Some(prev_tx);
            }
        }
        Ok(psbt_input)
    }

    fn update_psbt_with_descriptor(
        &self,
        psbt: &mut psbt::PartiallySignedTransaction,
    ) -> Result<(), Error> {
        // We need to borrow `psbt` mutably within the loops, so we have to allocate a vec for all
        // the input utxos and outputs
        //
        // Clippy complains that the collect is not required, but that's wrong
        #[allow(clippy::needless_collect)]
        let utxos = (0..psbt.inputs.len())
            .filter_map(|i| psbt.get_utxo_for(i).map(|utxo| (true, i, utxo)))
            .chain(
                psbt.unsigned_tx
                    .output
                    .iter()
                    .enumerate()
                    .map(|(i, out)| (false, i, out.clone())),
            )
            .collect::<Vec<_>>();

        // Try to figure out the keychain and derivation for every input and output
        for (is_input, index, out) in utxos.into_iter() {
            if let Some((keychain, child)) = self
                .database
                .borrow()
                .get_path_from_script_pubkey(&out.script_pubkey)?
            {
                debug!(
                    "Found descriptor for input #{} {:?}/{}",
                    index, keychain, child
                );

                let desc = self.get_descriptor_for_keychain(keychain);
                let desc = desc.at_derivation_index(child);

                if is_input {
                    psbt.update_input_with_descriptor(index, &desc)
                        .map_err(MiniscriptPsbtError::UtxoUpdate)?;
                } else {
                    psbt.update_output_with_descriptor(index, &desc)
                        .map_err(MiniscriptPsbtError::OutputUpdate)?;
                }
            }
        }

        Ok(())
    }

    /// Return an immutable reference to the internal database
    pub fn database(&self) -> impl std::ops::Deref<Target = D> + '_ {
        self.database.borrow()
    }

    /// Sync the internal database with the blockchain
    #[maybe_async]
    pub fn sync<B: WalletSync + GetHeight>(
        &self,
        blockchain: &B,
        sync_opts: SyncOptions,
    ) -> Result<(), Error> {
        debug!("Begin sync...");

        // TODO: for the next runs, we cannot reuse the `sync_opts.progress` object due to trait
        // restrictions
        let mut progress_iter = sync_opts.progress.into_iter();
        let mut new_progress = || {
            progress_iter
                .next()
                .unwrap_or_else(|| Box::new(NoopProgress))
        };

        let run_setup = self.ensure_addresses_cached(CACHE_ADDR_BATCH_SIZE)?;
        debug!("run_setup: {}", run_setup);

        // TODO: what if i generate an address first and cache some addresses?
        // TODO: we should sync if generating an address triggers a new batch to be stored

        // We need to ensure descriptor is derivable to fullfil "missing cache", otherwise we will
        // end up with an infinite loop
        let has_wildcard = self.descriptor.has_wildcard()
            && (self.change_descriptor.is_none()
                || self.change_descriptor.as_ref().unwrap().has_wildcard());

        // Restrict max rounds in case of faulty "missing cache" implementation by blockchain
        let max_rounds = if has_wildcard { 100 } else { 1 };

        for _ in 0..max_rounds {
            let sync_res = if run_setup {
                maybe_await!(blockchain.wallet_setup(&self.database, new_progress()))
            } else {
                maybe_await!(blockchain.wallet_sync(&self.database, new_progress()))
            };

            // If the error is the special `MissingCachedScripts` error, we return the number of
            // scripts we should ensure cached.
            // On any other error, we should return the error.
            // On no error, we say `ensure_cache` is 0.
            let ensure_cache = sync_res.map_or_else(
                |e| match e {
                    Error::MissingCachedScripts(inner) => {
                        // each call to `WalletSync` is expensive, maximize on scripts to search for
                        let extra =
                            std::cmp::max(inner.missing_count as u32, CACHE_ADDR_BATCH_SIZE);
                        let last = inner.last_count as u32;
                        Ok(extra + last)
                    }
                    _ => Err(e),
                },
                |_| Ok(0_u32),
            )?;

            // cache and try again, break when there is nothing to cache
            if !self.ensure_addresses_cached(ensure_cache)? {
                break;
            }
        }

        let sync_time = SyncTime {
            block_time: BlockTime {
                height: maybe_await!(blockchain.get_height())?,
                timestamp: time::get_timestamp(),
            },
        };
        debug!("Saving `sync_time` = {:?}", sync_time);
        self.database.borrow_mut().set_sync_time(sync_time)?;

        Ok(())
    }

    /// Return the checksum of the public descriptor associated to `keychain`
    ///
    /// Internally calls [`Self::get_descriptor_for_keychain`] to fetch the right descriptor
    pub fn descriptor_checksum(&self, keychain: KeychainKind) -> String {
        self.get_descriptor_for_keychain(keychain)
            .to_string()
            .split_once('#')
            .unwrap()
            .1
            .to_string()
    }
}

/// Deterministically generate a unique name given the descriptors defining the wallet
///
/// Compatible with [`wallet_name_from_descriptor`]
pub fn wallet_name_from_descriptor<T>(
    descriptor: T,
    change_descriptor: Option<T>,
    network: Network,
    secp: &SecpCtx,
) -> Result<String, Error>
where
    T: IntoWalletDescriptor,
{
    //TODO check descriptors contains only public keys
    let descriptor = descriptor
        .into_wallet_descriptor(secp, network)?
        .0
        .to_string();
    let mut wallet_name = calc_checksum(&descriptor[..descriptor.find('#').unwrap()])?;
    if let Some(change_descriptor) = change_descriptor {
        let change_descriptor = change_descriptor
            .into_wallet_descriptor(secp, network)?
            .0
            .to_string();
        wallet_name.push_str(
            calc_checksum(&change_descriptor[..change_descriptor.find('#').unwrap()])?.as_str(),
        );
    }

    Ok(wallet_name)
}

/// Return a fake wallet that appears to be funded for testing.
pub fn get_funded_wallet(
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
        .database
        .borrow_mut()
        .set_script_pubkey(
            &bitcoin::Address::from_str(&tx_meta.output.get(0).unwrap().to_address)
                .unwrap()
                .script_pubkey(),
            KeychainKind::External,
            funding_address_kix,
        )
        .unwrap();
    wallet
        .database
        .borrow_mut()
        .set_last_index(KeychainKind::External, funding_address_kix)
        .unwrap();

    let txid = crate::populate_test_db!(wallet.database.borrow_mut(), tx_meta, Some(100));

    (wallet, descriptors, txid)
}

#[cfg(test)]
pub(crate) mod test {
    use assert_matches::assert_matches;
    use bitcoin::{util::psbt, Network, PackedLockTime, Sequence};

    use crate::database::Database;
    use crate::types::KeychainKind;

    use super::*;
    use crate::signer::{SignOptions, SignerError};
    use crate::wallet::AddressIndex::{LastUnused, New, Peek, Reset};

    // The satisfaction size of a P2WPKH is 112 WU =
    // 1 (elements in witness) + 1 (OP_PUSH) + 33 (pk) + 1 (OP_PUSH) + 72 (signature + sighash) + 1*4 (script len)
    // On the witness itself, we have to push once for the pk (33WU) and once for signature + sighash (72WU), for
    // a total of 105 WU.
    // Here, we push just once for simplicity, so we have to add an extra byte for the missing
    // OP_PUSH.
    const P2WPKH_FAKE_WITNESS_SIZE: usize = 106;

    #[test]
    fn test_descriptor_checksum() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let checksum = wallet.descriptor_checksum(KeychainKind::External);
        assert_eq!(checksum.len(), 8);
        assert_eq!(
            calc_checksum(&wallet.descriptor.to_string()).unwrap(),
            checksum
        );
    }

    #[test]
    fn test_db_checksum() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let desc = wallet.descriptor.to_string();

        let checksum = calc_checksum_bytes_internal(&desc, true).unwrap();
        let checksum_inception = calc_checksum_bytes_internal(&desc, false).unwrap();
        let checksum_invalid = [b'q'; 8];

        let mut db = MemoryDatabase::new();
        db.check_descriptor_checksum(KeychainKind::External, checksum)
            .expect("failed to save actual checksum");
        Wallet::db_checksum(&mut db, &desc, KeychainKind::External)
            .expect("db that uses actual checksum should be supported");

        let mut db = MemoryDatabase::new();
        db.check_descriptor_checksum(KeychainKind::External, checksum_inception)
            .expect("failed to save checksum inception");
        Wallet::db_checksum(&mut db, &desc, KeychainKind::External)
            .expect("db that uses checksum inception should be supported");

        let mut db = MemoryDatabase::new();
        db.check_descriptor_checksum(KeychainKind::External, checksum_invalid)
            .expect("failed to save invalid checksum");
        Wallet::db_checksum(&mut db, &desc, KeychainKind::External)
            .expect_err("db that uses invalid checksum should fail");
    }

    #[test]
    fn test_get_funded_wallet_balance() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        assert_eq!(wallet.get_balance().unwrap().confirmed, 50000);
    }

    #[test]
    fn test_cache_addresses_fixed() {
        let db = MemoryDatabase::new();
        let wallet = Wallet::new(
            "wpkh(L5EZftvrYaSudiozVRzTqLcHLNDoVn7H5HSfM9BAN6tMJX8oTWz6)",
            None,
            Network::Testnet,
            db,
        )
        .unwrap();

        assert_eq!(
            wallet.get_address(New).unwrap().to_string(),
            "tb1qj08ys4ct2hzzc2hcz6h2hgrvlmsjynaw43s835"
        );
        assert_eq!(
            wallet.get_address(New).unwrap().to_string(),
            "tb1qj08ys4ct2hzzc2hcz6h2hgrvlmsjynaw43s835"
        );

        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(KeychainKind::External, 0)
            .unwrap()
            .is_some());
        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(KeychainKind::Internal, 0)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_cache_addresses() {
        let db = MemoryDatabase::new();
        let wallet = Wallet::new("wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)", None, Network::Testnet, db).unwrap();

        assert_eq!(
            wallet.get_address(New).unwrap().to_string(),
            "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
        );
        assert_eq!(
            wallet.get_address(New).unwrap().to_string(),
            "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
        );

        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(KeychainKind::External, CACHE_ADDR_BATCH_SIZE - 1)
            .unwrap()
            .is_some());
        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(KeychainKind::External, CACHE_ADDR_BATCH_SIZE)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_cache_addresses_refill() {
        let db = MemoryDatabase::new();
        let wallet = Wallet::new("wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)", None, Network::Testnet, db).unwrap();

        assert_eq!(
            wallet.get_address(New).unwrap().to_string(),
            "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
        );
        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(KeychainKind::External, CACHE_ADDR_BATCH_SIZE - 1)
            .unwrap()
            .is_some());

        for _ in 0..CACHE_ADDR_BATCH_SIZE {
            wallet.get_address(New).unwrap();
        }

        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(KeychainKind::External, CACHE_ADDR_BATCH_SIZE * 2 - 1)
            .unwrap()
            .is_some());
    }

    pub(crate) fn get_test_wpkh() -> &'static str {
        "wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)"
    }

    pub(crate) fn get_test_single_sig_csv() -> &'static str {
        // and(pk(Alice),older(6))
        "wsh(and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),older(6)))"
    }

    pub(crate) fn get_test_a_or_b_plus_csv() -> &'static str {
        // or(pk(Alice),and(pk(Bob),older(144)))
        "wsh(or_d(pk(cRjo6jqfVNP33HhSS76UhXETZsGTZYx8FMFvR9kpbtCSV1PmdZdu),and_v(v:pk(cMnkdebixpXMPfkcNEjjGin7s94hiehAH4mLbYkZoh9KSiNNmqC8),older(144))))"
    }

    pub(crate) fn get_test_single_sig_cltv() -> &'static str {
        // and(pk(Alice),after(100000))
        "wsh(and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),after(100000)))"
    }

    pub(crate) fn get_test_tr_single_sig() -> &'static str {
        "tr(cNJmN3fH9DDbDt131fQNkVakkpzawJBSeybCUNmP1BovpmGQ45xG)"
    }

    pub(crate) fn get_test_tr_with_taptree() -> &'static str {
        "tr(b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55,{pk(cPZzKuNmpuUjD1e8jUU4PVzy2b5LngbSip8mBsxf4e7rSFZVb4Uh),pk(8aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642)})"
    }

    pub(crate) fn get_test_tr_with_taptree_both_priv() -> &'static str {
        "tr(b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55,{pk(cPZzKuNmpuUjD1e8jUU4PVzy2b5LngbSip8mBsxf4e7rSFZVb4Uh),pk(cNaQCDwmmh4dS9LzCgVtyy1e1xjCJ21GUDHe9K98nzb689JvinGV)})"
    }

    pub(crate) fn get_test_tr_repeated_key() -> &'static str {
        "tr(b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55,{and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),after(100)),and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),after(200))})"
    }

    pub(crate) fn get_test_tr_single_sig_xprv() -> &'static str {
        "tr(tprv8ZgxMBicQKsPdDArR4xSAECuVxeX1jwwSXR4ApKbkYgZiziDc4LdBy2WvJeGDfUSE4UT4hHhbgEwbdq8ajjUHiKDegkwrNU6V55CxcxonVN/*)"
    }

    pub(crate) fn get_test_tr_with_taptree_xprv() -> &'static str {
        "tr(cNJmN3fH9DDbDt131fQNkVakkpzawJBSeybCUNmP1BovpmGQ45xG,{pk(tprv8ZgxMBicQKsPdDArR4xSAECuVxeX1jwwSXR4ApKbkYgZiziDc4LdBy2WvJeGDfUSE4UT4hHhbgEwbdq8ajjUHiKDegkwrNU6V55CxcxonVN/*),pk(8aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642)})"
    }

    pub(crate) fn get_test_tr_dup_keys() -> &'static str {
        "tr(cNJmN3fH9DDbDt131fQNkVakkpzawJBSeybCUNmP1BovpmGQ45xG,{pk(8aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642),pk(8aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642)})"
    }

    macro_rules! assert_fee_rate {
        ($psbt:expr, $fees:expr, $fee_rate:expr $( ,@dust_change $( $dust_change:expr )* )* $( ,@add_signature $( $add_signature:expr )* )* ) => ({
            let psbt = $psbt.clone();
            #[allow(unused_mut)]
            let mut tx = $psbt.clone().extract_tx();
            $(
                $( $add_signature )*
                for txin in &mut tx.input {
                    txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
                }
            )*

            #[allow(unused_mut)]
            #[allow(unused_assignments)]
            let mut dust_change = false;
            $(
                $( $dust_change )*
                dust_change = true;
            )*

            let fee_amount = psbt
                .inputs
                .iter()
                .fold(0, |acc, i| acc + i.witness_utxo.as_ref().unwrap().value)
                - psbt
                    .unsigned_tx
                    .output
                    .iter()
                    .fold(0, |acc, o| acc + o.value);

            assert_eq!(fee_amount, $fees);

            let tx_fee_rate = FeeRate::from_wu($fees, tx.weight());
            let fee_rate = $fee_rate;

            if !dust_change {
                assert!(tx_fee_rate >= fee_rate && (tx_fee_rate - fee_rate).as_sat_per_vb().abs() < 0.5, "Expected fee rate of {:?}, the tx has {:?}", fee_rate, tx_fee_rate);
            } else {
                assert!(tx_fee_rate >= fee_rate, "Expected fee rate of at least {:?}, the tx has {:?}", fee_rate, tx_fee_rate);
            }
        });
    }

    macro_rules! from_str {
        ($e:expr, $t:ty) => {{
            use std::str::FromStr;
            <$t>::from_str($e).unwrap()
        }};

        ($e:expr) => {
            from_str!($e, _)
        };
    }

    #[test]
    #[should_panic(expected = "NoRecipients")]
    fn test_create_tx_empty_recipients() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        wallet.build_tx().finish().unwrap();
    }

    #[test]
    #[should_panic(expected = "NoUtxosSelected")]
    fn test_create_tx_manually_selected_empty_utxos() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .manually_selected_only();
        builder.finish().unwrap();
    }

    #[test]
    #[should_panic(expected = "Invalid version `0`")]
    fn test_create_tx_version_0() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .version(0);
        builder.finish().unwrap();
    }

    #[test]
    #[should_panic(
        expected = "TxBuilder requested version `1`, but at least `2` is needed to use OP_CSV"
    )]
    fn test_create_tx_version_1_csv() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_csv());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .version(1);
        builder.finish().unwrap();
    }

    #[test]
    fn test_create_tx_custom_version() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .version(42);
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(psbt.unsigned_tx.version, 42);
    }

    #[test]
    fn test_create_tx_default_locktime() {
        let descriptors = testutils!(@descriptors (get_test_wpkh()));
        let wallet = Wallet::new(
            &descriptors.0,
            None,
            Network::Regtest,
            AnyDatabase::Memory(MemoryDatabase::new()),
        )
        .unwrap();

        let tx_meta = testutils! {
                @tx ( (@external descriptors, 0) => 50_000 )
        };

        // Add the transaction to our db, but do not sync the db.
        crate::populate_test_db!(wallet.database.borrow_mut(), tx_meta, None);

        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let (psbt, _) = builder.finish().unwrap();

        // Since we never synced the wallet we don't have a last_sync_height
        // we could use to try to prevent fee sniping. We default to 0.
        assert_eq!(psbt.unsigned_tx.lock_time, PackedLockTime(0));
    }

    #[test]
    fn test_create_tx_fee_sniping_locktime_provided_height() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let sync_time = SyncTime {
            block_time: BlockTime {
                height: 24,
                timestamp: 0,
            },
        };
        wallet
            .database
            .borrow_mut()
            .set_sync_time(sync_time)
            .unwrap();
        let current_height = 25;
        builder.current_height(current_height);
        let (psbt, _) = builder.finish().unwrap();

        // current_height will override the last sync height
        assert_eq!(psbt.unsigned_tx.lock_time, PackedLockTime(current_height));
    }

    #[test]
    fn test_create_tx_fee_sniping_locktime_last_sync() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let sync_time = SyncTime {
            block_time: BlockTime {
                height: 25,
                timestamp: 0,
            },
        };
        wallet
            .database
            .borrow_mut()
            .set_sync_time(sync_time.clone())
            .unwrap();
        let (psbt, _) = builder.finish().unwrap();

        // If there's no current_height we're left with using the last sync height
        assert_eq!(
            psbt.unsigned_tx.lock_time,
            PackedLockTime(sync_time.block_time.height)
        );
    }

    #[test]
    fn test_create_tx_default_locktime_cltv() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_cltv());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(psbt.unsigned_tx.lock_time, PackedLockTime(100_000));
    }

    #[test]
    fn test_create_tx_custom_locktime() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .current_height(630_001)
            .nlocktime(LockTime::from_height(630_000).unwrap());
        let (psbt, _) = builder.finish().unwrap();

        // When we explicitly specify a nlocktime
        // we don't try any fee sniping prevention trick
        // (we ignore the current_height)
        assert_eq!(psbt.unsigned_tx.lock_time, PackedLockTime(630_000));
    }

    #[test]
    fn test_create_tx_custom_locktime_compatible_with_cltv() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_cltv());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .nlocktime(LockTime::from_height(630_000).unwrap());
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(psbt.unsigned_tx.lock_time, PackedLockTime(630_000));
    }

    #[test]
    #[should_panic(
        expected = "TxBuilder requested timelock of `Blocks(Height(50000))`, but at least `Blocks(Height(100000))` is required to spend from this script"
    )]
    fn test_create_tx_custom_locktime_incompatible_with_cltv() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_cltv());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .nlocktime(LockTime::from_height(50000).unwrap());
        builder.finish().unwrap();
    }

    #[test]
    fn test_create_tx_no_rbf_csv() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_csv());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(6));
    }

    #[test]
    fn test_create_tx_with_default_rbf_csv() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_csv());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .enable_rbf();
        let (psbt, _) = builder.finish().unwrap();
        // When CSV is enabled it takes precedence over the rbf value (unless forced by the user).
        // It will be set to the OP_CSV value, in this case 6
        assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(6));
    }

    #[test]
    #[should_panic(
        expected = "Cannot enable RBF with nSequence `Sequence(3)` given a required OP_CSV of `Sequence(6)`"
    )]
    fn test_create_tx_with_custom_rbf_csv() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_csv());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .enable_rbf_with_sequence(Sequence(3));
        builder.finish().unwrap();
    }

    #[test]
    fn test_create_tx_no_rbf_cltv() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_cltv());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(0xFFFFFFFE));
    }

    #[test]
    #[should_panic(expected = "Cannot enable RBF with a nSequence >= 0xFFFFFFFE")]
    fn test_create_tx_invalid_rbf_sequence() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .enable_rbf_with_sequence(Sequence(0xFFFFFFFE));
        builder.finish().unwrap();
    }

    #[test]
    fn test_create_tx_custom_rbf_sequence() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .enable_rbf_with_sequence(Sequence(0xDEADBEEF));
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(0xDEADBEEF));
    }

    #[test]
    fn test_create_tx_default_sequence() {
        let descriptors = testutils!(@descriptors (get_test_wpkh()));
        let wallet = Wallet::new(
            &descriptors.0,
            None,
            Network::Regtest,
            AnyDatabase::Memory(MemoryDatabase::new()),
        )
        .unwrap();

        let tx_meta = testutils! {
                @tx ( (@external descriptors, 0) => 50_000 )
        };

        // Add the transaction to our db, but do not sync the db. Unsynced db
        // should trigger the default sequence value for a new transaction as 0xFFFFFFFF
        crate::populate_test_db!(wallet.database.borrow_mut(), tx_meta, None);

        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(0xFFFFFFFF));
    }

    #[test]
    #[should_panic(
        expected = "The `change_policy` can be set only if the wallet has a change_descriptor"
    )]
    fn test_create_tx_change_policy_no_internal() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .do_not_spend_change();
        builder.finish().unwrap();
    }

    #[test]
    fn test_create_tx_drain_wallet_and_drain_to() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(psbt.unsigned_tx.output.len(), 1);
        assert_eq!(
            psbt.unsigned_tx.output[0].value,
            50_000 - details.fee.unwrap_or(0)
        );
    }

    #[test]
    fn test_create_tx_drain_wallet_and_drain_to_and_with_recipient() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap();
        let drain_addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 20_000)
            .drain_to(drain_addr.script_pubkey())
            .drain_wallet();
        let (psbt, details) = builder.finish().unwrap();
        let outputs = psbt.unsigned_tx.output;

        assert_eq!(outputs.len(), 2);
        let main_output = outputs
            .iter()
            .find(|x| x.script_pubkey == addr.script_pubkey())
            .unwrap();
        let drain_output = outputs
            .iter()
            .find(|x| x.script_pubkey == drain_addr.script_pubkey())
            .unwrap();
        assert_eq!(main_output.value, 20_000,);
        assert_eq!(drain_output.value, 30_000 - details.fee.unwrap_or(0));
    }

    #[test]
    fn test_create_tx_drain_to_and_utxos() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let utxos: Vec<_> = wallet
            .get_available_utxos()
            .unwrap()
            .into_iter()
            .map(|(u, _)| u.outpoint)
            .collect();
        let mut builder = wallet.build_tx();
        builder
            .drain_to(addr.script_pubkey())
            .add_utxos(&utxos)
            .unwrap();
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(psbt.unsigned_tx.output.len(), 1);
        assert_eq!(
            psbt.unsigned_tx.output[0].value,
            50_000 - details.fee.unwrap_or(0)
        );
    }

    #[test]
    #[should_panic(expected = "NoRecipients")]
    fn test_create_tx_drain_to_no_drain_wallet_no_utxos() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let drain_addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(drain_addr.script_pubkey());
        builder.finish().unwrap();
    }

    #[test]
    fn test_create_tx_default_fee_rate() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let (psbt, details) = builder.finish().unwrap();

        assert_fee_rate!(psbt, details.fee.unwrap_or(0), FeeRate::default(), @add_signature);
    }

    #[test]
    fn test_create_tx_custom_fee_rate() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .fee_rate(FeeRate::from_sat_per_vb(5.0));
        let (psbt, details) = builder.finish().unwrap();

        assert_fee_rate!(psbt, details.fee.unwrap_or(0), FeeRate::from_sat_per_vb(5.0), @add_signature);
    }

    #[test]
    fn test_create_tx_absolute_fee() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .drain_to(addr.script_pubkey())
            .drain_wallet()
            .fee_absolute(100);
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(details.fee.unwrap_or(0), 100);
        assert_eq!(psbt.unsigned_tx.output.len(), 1);
        assert_eq!(
            psbt.unsigned_tx.output[0].value,
            50_000 - details.fee.unwrap_or(0)
        );
    }

    #[test]
    fn test_create_tx_absolute_zero_fee() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .drain_to(addr.script_pubkey())
            .drain_wallet()
            .fee_absolute(0);
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(details.fee.unwrap_or(0), 0);
        assert_eq!(psbt.unsigned_tx.output.len(), 1);
        assert_eq!(
            psbt.unsigned_tx.output[0].value,
            50_000 - details.fee.unwrap_or(0)
        );
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_create_tx_absolute_high_fee() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .drain_to(addr.script_pubkey())
            .drain_wallet()
            .fee_absolute(60_000);
        let (_psbt, _details) = builder.finish().unwrap();
    }

    #[test]
    fn test_create_tx_add_change() {
        use super::tx_builder::TxOrdering;

        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .ordering(TxOrdering::Untouched);
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(psbt.unsigned_tx.output.len(), 2);
        assert_eq!(psbt.unsigned_tx.output[0].value, 25_000);
        assert_eq!(
            psbt.unsigned_tx.output[1].value,
            25_000 - details.fee.unwrap_or(0)
        );
    }

    #[test]
    fn test_create_tx_skip_change_dust() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 49_800);
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(psbt.unsigned_tx.output.len(), 1);
        assert_eq!(psbt.unsigned_tx.output[0].value, 49_800);
        assert_eq!(details.fee.unwrap_or(0), 200);
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_create_tx_drain_to_dust_amount() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        // very high fee rate, so that the only output would be below dust
        let mut builder = wallet.build_tx();
        builder
            .drain_to(addr.script_pubkey())
            .drain_wallet()
            .fee_rate(FeeRate::from_sat_per_vb(453.0));
        builder.finish().unwrap();
    }

    #[test]
    fn test_create_tx_ordering_respected() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 30_000)
            .add_recipient(addr.script_pubkey(), 10_000)
            .ordering(super::tx_builder::TxOrdering::Bip69Lexicographic);
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(psbt.unsigned_tx.output.len(), 3);
        assert_eq!(
            psbt.unsigned_tx.output[0].value,
            10_000 - details.fee.unwrap_or(0)
        );
        assert_eq!(psbt.unsigned_tx.output[1].value, 10_000);
        assert_eq!(psbt.unsigned_tx.output[2].value, 30_000);
    }

    #[test]
    fn test_create_tx_default_sighash() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 30_000);
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(psbt.inputs[0].sighash_type, None);
    }

    #[test]
    fn test_create_tx_custom_sighash() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 30_000)
            .sighash(bitcoin::EcdsaSighashType::Single.into());
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(
            psbt.inputs[0].sighash_type,
            Some(bitcoin::EcdsaSighashType::Single.into())
        );
    }

    #[test]
    fn test_create_tx_input_hd_keypaths() {
        use bitcoin::util::bip32::{DerivationPath, Fingerprint};
        use std::str::FromStr;

        let (wallet, _, _) = get_funded_wallet("wpkh([d34db33f/44'/0'/0']tpubDEnoLuPdBep9bzw5LoGYpsxUQYheRQ9gcgrJhJEcdKFB9cWQRyYmkCyRoTqeD4tJYiVVgt6A3rN6rWn9RYhR9sBsGxji29LYWHuKKbdb1ev/0/*)");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(psbt.inputs[0].bip32_derivation.len(), 1);
        assert_eq!(
            psbt.inputs[0].bip32_derivation.values().next().unwrap(),
            &(
                Fingerprint::from_str("d34db33f").unwrap(),
                DerivationPath::from_str("m/44'/0'/0'/0/0").unwrap()
            )
        );
    }

    #[test]
    fn test_create_tx_output_hd_keypaths() {
        use bitcoin::util::bip32::{DerivationPath, Fingerprint};
        use std::str::FromStr;

        let (wallet, descriptors, _) = get_funded_wallet("wpkh([d34db33f/44'/0'/0']tpubDEnoLuPdBep9bzw5LoGYpsxUQYheRQ9gcgrJhJEcdKFB9cWQRyYmkCyRoTqeD4tJYiVVgt6A3rN6rWn9RYhR9sBsGxji29LYWHuKKbdb1ev/0/*)");
        // cache some addresses
        wallet.get_address(New).unwrap();

        let addr = testutils!(@external descriptors, 5);
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(psbt.outputs[0].bip32_derivation.len(), 1);
        assert_eq!(
            psbt.outputs[0].bip32_derivation.values().next().unwrap(),
            &(
                Fingerprint::from_str("d34db33f").unwrap(),
                DerivationPath::from_str("m/44'/0'/0'/0/5").unwrap()
            )
        );
    }

    #[test]
    fn test_create_tx_set_redeem_script_p2sh() {
        use bitcoin::hashes::hex::FromHex;

        let (wallet, _, _) =
            get_funded_wallet("sh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(
            psbt.inputs[0].redeem_script,
            Some(Script::from(
                Vec::<u8>::from_hex(
                    "21032b0558078bec38694a84933d659303e2575dae7e91685911454115bfd64487e3ac"
                )
                .unwrap()
            ))
        );
        assert_eq!(psbt.inputs[0].witness_script, None);
    }

    #[test]
    fn test_create_tx_set_witness_script_p2wsh() {
        use bitcoin::hashes::hex::FromHex;

        let (wallet, _, _) =
            get_funded_wallet("wsh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(psbt.inputs[0].redeem_script, None);
        assert_eq!(
            psbt.inputs[0].witness_script,
            Some(Script::from(
                Vec::<u8>::from_hex(
                    "21032b0558078bec38694a84933d659303e2575dae7e91685911454115bfd64487e3ac"
                )
                .unwrap()
            ))
        );
    }

    #[test]
    fn test_create_tx_set_redeem_witness_script_p2wsh_p2sh() {
        use bitcoin::hashes::hex::FromHex;

        let (wallet, _, _) =
            get_funded_wallet("sh(wsh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)))");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (psbt, _) = builder.finish().unwrap();

        let script = Script::from(
            Vec::<u8>::from_hex(
                "21032b0558078bec38694a84933d659303e2575dae7e91685911454115bfd64487e3ac",
            )
            .unwrap(),
        );

        assert_eq!(psbt.inputs[0].redeem_script, Some(script.to_v0_p2wsh()));
        assert_eq!(psbt.inputs[0].witness_script, Some(script));
    }

    #[test]
    fn test_create_tx_non_witness_utxo() {
        let (wallet, _, _) =
            get_funded_wallet("sh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (psbt, _) = builder.finish().unwrap();

        assert!(psbt.inputs[0].non_witness_utxo.is_some());
        assert!(psbt.inputs[0].witness_utxo.is_none());
    }

    #[test]
    fn test_create_tx_only_witness_utxo() {
        let (wallet, _, _) =
            get_funded_wallet("wsh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .drain_to(addr.script_pubkey())
            .only_witness_utxo()
            .drain_wallet();
        let (psbt, _) = builder.finish().unwrap();

        assert!(psbt.inputs[0].non_witness_utxo.is_none());
        assert!(psbt.inputs[0].witness_utxo.is_some());
    }

    #[test]
    fn test_create_tx_shwpkh_has_witness_utxo() {
        let (wallet, _, _) =
            get_funded_wallet("sh(wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (psbt, _) = builder.finish().unwrap();

        assert!(psbt.inputs[0].witness_utxo.is_some());
    }

    #[test]
    fn test_create_tx_both_non_witness_utxo_and_witness_utxo_default() {
        let (wallet, _, _) =
            get_funded_wallet("wsh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (psbt, _) = builder.finish().unwrap();

        assert!(psbt.inputs[0].non_witness_utxo.is_some());
        assert!(psbt.inputs[0].witness_utxo.is_some());
    }

    #[test]
    fn test_create_tx_add_utxo() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        let small_output_txid = crate::populate_test_db!(
            wallet.database.borrow_mut(),
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 30_000)
            .add_utxo(OutPoint {
                txid: small_output_txid,
                vout: 0,
            })
            .unwrap();
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(
            psbt.unsigned_tx.input.len(),
            2,
            "should add an additional input since 25_000 < 30_000"
        );
        assert_eq!(details.sent, 75_000, "total should be sum of both inputs");
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_create_tx_manually_selected_insufficient() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        let small_output_txid = crate::populate_test_db!(
            wallet.database.borrow_mut(),
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 30_000)
            .add_utxo(OutPoint {
                txid: small_output_txid,
                vout: 0,
            })
            .unwrap()
            .manually_selected_only();
        builder.finish().unwrap();
    }

    #[test]
    #[should_panic(expected = "SpendingPolicyRequired(External)")]
    fn test_create_tx_policy_path_required() {
        let (wallet, _, _) = get_funded_wallet(get_test_a_or_b_plus_csv());

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 30_000);
        builder.finish().unwrap();
    }

    #[test]
    fn test_create_tx_policy_path_no_csv() {
        let descriptors = testutils!(@descriptors (get_test_wpkh()));
        let wallet = Wallet::new(
            &descriptors.0,
            None,
            Network::Regtest,
            AnyDatabase::Memory(MemoryDatabase::new()),
        )
        .unwrap();

        let tx_meta = testutils! {
                @tx ( (@external descriptors, 0) => 50_000 )
        };

        // Add the transaction to our db, but do not sync the db. Unsynced db
        // should trigger the default sequence value for a new transaction as 0xFFFFFFFF
        crate::populate_test_db!(wallet.database.borrow_mut(), tx_meta, None);

        let external_policy = wallet.policies(KeychainKind::External).unwrap().unwrap();
        let root_id = external_policy.id;
        // child #0 is just the key "A"
        let path = vec![(root_id, vec![0])].into_iter().collect();

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 30_000)
            .policy_path(path, KeychainKind::External);
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(0xFFFFFFFF));
    }

    #[test]
    fn test_create_tx_policy_path_use_csv() {
        let (wallet, _, _) = get_funded_wallet(get_test_a_or_b_plus_csv());

        let external_policy = wallet.policies(KeychainKind::External).unwrap().unwrap();
        let root_id = external_policy.id;
        // child #1 is or(pk(B),older(144))
        let path = vec![(root_id, vec![1])].into_iter().collect();

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 30_000)
            .policy_path(path, KeychainKind::External);
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(psbt.unsigned_tx.input[0].sequence, Sequence(144));
    }

    #[test]
    fn test_create_tx_global_xpubs_with_origin() {
        use bitcoin::hashes::hex::FromHex;
        use bitcoin::util::bip32;

        let (wallet, _, _) = get_funded_wallet("wpkh([73756c7f/48'/0'/0'/2']tpubDCKxNyM3bLgbEX13Mcd8mYxbVg9ajDkWXMh29hMWBurKfVmBfWAM96QVP3zaUcN51HvkZ3ar4VwP82kC8JZhhux8vFQoJintSpVBwpFvyU3/0/*)");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .add_global_xpubs();
        let (psbt, _) = builder.finish().unwrap();

        let key = bip32::ExtendedPubKey::from_str("tpubDCKxNyM3bLgbEX13Mcd8mYxbVg9ajDkWXMh29hMWBurKfVmBfWAM96QVP3zaUcN51HvkZ3ar4VwP82kC8JZhhux8vFQoJintSpVBwpFvyU3").unwrap();
        let fingerprint = bip32::Fingerprint::from_hex("73756c7f").unwrap();
        let path = bip32::DerivationPath::from_str("m/48'/0'/0'/2'").unwrap();

        assert_eq!(psbt.xpub.len(), 1);
        assert_eq!(psbt.xpub.get(&key), Some(&(fingerprint, path)));
    }

    #[test]
    fn test_add_foreign_utxo() {
        let (wallet1, _, _) = get_funded_wallet(get_test_wpkh());
        let (wallet2, _, _) =
            get_funded_wallet("wpkh(cVbZ8ovhye9AoAHFsqobCf7LxbXDAECy9Kb8TZdfsDYMZGBUyCnm)");

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let utxo = wallet2.list_unspent().unwrap().remove(0);
        let foreign_utxo_satisfaction = wallet2
            .get_descriptor_for_keychain(KeychainKind::External)
            .max_satisfaction_weight()
            .unwrap();

        let psbt_input = psbt::Input {
            witness_utxo: Some(utxo.txout.clone()),
            ..Default::default()
        };

        let mut builder = wallet1.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 60_000)
            .only_witness_utxo()
            .add_foreign_utxo(utxo.outpoint, psbt_input, foreign_utxo_satisfaction)
            .unwrap();
        let (mut psbt, details) = builder.finish().unwrap();

        assert_eq!(
            details.sent - details.received,
            10_000 + details.fee.unwrap_or(0),
            "we should have only net spent ~10_000"
        );

        assert!(
            psbt.unsigned_tx
                .input
                .iter()
                .any(|input| input.previous_output == utxo.outpoint),
            "foreign_utxo should be in there"
        );

        let finished = wallet1
            .sign(
                &mut psbt,
                SignOptions {
                    trust_witness_utxo: true,
                    ..Default::default()
                },
            )
            .unwrap();

        assert!(
            !finished,
            "only one of the inputs should have been signed so far"
        );

        let finished = wallet2
            .sign(
                &mut psbt,
                SignOptions {
                    trust_witness_utxo: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(finished, "all the inputs should have been signed now");
    }

    #[test]
    #[should_panic(expected = "Generic(\"Foreign utxo missing witness_utxo or non_witness_utxo\")")]
    fn test_add_foreign_utxo_invalid_psbt_input() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let mut builder = wallet.build_tx();
        let outpoint = wallet.list_unspent().unwrap()[0].outpoint;
        let foreign_utxo_satisfaction = wallet
            .get_descriptor_for_keychain(KeychainKind::External)
            .max_satisfaction_weight()
            .unwrap();
        builder
            .add_foreign_utxo(outpoint, psbt::Input::default(), foreign_utxo_satisfaction)
            .unwrap();
    }

    #[test]
    fn test_add_foreign_utxo_where_outpoint_doesnt_match_psbt_input() {
        let (wallet1, _, txid1) = get_funded_wallet(get_test_wpkh());
        let (wallet2, _, txid2) =
            get_funded_wallet("wpkh(cVbZ8ovhye9AoAHFsqobCf7LxbXDAECy9Kb8TZdfsDYMZGBUyCnm)");

        let utxo2 = wallet2.list_unspent().unwrap().remove(0);
        let tx1 = wallet1
            .database
            .borrow()
            .get_tx(&txid1, true)
            .unwrap()
            .unwrap()
            .transaction
            .unwrap();
        let tx2 = wallet2
            .database
            .borrow()
            .get_tx(&txid2, true)
            .unwrap()
            .unwrap()
            .transaction
            .unwrap();

        let satisfaction_weight = wallet2
            .get_descriptor_for_keychain(KeychainKind::External)
            .max_satisfaction_weight()
            .unwrap();

        let mut builder = wallet1.build_tx();
        assert!(
            builder
                .add_foreign_utxo(
                    utxo2.outpoint,
                    psbt::Input {
                        non_witness_utxo: Some(tx1),
                        ..Default::default()
                    },
                    satisfaction_weight
                )
                .is_err(),
            "should fail when outpoint doesn't match psbt_input"
        );
        assert!(
            builder
                .add_foreign_utxo(
                    utxo2.outpoint,
                    psbt::Input {
                        non_witness_utxo: Some(tx2),
                        ..Default::default()
                    },
                    satisfaction_weight
                )
                .is_ok(),
            "shoulld be ok when outpoint does match psbt_input"
        );
    }

    #[test]
    fn test_add_foreign_utxo_only_witness_utxo() {
        let (wallet1, _, _) = get_funded_wallet(get_test_wpkh());
        let (wallet2, _, txid2) =
            get_funded_wallet("wpkh(cVbZ8ovhye9AoAHFsqobCf7LxbXDAECy9Kb8TZdfsDYMZGBUyCnm)");
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let utxo2 = wallet2.list_unspent().unwrap().remove(0);

        let satisfaction_weight = wallet2
            .get_descriptor_for_keychain(KeychainKind::External)
            .max_satisfaction_weight()
            .unwrap();

        let mut builder = wallet1.build_tx();
        builder.add_recipient(addr.script_pubkey(), 60_000);

        {
            let mut builder = builder.clone();
            let psbt_input = psbt::Input {
                witness_utxo: Some(utxo2.txout.clone()),
                ..Default::default()
            };
            builder
                .add_foreign_utxo(utxo2.outpoint, psbt_input, satisfaction_weight)
                .unwrap();
            assert!(
                builder.finish().is_err(),
                "psbt_input with witness_utxo should fail with only witness_utxo"
            );
        }

        {
            let mut builder = builder.clone();
            let psbt_input = psbt::Input {
                witness_utxo: Some(utxo2.txout.clone()),
                ..Default::default()
            };
            builder
                .only_witness_utxo()
                .add_foreign_utxo(utxo2.outpoint, psbt_input, satisfaction_weight)
                .unwrap();
            assert!(
                builder.finish().is_ok(),
                "psbt_input with just witness_utxo should succeed when `only_witness_utxo` is enabled"
            );
        }

        {
            let mut builder = builder.clone();
            let tx2 = wallet2
                .database
                .borrow()
                .get_tx(&txid2, true)
                .unwrap()
                .unwrap()
                .transaction
                .unwrap();
            let psbt_input = psbt::Input {
                non_witness_utxo: Some(tx2),
                ..Default::default()
            };
            builder
                .add_foreign_utxo(utxo2.outpoint, psbt_input, satisfaction_weight)
                .unwrap();
            assert!(
                builder.finish().is_ok(),
                "psbt_input with non_witness_utxo should succeed by default"
            );
        }
    }

    #[test]
    fn test_get_psbt_input() {
        // this should grab a known good utxo and set the input
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        for utxo in wallet.list_unspent().unwrap() {
            let psbt_input = wallet.get_psbt_input(utxo, None, false).unwrap();
            assert!(psbt_input.witness_utxo.is_some() || psbt_input.non_witness_utxo.is_some());
        }
    }

    #[test]
    #[should_panic(
        expected = "MissingKeyOrigin(\"tpubDCKxNyM3bLgbEX13Mcd8mYxbVg9ajDkWXMh29hMWBurKfVmBfWAM96QVP3zaUcN51HvkZ3ar4VwP82kC8JZhhux8vFQoJintSpVBwpFvyU3\")"
    )]
    fn test_create_tx_global_xpubs_origin_missing() {
        let (wallet, _, _) = get_funded_wallet("wpkh(tpubDCKxNyM3bLgbEX13Mcd8mYxbVg9ajDkWXMh29hMWBurKfVmBfWAM96QVP3zaUcN51HvkZ3ar4VwP82kC8JZhhux8vFQoJintSpVBwpFvyU3/0/*)");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .add_global_xpubs();
        builder.finish().unwrap();
    }

    #[test]
    fn test_create_tx_global_xpubs_master_without_origin() {
        use bitcoin::hashes::hex::FromHex;
        use bitcoin::util::bip32;

        let (wallet, _, _) = get_funded_wallet("wpkh(tpubD6NzVbkrYhZ4Y55A58Gv9RSNF5hy84b5AJqYy7sCcjFrkcLpPre8kmgfit6kY1Zs3BLgeypTDBZJM222guPpdz7Cup5yzaMu62u7mYGbwFL/0/*)");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .add_global_xpubs();
        let (psbt, _) = builder.finish().unwrap();

        let key = bip32::ExtendedPubKey::from_str("tpubD6NzVbkrYhZ4Y55A58Gv9RSNF5hy84b5AJqYy7sCcjFrkcLpPre8kmgfit6kY1Zs3BLgeypTDBZJM222guPpdz7Cup5yzaMu62u7mYGbwFL").unwrap();
        let fingerprint = bip32::Fingerprint::from_hex("997a323b").unwrap();

        assert_eq!(psbt.xpub.len(), 1);
        assert_eq!(
            psbt.xpub.get(&key),
            Some(&(fingerprint, bip32::DerivationPath::default()))
        );
    }

    #[test]
    #[should_panic(expected = "IrreplaceableTransaction")]
    fn test_bump_fee_irreplaceable_tx() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let (psbt, mut details) = builder.finish().unwrap();

        let tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the utxos, we know they can't be used anyways
        details.transaction = Some(tx);
        wallet.database.borrow_mut().set_tx(&details).unwrap();

        wallet.build_fee_bump(txid).unwrap().finish().unwrap();
    }

    #[test]
    #[should_panic(expected = "TransactionConfirmed")]
    fn test_bump_fee_confirmed_tx() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let (psbt, mut details) = builder.finish().unwrap();

        let tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the utxos, we know they can't be used anyways
        details.transaction = Some(tx);
        details.confirmation_time = Some(BlockTime {
            timestamp: 12345678,
            height: 42,
        });
        wallet.database.borrow_mut().set_tx(&details).unwrap();

        wallet.build_fee_bump(txid).unwrap().finish().unwrap();
    }

    #[test]
    #[should_panic(expected = "FeeRateTooLow")]
    fn test_bump_fee_low_fee_rate() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .enable_rbf();
        let (psbt, mut details) = builder.finish().unwrap();

        let tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the utxos, we know they can't be used anyways
        details.transaction = Some(tx);
        wallet.database.borrow_mut().set_tx(&details).unwrap();

        let mut builder = wallet.build_fee_bump(txid).unwrap();
        builder.fee_rate(FeeRate::from_sat_per_vb(1.0));
        builder.finish().unwrap();
    }

    #[test]
    #[should_panic(expected = "FeeTooLow")]
    fn test_bump_fee_low_abs() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .enable_rbf();
        let (psbt, mut details) = builder.finish().unwrap();

        let tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the utxos, we know they can't be used anyways
        details.transaction = Some(tx);
        wallet.database.borrow_mut().set_tx(&details).unwrap();

        let mut builder = wallet.build_fee_bump(txid).unwrap();
        builder.fee_absolute(10);
        builder.finish().unwrap();
    }

    #[test]
    #[should_panic(expected = "FeeTooLow")]
    fn test_bump_fee_zero_abs() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .enable_rbf();
        let (psbt, mut details) = builder.finish().unwrap();

        let tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the utxos, we know they can't be used anyways
        details.transaction = Some(tx);
        wallet.database.borrow_mut().set_tx(&details).unwrap();

        let mut builder = wallet.build_fee_bump(txid).unwrap();
        builder.fee_absolute(0);
        builder.finish().unwrap();
    }

    #[test]
    fn test_bump_fee_reduce_change() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .enable_rbf();
        let (psbt, mut original_details) = builder.finish().unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the new utxos, we know they can't be used anyways
        for txin in &mut tx.input {
            txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        let mut builder = wallet.build_fee_bump(txid).unwrap();
        builder.fee_rate(FeeRate::from_sat_per_vb(2.5)).enable_rbf();
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(details.sent, original_details.sent);
        assert_eq!(
            details.received + details.fee.unwrap_or(0),
            original_details.received + original_details.fee.unwrap_or(0)
        );
        assert!(details.fee.unwrap_or(0) > original_details.fee.unwrap_or(0));

        let tx = &psbt.unsigned_tx;
        assert_eq!(tx.output.len(), 2);
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey == addr.script_pubkey())
                .unwrap()
                .value,
            25_000
        );
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey != addr.script_pubkey())
                .unwrap()
                .value,
            details.received
        );

        assert_fee_rate!(psbt, details.fee.unwrap_or(0), FeeRate::from_sat_per_vb(2.5), @add_signature);
    }

    #[test]
    fn test_bump_fee_absolute_reduce_change() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .enable_rbf();
        let (psbt, mut original_details) = builder.finish().unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the new utxos, we know they can't be used anyways
        for txin in &mut tx.input {
            txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        let mut builder = wallet.build_fee_bump(txid).unwrap();
        builder.fee_absolute(200);
        builder.enable_rbf();
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(details.sent, original_details.sent);
        assert_eq!(
            details.received + details.fee.unwrap_or(0),
            original_details.received + original_details.fee.unwrap_or(0)
        );
        assert!(
            details.fee.unwrap_or(0) > original_details.fee.unwrap_or(0),
            "{} > {}",
            details.fee.unwrap_or(0),
            original_details.fee.unwrap_or(0)
        );

        let tx = &psbt.unsigned_tx;
        assert_eq!(tx.output.len(), 2);
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey == addr.script_pubkey())
                .unwrap()
                .value,
            25_000
        );
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey != addr.script_pubkey())
                .unwrap()
                .value,
            details.received
        );

        assert_eq!(details.fee.unwrap_or(0), 200);
    }

    #[test]
    fn test_bump_fee_reduce_single_recipient() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .drain_to(addr.script_pubkey())
            .drain_wallet()
            .enable_rbf();
        let (psbt, mut original_details) = builder.finish().unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        for txin in &mut tx.input {
            txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        let mut builder = wallet.build_fee_bump(txid).unwrap();
        builder
            .fee_rate(FeeRate::from_sat_per_vb(2.5))
            .allow_shrinking(addr.script_pubkey())
            .unwrap();
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(details.sent, original_details.sent);
        assert!(details.fee.unwrap_or(0) > original_details.fee.unwrap_or(0));

        let tx = &psbt.unsigned_tx;
        assert_eq!(tx.output.len(), 1);
        assert_eq!(tx.output[0].value + details.fee.unwrap_or(0), details.sent);

        assert_fee_rate!(psbt, details.fee.unwrap_or(0), FeeRate::from_sat_per_vb(2.5), @add_signature);
    }

    #[test]
    fn test_bump_fee_absolute_reduce_single_recipient() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .drain_to(addr.script_pubkey())
            .drain_wallet()
            .enable_rbf();
        let (psbt, mut original_details) = builder.finish().unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        for txin in &mut tx.input {
            txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        let mut builder = wallet.build_fee_bump(txid).unwrap();
        builder
            .allow_shrinking(addr.script_pubkey())
            .unwrap()
            .fee_absolute(300);
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(details.sent, original_details.sent);
        assert!(details.fee.unwrap_or(0) > original_details.fee.unwrap_or(0));

        let tx = &psbt.unsigned_tx;
        assert_eq!(tx.output.len(), 1);
        assert_eq!(tx.output[0].value + details.fee.unwrap_or(0), details.sent);

        assert_eq!(details.fee.unwrap_or(0), 300);
    }

    #[test]
    fn test_bump_fee_drain_wallet() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        // receive an extra tx so that our wallet has two utxos.
        let incoming_txid = crate::populate_test_db!(
            wallet.database.borrow_mut(),
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );
        let outpoint = OutPoint {
            txid: incoming_txid,
            vout: 0,
        };
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .drain_to(addr.script_pubkey())
            .add_utxo(outpoint)
            .unwrap()
            .manually_selected_only()
            .enable_rbf();
        let (psbt, mut original_details) = builder.finish().unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        for txin in &mut tx.input {
            txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();
        assert_eq!(original_details.sent, 25_000);

        // for the new feerate, it should be enough to reduce the output, but since we specify
        // `drain_wallet` we expect to spend everything
        let mut builder = wallet.build_fee_bump(txid).unwrap();
        builder
            .drain_wallet()
            .allow_shrinking(addr.script_pubkey())
            .unwrap()
            .fee_rate(FeeRate::from_sat_per_vb(5.0));
        let (_, details) = builder.finish().unwrap();
        assert_eq!(details.sent, 75_000);
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_bump_fee_remove_output_manually_selected_only() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        // receive an extra tx so that our wallet has two utxos. then we manually pick only one of
        // them, and make sure that `bump_fee` doesn't try to add more. This fails because we've
        // told the wallet it's not allowed to add more inputs AND it can't reduce the value of the
        // existing output. In other words, bump_fee + manually_selected_only is always an error
        // unless you've also set "allow_shrinking" OR there is a change output.
        let incoming_txid = crate::populate_test_db!(
            wallet.database.borrow_mut(),
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );
        let outpoint = OutPoint {
            txid: incoming_txid,
            vout: 0,
        };
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .drain_to(addr.script_pubkey())
            .add_utxo(outpoint)
            .unwrap()
            .manually_selected_only()
            .enable_rbf();
        let (psbt, mut original_details) = builder.finish().unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        for txin in &mut tx.input {
            txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();
        assert_eq!(original_details.sent, 25_000);

        let mut builder = wallet.build_fee_bump(txid).unwrap();
        builder
            .manually_selected_only()
            .fee_rate(FeeRate::from_sat_per_vb(255.0));
        builder.finish().unwrap();
    }

    #[test]
    fn test_bump_fee_add_input() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        crate::populate_test_db!(
            wallet.database.borrow_mut(),
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 45_000)
            .enable_rbf();
        let (psbt, mut original_details) = builder.finish().unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the new utxos, we know they can't be used anyways
        for txin in &mut tx.input {
            txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        let mut builder = wallet.build_fee_bump(txid).unwrap();
        builder.fee_rate(FeeRate::from_sat_per_vb(50.0));
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(details.sent, original_details.sent + 25_000);
        assert_eq!(details.fee.unwrap_or(0) + details.received, 30_000);

        let tx = &psbt.unsigned_tx;
        assert_eq!(tx.input.len(), 2);
        assert_eq!(tx.output.len(), 2);
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey == addr.script_pubkey())
                .unwrap()
                .value,
            45_000
        );
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey != addr.script_pubkey())
                .unwrap()
                .value,
            details.received
        );

        assert_fee_rate!(psbt, details.fee.unwrap_or(0), FeeRate::from_sat_per_vb(50.0), @add_signature);
    }

    #[test]
    fn test_bump_fee_absolute_add_input() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        crate::populate_test_db!(
            wallet.database.borrow_mut(),
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 45_000)
            .enable_rbf();
        let (psbt, mut original_details) = builder.finish().unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the new utxos, we know they can't be used anyways
        for txin in &mut tx.input {
            txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        let mut builder = wallet.build_fee_bump(txid).unwrap();
        builder.fee_absolute(6_000);
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(details.sent, original_details.sent + 25_000);
        assert_eq!(details.fee.unwrap_or(0) + details.received, 30_000);

        let tx = &psbt.unsigned_tx;
        assert_eq!(tx.input.len(), 2);
        assert_eq!(tx.output.len(), 2);
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey == addr.script_pubkey())
                .unwrap()
                .value,
            45_000
        );
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey != addr.script_pubkey())
                .unwrap()
                .value,
            details.received
        );

        assert_eq!(details.fee.unwrap_or(0), 6_000);
    }

    #[test]
    fn test_bump_fee_no_change_add_input_and_change() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        let incoming_txid = crate::populate_test_db!(
            wallet.database.borrow_mut(),
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        // initially make a tx without change by using `drain_to`
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .drain_to(addr.script_pubkey())
            .add_utxo(OutPoint {
                txid: incoming_txid,
                vout: 0,
            })
            .unwrap()
            .manually_selected_only()
            .enable_rbf();
        let (psbt, mut original_details) = builder.finish().unwrap();

        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the new utxos, we know they can't be used anyways
        for txin in &mut tx.input {
            txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        // now bump the fees without using `allow_shrinking`. the wallet should add an
        // extra input and a change output, and leave the original output untouched
        let mut builder = wallet.build_fee_bump(txid).unwrap();
        builder.fee_rate(FeeRate::from_sat_per_vb(50.0));
        let (psbt, details) = builder.finish().unwrap();

        let original_send_all_amount = original_details.sent - original_details.fee.unwrap_or(0);
        assert_eq!(details.sent, original_details.sent + 50_000);
        assert_eq!(
            details.received,
            75_000 - original_send_all_amount - details.fee.unwrap_or(0)
        );

        let tx = &psbt.unsigned_tx;
        assert_eq!(tx.input.len(), 2);
        assert_eq!(tx.output.len(), 2);
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey == addr.script_pubkey())
                .unwrap()
                .value,
            original_send_all_amount
        );
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey != addr.script_pubkey())
                .unwrap()
                .value,
            75_000 - original_send_all_amount - details.fee.unwrap_or(0)
        );

        assert_fee_rate!(psbt, details.fee.unwrap_or(0), FeeRate::from_sat_per_vb(50.0), @add_signature);
    }

    #[test]
    fn test_bump_fee_add_input_change_dust() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        crate::populate_test_db!(
            wallet.database.borrow_mut(),
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 45_000)
            .enable_rbf();
        let (psbt, mut original_details) = builder.finish().unwrap();
        let mut tx = psbt.extract_tx();
        assert_eq!(tx.input.len(), 1);
        assert_eq!(tx.output.len(), 2);
        let txid = tx.txid();
        // skip saving the new utxos, we know they can't be used anyways
        for txin in &mut tx.input {
            txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        let original_tx_weight = tx.weight();
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        let mut builder = wallet.build_fee_bump(txid).unwrap();
        // We set a fee high enough that during rbf we are forced to add
        // a new input and also that we have to remove the change
        // that we had previously

        // We calculate the new weight as:
        //   original weight
        // + extra input weight: 160 WU = (32 (prevout) + 4 (vout) + 4 (nsequence)) * 4
        // + input satisfaction weight: 112 WU = 106 (witness) + 2 (witness len) + (1 (script len)) * 4
        // - change output weight: 124 WU = (8 (value) + 1 (script len) + 22 (script)) * 4
        let new_tx_weight = original_tx_weight + 160 + 112 - 124;
        // two inputs (50k, 25k) and one output (45k) - epsilon
        // We use epsilon here to avoid asking for a slightly too high feerate
        let fee_abs = 50_000 + 25_000 - 45_000 - 10;
        builder.fee_rate(FeeRate::from_wu(fee_abs, new_tx_weight));
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(
            original_details.received,
            5_000 - original_details.fee.unwrap_or(0)
        );

        assert_eq!(details.sent, original_details.sent + 25_000);
        assert_eq!(details.fee.unwrap_or(0), 30_000);
        assert_eq!(details.received, 0);

        let tx = &psbt.unsigned_tx;
        assert_eq!(tx.input.len(), 2);
        assert_eq!(tx.output.len(), 1);
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey == addr.script_pubkey())
                .unwrap()
                .value,
            45_000
        );

        assert_fee_rate!(psbt, details.fee.unwrap_or(0), FeeRate::from_sat_per_vb(140.0), @dust_change, @add_signature);
    }

    #[test]
    fn test_bump_fee_force_add_input() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        let incoming_txid = crate::populate_test_db!(
            wallet.database.borrow_mut(),
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 45_000)
            .enable_rbf();
        let (psbt, mut original_details) = builder.finish().unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the new utxos, we know they can't be used anyways
        for txin in &mut tx.input {
            txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        // the new fee_rate is low enough that just reducing the change would be fine, but we force
        // the addition of an extra input with `add_utxo()`
        let mut builder = wallet.build_fee_bump(txid).unwrap();
        builder
            .add_utxo(OutPoint {
                txid: incoming_txid,
                vout: 0,
            })
            .unwrap()
            .fee_rate(FeeRate::from_sat_per_vb(5.0));
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(details.sent, original_details.sent + 25_000);
        assert_eq!(details.fee.unwrap_or(0) + details.received, 30_000);

        let tx = &psbt.unsigned_tx;
        assert_eq!(tx.input.len(), 2);
        assert_eq!(tx.output.len(), 2);
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey == addr.script_pubkey())
                .unwrap()
                .value,
            45_000
        );
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey != addr.script_pubkey())
                .unwrap()
                .value,
            details.received
        );

        assert_fee_rate!(psbt, details.fee.unwrap_or(0), FeeRate::from_sat_per_vb(5.0), @add_signature);
    }

    #[test]
    fn test_bump_fee_absolute_force_add_input() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        let incoming_txid = crate::populate_test_db!(
            wallet.database.borrow_mut(),
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 45_000)
            .enable_rbf();
        let (psbt, mut original_details) = builder.finish().unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the new utxos, we know they can't be used anyways
        for txin in &mut tx.input {
            txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        // the new fee_rate is low enough that just reducing the change would be fine, but we force
        // the addition of an extra input with `add_utxo()`
        let mut builder = wallet.build_fee_bump(txid).unwrap();
        builder
            .add_utxo(OutPoint {
                txid: incoming_txid,
                vout: 0,
            })
            .unwrap()
            .fee_absolute(250);
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(details.sent, original_details.sent + 25_000);
        assert_eq!(details.fee.unwrap_or(0) + details.received, 30_000);

        let tx = &psbt.unsigned_tx;
        assert_eq!(tx.input.len(), 2);
        assert_eq!(tx.output.len(), 2);
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey == addr.script_pubkey())
                .unwrap()
                .value,
            45_000
        );
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey != addr.script_pubkey())
                .unwrap()
                .value,
            details.received
        );

        assert_eq!(details.fee.unwrap_or(0), 250);
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_bump_fee_unconfirmed_inputs_only() {
        // We try to bump the fee, but:
        // - We can't reduce the change, as we have no change
        // - All our UTXOs are unconfirmed
        // So, we fail with "InsufficientFunds", as per RBF rule 2:
        // The replacement transaction may only include an unconfirmed input
        // if that input was included in one of the original transactions.
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .drain_wallet()
            .drain_to(addr.script_pubkey())
            .enable_rbf();
        let (psbt, mut original_details) = builder.finish().unwrap();
        // Now we receive one transaction with 0 confirmations. We won't be able to use that for
        // fee bumping, as it's still unconfirmed!
        crate::populate_test_db!(
            wallet.database.borrow_mut(),
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 0)),
            Some(100),
        );
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        for txin in &mut tx.input {
            txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        let mut builder = wallet.build_fee_bump(txid).unwrap();
        builder.fee_rate(FeeRate::from_sat_per_vb(25.0));
        builder.finish().unwrap();
    }

    #[test]
    fn test_bump_fee_unconfirmed_input() {
        // We create a tx draining the wallet and spending one confirmed
        // and one unconfirmed UTXO. We check that we can fee bump normally
        // (BIP125 rule 2 only apply to newly added unconfirmed input, you can
        // always fee bump with an unconfirmed input if it was included in the
        // original transaction)
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        // We receive a tx with 0 confirmations, which will be used as an input
        // in the drain tx.
        crate::populate_test_db!(
            wallet.database.borrow_mut(),
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 0)),
            Some(100),
        );
        let mut builder = wallet.build_tx();
        builder
            .drain_wallet()
            .drain_to(addr.script_pubkey())
            .enable_rbf();
        let (psbt, mut original_details) = builder.finish().unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        for txin in &mut tx.input {
            txin.witness.push([0x00; P2WPKH_FAKE_WITNESS_SIZE]); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        let mut builder = wallet.build_fee_bump(txid).unwrap();
        builder
            .fee_rate(FeeRate::from_sat_per_vb(15.0))
            .allow_shrinking(addr.script_pubkey())
            .unwrap();
        builder.finish().unwrap();
    }

    #[test]
    fn test_fee_amount_negative_drain_val() {
        // While building the transaction, bdk would calculate the drain_value
        // as
        // current_delta - fee_amount - drain_fee
        // using saturating_sub, meaning that if the result would end up negative,
        // it'll remain to zero instead.
        // This caused a bug in master where we would calculate the wrong fee
        // for a transaction.
        // See https://github.com/bitcoindevkit/bdk/issues/660
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        let send_to = Address::from_str("tb1ql7w62elx9ucw4pj5lgw4l028hmuw80sndtntxt").unwrap();
        let fee_rate = FeeRate::from_sat_per_vb(2.01);
        let incoming_txid = crate::populate_test_db!(
            wallet.database.borrow_mut(),
            testutils! (@tx ( (@external descriptors, 0) => 8859 ) (@confirmations 1)),
            Some(100),
        );

        let mut builder = wallet.build_tx();
        builder
            .add_recipient(send_to.script_pubkey(), 8630)
            .add_utxo(OutPoint::new(incoming_txid, 0))
            .unwrap()
            .enable_rbf()
            .fee_rate(fee_rate);
        let (psbt, details) = builder.finish().unwrap();

        assert!(psbt.inputs.len() == 1);
        assert_fee_rate!(psbt, details.fee.unwrap_or(0), fee_rate, @add_signature);
    }

    #[test]
    fn test_sign_single_xprv() {
        let (wallet, _, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (mut psbt, _) = builder.finish().unwrap();

        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized);

        let extracted = psbt.extract_tx();
        assert_eq!(extracted.input[0].witness.len(), 2);
    }

    #[test]
    fn test_sign_single_xprv_with_master_fingerprint_and_path() {
        let (wallet, _, _) = get_funded_wallet("wpkh([d34db33f/84h/1h/0h]tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (mut psbt, _) = builder.finish().unwrap();

        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized);

        let extracted = psbt.extract_tx();
        assert_eq!(extracted.input[0].witness.len(), 2);
    }

    #[test]
    fn test_sign_single_xprv_bip44_path() {
        let (wallet, _, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/44'/0'/0'/0/*)");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (mut psbt, _) = builder.finish().unwrap();

        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized);

        let extracted = psbt.extract_tx();
        assert_eq!(extracted.input[0].witness.len(), 2);
    }

    #[test]
    fn test_sign_single_xprv_sh_wpkh() {
        let (wallet, _, _) = get_funded_wallet("sh(wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*))");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (mut psbt, _) = builder.finish().unwrap();

        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized);

        let extracted = psbt.extract_tx();
        assert_eq!(extracted.input[0].witness.len(), 2);
    }

    #[test]
    fn test_sign_single_wif() {
        let (wallet, _, _) =
            get_funded_wallet("wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (mut psbt, _) = builder.finish().unwrap();

        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized);

        let extracted = psbt.extract_tx();
        assert_eq!(extracted.input[0].witness.len(), 2);
    }

    #[test]
    fn test_sign_single_xprv_no_hd_keypaths() {
        let (wallet, _, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (mut psbt, _) = builder.finish().unwrap();

        psbt.inputs[0].bip32_derivation.clear();
        assert_eq!(psbt.inputs[0].bip32_derivation.len(), 0);

        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized);

        let extracted = psbt.extract_tx();
        assert_eq!(extracted.input[0].witness.len(), 2);
    }

    #[test]
    fn test_include_output_redeem_witness_script() {
        let (wallet, _, _) = get_funded_wallet("sh(wsh(multi(1,cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW,cRjo6jqfVNP33HhSS76UhXETZsGTZYx8FMFvR9kpbtCSV1PmdZdu)))");
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 45_000)
            .include_output_redeem_witness_script();
        let (psbt, _) = builder.finish().unwrap();

        // p2sh-p2wsh transaction should contain both witness and redeem scripts
        assert!(psbt
            .outputs
            .iter()
            .any(|output| output.redeem_script.is_some() && output.witness_script.is_some()));
    }

    #[test]
    fn test_signing_only_one_of_multiple_inputs() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 45_000)
            .include_output_redeem_witness_script();
        let (mut psbt, _) = builder.finish().unwrap();

        // add another input to the psbt that is at least passable.
        let dud_input = bitcoin::util::psbt::Input {
            witness_utxo: Some(TxOut {
                value: 100_000,
                script_pubkey: miniscript::Descriptor::<bitcoin::PublicKey>::from_str(
                    "wpkh(025476c2e83188368da1ff3e292e7acafcdb3566bb0ad253f62fc70f07aeee6357)",
                )
                .unwrap()
                .script_pubkey(),
            }),
            ..Default::default()
        };

        psbt.inputs.push(dud_input);
        psbt.unsigned_tx.input.push(bitcoin::TxIn::default());
        let is_final = wallet
            .sign(
                &mut psbt,
                SignOptions {
                    trust_witness_utxo: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            !is_final,
            "shouldn't be final since we can't sign one of the inputs"
        );
        assert!(
            psbt.inputs[0].final_script_witness.is_some(),
            "should finalized input it signed"
        )
    }

    #[test]
    fn test_remove_partial_sigs_after_finalize_sign_option() {
        let (wallet, _, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");

        for remove_partial_sigs in &[true, false] {
            let addr = wallet.get_address(New).unwrap();
            let mut builder = wallet.build_tx();
            builder.drain_to(addr.script_pubkey()).drain_wallet();
            let mut psbt = builder.finish().unwrap().0;

            assert!(wallet
                .sign(
                    &mut psbt,
                    SignOptions {
                        remove_partial_sigs: *remove_partial_sigs,
                        ..Default::default()
                    },
                )
                .unwrap());

            psbt.inputs.iter().for_each(|input| {
                if *remove_partial_sigs {
                    assert!(input.partial_sigs.is_empty())
                } else {
                    assert!(!input.partial_sigs.is_empty())
                }
            });
        }
    }

    #[test]
    fn test_try_finalize_sign_option() {
        let (wallet, _, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");

        for try_finalize in &[true, false] {
            let addr = wallet.get_address(New).unwrap();
            let mut builder = wallet.build_tx();
            builder.drain_to(addr.script_pubkey()).drain_wallet();
            let mut psbt = builder.finish().unwrap().0;

            let finalized = wallet
                .sign(
                    &mut psbt,
                    SignOptions {
                        try_finalize: *try_finalize,
                        ..Default::default()
                    },
                )
                .unwrap();

            psbt.inputs.iter().for_each(|input| {
                if *try_finalize {
                    assert!(finalized);
                    assert!(input.final_script_sig.is_some());
                    assert!(input.final_script_witness.is_some());
                } else {
                    assert!(!finalized);
                    assert!(input.final_script_sig.is_none());
                    assert!(input.final_script_witness.is_none());
                }
            });
        }
    }

    #[test]
    fn test_sign_nonstandard_sighash() {
        let sighash = EcdsaSighashType::NonePlusAnyoneCanPay;

        let (wallet, _, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .drain_to(addr.script_pubkey())
            .sighash(sighash.into())
            .drain_wallet();
        let (mut psbt, _) = builder.finish().unwrap();

        let result = wallet.sign(&mut psbt, Default::default());
        assert!(
            result.is_err(),
            "Signing should have failed because the TX uses non-standard sighashes"
        );
        assert_matches!(
            result,
            Err(Error::Signer(SignerError::NonStandardSighash)),
            "Signing failed with the wrong error type"
        );

        // try again after opting-in
        let result = wallet.sign(
            &mut psbt,
            SignOptions {
                allow_all_sighashes: true,
                ..Default::default()
            },
        );
        assert!(result.is_ok(), "Signing should have worked");
        assert!(
            result.unwrap(),
            "Should finalize the input since we can produce signatures"
        );

        let extracted = psbt.extract_tx();
        assert_eq!(
            *extracted.input[0].witness.to_vec()[0].last().unwrap(),
            sighash.to_u32() as u8,
            "The signature should have been made with the right sighash"
        );
    }

    #[test]
    fn test_unused_address() {
        let db = MemoryDatabase::new();
        let wallet = Wallet::new("wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)",
                                         None, Network::Testnet, db).unwrap();

        assert_eq!(
            wallet.get_address(LastUnused).unwrap().to_string(),
            "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
        );
        assert_eq!(
            wallet.get_address(LastUnused).unwrap().to_string(),
            "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
        );
    }

    #[test]
    fn test_next_unused_address() {
        let descriptor = "wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)";
        let descriptors = testutils!(@descriptors (descriptor));
        let wallet = Wallet::new(
            &descriptors.0,
            None,
            Network::Testnet,
            MemoryDatabase::new(),
        )
        .unwrap();

        assert_eq!(
            wallet.get_address(LastUnused).unwrap().to_string(),
            "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
        );

        // use the above address
        crate::populate_test_db!(
            wallet.database.borrow_mut(),
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        assert_eq!(
            wallet.get_address(LastUnused).unwrap().to_string(),
            "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
        );
    }

    #[test]
    fn test_peek_address_at_index() {
        let db = MemoryDatabase::new();
        let wallet = Wallet::new("wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)",
                                         None, Network::Testnet, db).unwrap();

        assert_eq!(
            wallet.get_address(Peek(1)).unwrap().to_string(),
            "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
        );

        assert_eq!(
            wallet.get_address(Peek(0)).unwrap().to_string(),
            "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
        );

        assert_eq!(
            wallet.get_address(Peek(2)).unwrap().to_string(),
            "tb1qzntf2mqex4ehwkjlfdyy3ewdlk08qkvkvrz7x2"
        );

        // current new address is not affected
        assert_eq!(
            wallet.get_address(New).unwrap().to_string(),
            "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
        );

        assert_eq!(
            wallet.get_address(New).unwrap().to_string(),
            "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
        );
    }

    #[test]
    fn test_peek_address_at_index_not_derivable() {
        let db = MemoryDatabase::new();
        let wallet = Wallet::new("wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/1)",
                                         None, Network::Testnet, db).unwrap();

        assert_eq!(
            wallet.get_address(Peek(1)).unwrap().to_string(),
            "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
        );

        assert_eq!(
            wallet.get_address(Peek(0)).unwrap().to_string(),
            "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
        );

        assert_eq!(
            wallet.get_address(Peek(2)).unwrap().to_string(),
            "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
        );
    }

    #[test]
    fn test_reset_address_index() {
        let db = MemoryDatabase::new();
        let wallet = Wallet::new("wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)",
                                         None, Network::Testnet, db).unwrap();

        // new index 0
        assert_eq!(
            wallet.get_address(New).unwrap().to_string(),
            "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
        );

        // new index 1
        assert_eq!(
            wallet.get_address(New).unwrap().to_string(),
            "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
        );

        // new index 2
        assert_eq!(
            wallet.get_address(New).unwrap().to_string(),
            "tb1qzntf2mqex4ehwkjlfdyy3ewdlk08qkvkvrz7x2"
        );

        //  reset index 1 again
        assert_eq!(
            wallet.get_address(Reset(1)).unwrap().to_string(),
            "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
        );

        // new index 2 again
        assert_eq!(
            wallet.get_address(New).unwrap().to_string(),
            "tb1qzntf2mqex4ehwkjlfdyy3ewdlk08qkvkvrz7x2"
        );
    }

    #[test]
    fn test_returns_index_and_address() {
        let db = MemoryDatabase::new();
        let wallet = Wallet::new("wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)",
                                         None, Network::Testnet, db).unwrap();

        // new index 0
        assert_eq!(
            wallet.get_address(New).unwrap(),
            AddressInfo {
                index: 0,
                address: Address::from_str("tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a").unwrap(),
                keychain: KeychainKind::External,
            }
        );

        // new index 1
        assert_eq!(
            wallet.get_address(New).unwrap(),
            AddressInfo {
                index: 1,
                address: Address::from_str("tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7").unwrap(),
                keychain: KeychainKind::External,
            }
        );

        // peek index 25
        assert_eq!(
            wallet.get_address(Peek(25)).unwrap(),
            AddressInfo {
                index: 25,
                address: Address::from_str("tb1qsp7qu0knx3sl6536dzs0703u2w2ag6ppl9d0c2").unwrap(),
                keychain: KeychainKind::External,
            }
        );

        // new index 2
        assert_eq!(
            wallet.get_address(New).unwrap(),
            AddressInfo {
                index: 2,
                address: Address::from_str("tb1qzntf2mqex4ehwkjlfdyy3ewdlk08qkvkvrz7x2").unwrap(),
                keychain: KeychainKind::External,
            }
        );

        //  reset index 1 again
        assert_eq!(
            wallet.get_address(Reset(1)).unwrap(),
            AddressInfo {
                index: 1,
                address: Address::from_str("tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7").unwrap(),
                keychain: KeychainKind::External,
            }
        );

        // new index 2 again
        assert_eq!(
            wallet.get_address(New).unwrap(),
            AddressInfo {
                index: 2,
                address: Address::from_str("tb1qzntf2mqex4ehwkjlfdyy3ewdlk08qkvkvrz7x2").unwrap(),
                keychain: KeychainKind::External,
            }
        );
    }

    #[test]
    fn test_sending_to_bip350_bech32m_address() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr =
            Address::from_str("tb1pqqqqp399et2xygdj5xreqhjjvcmzhxw4aywxecjdzew6hylgvsesf3hn0c")
                .unwrap();
        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 45_000);
        builder.finish().unwrap();
    }

    #[test]
    fn test_get_address() {
        use crate::descriptor::template::Bip84;
        let key = bitcoin::util::bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();
        let wallet = Wallet::new(
            Bip84(key, KeychainKind::External),
            Some(Bip84(key, KeychainKind::Internal)),
            Network::Regtest,
            MemoryDatabase::default(),
        )
        .unwrap();

        assert_eq!(
            wallet.get_address(AddressIndex::New).unwrap(),
            AddressInfo {
                index: 0,
                address: Address::from_str("bcrt1qrhgaqu0zvf5q2d0gwwz04w0dh0cuehhqvzpp4w").unwrap(),
                keychain: KeychainKind::External,
            }
        );

        assert_eq!(
            wallet.get_internal_address(AddressIndex::New).unwrap(),
            AddressInfo {
                index: 0,
                address: Address::from_str("bcrt1q0ue3s5y935tw7v3gmnh36c5zzsaw4n9c9smq79").unwrap(),
                keychain: KeychainKind::Internal,
            }
        );

        let wallet = Wallet::new(
            Bip84(key, KeychainKind::External),
            None,
            Network::Regtest,
            MemoryDatabase::default(),
        )
        .unwrap();

        assert_eq!(
            wallet.get_internal_address(AddressIndex::New).unwrap(),
            AddressInfo {
                index: 0,
                address: Address::from_str("bcrt1qrhgaqu0zvf5q2d0gwwz04w0dh0cuehhqvzpp4w").unwrap(),
                keychain: KeychainKind::Internal,
            },
            "when there's no internal descriptor it should just use external"
        );
    }

    #[test]
    fn test_get_address_no_reuse_single_descriptor() {
        use crate::descriptor::template::Bip84;
        use std::collections::HashSet;

        let key = bitcoin::util::bip32::ExtendedPrivKey::from_str("tprv8ZgxMBicQKsPcx5nBGsR63Pe8KnRUqmbJNENAfGftF3yuXoMMoVJJcYeUw5eVkm9WBPjWYt6HMWYJNesB5HaNVBaFc1M6dRjWSYnmewUMYy").unwrap();
        let wallet = Wallet::new(
            Bip84(key, KeychainKind::External),
            None,
            Network::Regtest,
            MemoryDatabase::default(),
        )
        .unwrap();

        let mut used_set = HashSet::new();

        (0..3).for_each(|_| {
            let external_addr = wallet.get_address(AddressIndex::New).unwrap().address;
            assert!(used_set.insert(external_addr));

            let internal_addr = wallet
                .get_internal_address(AddressIndex::New)
                .unwrap()
                .address;
            assert!(used_set.insert(internal_addr));
        });
    }

    #[test]
    fn test_taproot_psbt_populate_tap_key_origins() {
        let (wallet, _, _) = get_funded_wallet(get_test_tr_single_sig_xprv());
        let addr = wallet.get_address(AddressIndex::New).unwrap();

        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(
            psbt.inputs[0]
                .tap_key_origins
                .clone()
                .into_iter()
                .collect::<Vec<_>>(),
            vec![(
                from_str!("b96d3a3dc76a4fc74e976511b23aecb78e0754c23c0ed7a6513e18cbbc7178e9"),
                (vec![], (from_str!("f6a5cb8b"), from_str!("m/0")))
            )],
            "Wrong input tap_key_origins"
        );
        assert_eq!(
            psbt.outputs[0]
                .tap_key_origins
                .clone()
                .into_iter()
                .collect::<Vec<_>>(),
            vec![(
                from_str!("e9b03068cf4a2621d4f81e68f6c4216e6bd260fe6edf6acc55c8d8ae5aeff0a8"),
                (vec![], (from_str!("f6a5cb8b"), from_str!("m/1")))
            )],
            "Wrong output tap_key_origins"
        );
    }

    #[test]
    fn test_taproot_psbt_populate_tap_key_origins_repeated_key() {
        let (wallet, _, _) = get_funded_wallet(get_test_tr_repeated_key());
        let addr = wallet.get_address(AddressIndex::New).unwrap();

        let path = vec![("e5mmg3xh".to_string(), vec![0])]
            .into_iter()
            .collect();

        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 25_000)
            .policy_path(path, KeychainKind::External);
        let (psbt, _) = builder.finish().unwrap();

        let mut input_key_origins = psbt.inputs[0]
            .tap_key_origins
            .clone()
            .into_iter()
            .collect::<Vec<_>>();
        input_key_origins.sort();

        assert_eq!(
            input_key_origins,
            vec![
                (
                    from_str!("b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55"),
                    (
                        vec![],
                        (FromStr::from_str("871fd295").unwrap(), vec![].into())
                    )
                ),
                (
                    from_str!("2b0558078bec38694a84933d659303e2575dae7e91685911454115bfd64487e3"),
                    (
                        vec![
                            from_str!(
                                "858ad7a7d7f270e2c490c4d6ba00c499e46b18fdd59ea3c2c47d20347110271e"
                            ),
                            from_str!(
                                "f6e927ad4492c051fe325894a4f5f14538333b55a35f099876be42009ec8f903"
                            ),
                        ],
                        (FromStr::from_str("ece52657").unwrap(), vec![].into())
                    )
                )
            ],
            "Wrong input tap_key_origins"
        );

        let mut output_key_origins = psbt.outputs[0]
            .tap_key_origins
            .clone()
            .into_iter()
            .collect::<Vec<_>>();
        output_key_origins.sort();

        assert_eq!(
            input_key_origins, output_key_origins,
            "Wrong output tap_key_origins"
        );
    }

    #[test]
    fn test_taproot_psbt_input_tap_tree() {
        use crate::bitcoin::psbt::serialize::Deserialize;
        use crate::bitcoin::psbt::TapTree;
        use bitcoin::hashes::hex::FromHex;
        use bitcoin::util::taproot;

        let (wallet, _, _) = get_funded_wallet(get_test_tr_with_taptree());
        let addr = wallet.get_address(AddressIndex::Peek(0)).unwrap();

        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (psbt, _) = builder.finish().unwrap();

        assert_eq!(
            psbt.inputs[0].tap_merkle_root,
            Some(
                FromHex::from_hex(
                    "61f81509635053e52d9d1217545916167394490da2287aca4693606e43851986"
                )
                .unwrap()
            ),
        );
        assert_eq!(
            psbt.inputs[0].tap_scripts.clone().into_iter().collect::<Vec<_>>(),
            vec![
                (taproot::ControlBlock::from_slice(&Vec::<u8>::from_hex("c0b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55b7ef769a745e625ed4b9a4982a4dc08274c59187e73e6f07171108f455081cb2").unwrap()).unwrap(), (from_str!("208aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642ac"), taproot::LeafVersion::TapScript)),
                (taproot::ControlBlock::from_slice(&Vec::<u8>::from_hex("c0b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55b9a515f7be31a70186e3c5937ee4a70cc4b4e1efe876c1d38e408222ffc64834").unwrap()).unwrap(), (from_str!("2051494dc22e24a32fe9dcfbd7e85faf345fa1df296fb49d156e859ef345201295ac"), taproot::LeafVersion::TapScript)),
            ],
        );
        assert_eq!(
            psbt.inputs[0].tap_internal_key,
            Some(from_str!(
                "b511bd5771e47ee27558b1765e87b541668304ec567721c7b880edc0a010da55"
            ))
        );

        // Since we are creating an output to the same address as the input, assert that the
        // internal_key is the same
        assert_eq!(
            psbt.inputs[0].tap_internal_key,
            psbt.outputs[0].tap_internal_key
        );

        assert_eq!(
            psbt.outputs[0].tap_tree,
            Some(TapTree::deserialize(&Vec::<u8>::from_hex("01c022208aee2b8120a5f157f1223f72b5e62b825831a27a9fdf427db7cc697494d4a642ac01c0222051494dc22e24a32fe9dcfbd7e85faf345fa1df296fb49d156e859ef345201295ac",).unwrap()).unwrap())
        );
    }

    #[test]
    fn test_taproot_sign_missing_witness_utxo() {
        let (wallet, _, _) = get_funded_wallet(get_test_tr_single_sig());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (mut psbt, _) = builder.finish().unwrap();
        let witness_utxo = psbt.inputs[0].witness_utxo.take();

        let result = wallet.sign(
            &mut psbt,
            SignOptions {
                allow_all_sighashes: true,
                ..Default::default()
            },
        );
        assert_matches!(
            result,
            Err(Error::Signer(SignerError::MissingWitnessUtxo)),
            "Signing should have failed with the correct error because the witness_utxo is missing"
        );

        // restore the witness_utxo
        psbt.inputs[0].witness_utxo = witness_utxo;

        let result = wallet.sign(
            &mut psbt,
            SignOptions {
                allow_all_sighashes: true,
                ..Default::default()
            },
        );

        assert_matches!(
            result,
            Ok(true),
            "Should finalize the input since we can produce signatures"
        );
    }

    #[test]
    fn test_taproot_sign_using_non_witness_utxo() {
        let (wallet, _, prev_txid) = get_funded_wallet(get_test_tr_single_sig());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (mut psbt, _) = builder.finish().unwrap();

        psbt.inputs[0].witness_utxo = None;
        psbt.inputs[0].non_witness_utxo = wallet.database().get_raw_tx(&prev_txid).unwrap();
        assert!(
            psbt.inputs[0].non_witness_utxo.is_some(),
            "Previous tx should be present in the database"
        );

        let result = wallet.sign(&mut psbt, Default::default());
        assert!(result.is_ok(), "Signing should have worked");
        assert!(
            result.unwrap(),
            "Should finalize the input since we can produce signatures"
        );
    }

    #[test]
    fn test_taproot_foreign_utxo() {
        let (wallet1, _, _) = get_funded_wallet(get_test_wpkh());
        let (wallet2, _, _) = get_funded_wallet(get_test_tr_single_sig());

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let utxo = wallet2.list_unspent().unwrap().remove(0);
        let psbt_input = wallet2.get_psbt_input(utxo.clone(), None, false).unwrap();
        let foreign_utxo_satisfaction = wallet2
            .get_descriptor_for_keychain(KeychainKind::External)
            .max_satisfaction_weight()
            .unwrap();

        assert!(
            psbt_input.non_witness_utxo.is_none(),
            "`non_witness_utxo` should never be populated for taproot"
        );

        let mut builder = wallet1.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), 60_000)
            .add_foreign_utxo(utxo.outpoint, psbt_input, foreign_utxo_satisfaction)
            .unwrap();
        let (psbt, details) = builder.finish().unwrap();

        assert_eq!(
            details.sent - details.received,
            10_000 + details.fee.unwrap_or(0),
            "we should have only net spent ~10_000"
        );

        assert!(
            psbt.unsigned_tx
                .input
                .iter()
                .any(|input| input.previous_output == utxo.outpoint),
            "foreign_utxo should be in there"
        );
    }

    fn test_spend_from_wallet(wallet: Wallet<AnyDatabase>) {
        let addr = wallet.get_address(AddressIndex::New).unwrap();

        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let (mut psbt, _) = builder.finish().unwrap();

        assert!(
            wallet.sign(&mut psbt, Default::default()).unwrap(),
            "Unable to finalize tx"
        );
    }

    #[test]
    fn test_taproot_key_spend() {
        let (wallet, _, _) = get_funded_wallet(get_test_tr_single_sig());
        test_spend_from_wallet(wallet);

        let (wallet, _, _) = get_funded_wallet(get_test_tr_single_sig_xprv());
        test_spend_from_wallet(wallet);
    }

    #[test]
    fn test_taproot_no_key_spend() {
        let (wallet, _, _) = get_funded_wallet(get_test_tr_with_taptree_both_priv());
        let addr = wallet.get_address(AddressIndex::New).unwrap();

        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let (mut psbt, _) = builder.finish().unwrap();

        assert!(
            wallet
                .sign(
                    &mut psbt,
                    SignOptions {
                        sign_with_tap_internal_key: false,
                        ..Default::default()
                    },
                )
                .unwrap(),
            "Unable to finalize tx"
        );

        assert!(psbt.inputs.iter().all(|i| i.tap_key_sig.is_none()));
    }

    #[test]
    fn test_taproot_script_spend() {
        let (wallet, _, _) = get_funded_wallet(get_test_tr_with_taptree());
        test_spend_from_wallet(wallet);

        let (wallet, _, _) = get_funded_wallet(get_test_tr_with_taptree_xprv());
        test_spend_from_wallet(wallet);
    }

    #[test]
    fn test_taproot_script_spend_sign_all_leaves() {
        use crate::signer::TapLeavesOptions;
        let (wallet, _, _) = get_funded_wallet(get_test_tr_with_taptree_both_priv());
        let addr = wallet.get_address(AddressIndex::New).unwrap();

        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let (mut psbt, _) = builder.finish().unwrap();

        assert!(
            wallet
                .sign(
                    &mut psbt,
                    SignOptions {
                        tap_leaves_options: TapLeavesOptions::All,
                        ..Default::default()
                    },
                )
                .unwrap(),
            "Unable to finalize tx"
        );

        assert!(psbt
            .inputs
            .iter()
            .all(|i| i.tap_script_sigs.len() == i.tap_scripts.len()));
    }

    #[test]
    fn test_taproot_script_spend_sign_include_some_leaves() {
        use crate::signer::TapLeavesOptions;
        use bitcoin::util::taproot::TapLeafHash;

        let (wallet, _, _) = get_funded_wallet(get_test_tr_with_taptree_both_priv());
        let addr = wallet.get_address(AddressIndex::New).unwrap();

        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let (mut psbt, _) = builder.finish().unwrap();
        let mut script_leaves: Vec<_> = psbt.inputs[0]
            .tap_scripts
            .clone()
            .values()
            .map(|(script, version)| TapLeafHash::from_script(script, *version))
            .collect();
        let included_script_leaves = vec![script_leaves.pop().unwrap()];
        let excluded_script_leaves = script_leaves;

        assert!(
            wallet
                .sign(
                    &mut psbt,
                    SignOptions {
                        tap_leaves_options: TapLeavesOptions::Include(
                            included_script_leaves.clone()
                        ),
                        ..Default::default()
                    },
                )
                .unwrap(),
            "Unable to finalize tx"
        );

        assert!(psbt.inputs[0]
            .tap_script_sigs
            .iter()
            .all(|s| included_script_leaves.contains(&s.0 .1)
                && !excluded_script_leaves.contains(&s.0 .1)));
    }

    #[test]
    fn test_taproot_script_spend_sign_exclude_some_leaves() {
        use crate::signer::TapLeavesOptions;
        use bitcoin::util::taproot::TapLeafHash;

        let (wallet, _, _) = get_funded_wallet(get_test_tr_with_taptree_both_priv());
        let addr = wallet.get_address(AddressIndex::New).unwrap();

        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let (mut psbt, _) = builder.finish().unwrap();
        let mut script_leaves: Vec<_> = psbt.inputs[0]
            .tap_scripts
            .clone()
            .values()
            .map(|(script, version)| TapLeafHash::from_script(script, *version))
            .collect();
        let included_script_leaves = vec![script_leaves.pop().unwrap()];
        let excluded_script_leaves = script_leaves;

        assert!(
            wallet
                .sign(
                    &mut psbt,
                    SignOptions {
                        tap_leaves_options: TapLeavesOptions::Exclude(
                            excluded_script_leaves.clone()
                        ),
                        ..Default::default()
                    },
                )
                .unwrap(),
            "Unable to finalize tx"
        );

        assert!(psbt.inputs[0]
            .tap_script_sigs
            .iter()
            .all(|s| included_script_leaves.contains(&s.0 .1)
                && !excluded_script_leaves.contains(&s.0 .1)));
    }

    #[test]
    fn test_taproot_script_spend_sign_no_leaves() {
        use crate::signer::TapLeavesOptions;
        let (wallet, _, _) = get_funded_wallet(get_test_tr_with_taptree_both_priv());
        let addr = wallet.get_address(AddressIndex::New).unwrap();

        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let (mut psbt, _) = builder.finish().unwrap();

        wallet
            .sign(
                &mut psbt,
                SignOptions {
                    tap_leaves_options: TapLeavesOptions::None,
                    ..Default::default()
                },
            )
            .unwrap();

        assert!(psbt.inputs.iter().all(|i| i.tap_script_sigs.is_empty()));
    }

    #[test]
    fn test_taproot_sign_derive_index_from_psbt() {
        let (wallet, _, _) = get_funded_wallet(get_test_tr_single_sig_xprv());

        let addr = wallet.get_address(AddressIndex::New).unwrap();

        let mut builder = wallet.build_tx();
        builder.add_recipient(addr.script_pubkey(), 25_000);
        let (mut psbt, _) = builder.finish().unwrap();

        // re-create the wallet with an empty db
        let wallet_empty = Wallet::new(
            get_test_tr_single_sig_xprv(),
            None,
            Network::Regtest,
            AnyDatabase::Memory(MemoryDatabase::new()),
        )
        .unwrap();

        // signing with an empty db means that we will only look at the psbt to infer the
        // derivation index
        assert!(
            wallet_empty.sign(&mut psbt, Default::default()).unwrap(),
            "Unable to finalize tx"
        );
    }

    #[test]
    fn test_taproot_sign_explicit_sighash_all() {
        let (wallet, _, _) = get_funded_wallet(get_test_tr_single_sig());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .drain_to(addr.script_pubkey())
            .sighash(SchnorrSighashType::All.into())
            .drain_wallet();
        let (mut psbt, _) = builder.finish().unwrap();

        let result = wallet.sign(&mut psbt, Default::default());
        assert!(
            result.is_ok(),
            "Signing should work because SIGHASH_ALL is safe"
        )
    }

    #[test]
    fn test_taproot_sign_non_default_sighash() {
        let sighash = SchnorrSighashType::NonePlusAnyoneCanPay;

        let (wallet, _, _) = get_funded_wallet(get_test_tr_single_sig());
        let addr = wallet.get_address(New).unwrap();
        let mut builder = wallet.build_tx();
        builder
            .drain_to(addr.script_pubkey())
            .sighash(sighash.into())
            .drain_wallet();
        let (mut psbt, _) = builder.finish().unwrap();

        let witness_utxo = psbt.inputs[0].witness_utxo.take();

        let result = wallet.sign(&mut psbt, Default::default());
        assert!(
            result.is_err(),
            "Signing should have failed because the TX uses non-standard sighashes"
        );
        assert_matches!(
            result,
            Err(Error::Signer(SignerError::NonStandardSighash)),
            "Signing failed with the wrong error type"
        );

        // try again after opting-in
        let result = wallet.sign(
            &mut psbt,
            SignOptions {
                allow_all_sighashes: true,
                ..Default::default()
            },
        );
        assert!(
            result.is_err(),
            "Signing should have failed because the witness_utxo is missing"
        );
        assert_matches!(
            result,
            Err(Error::Signer(SignerError::MissingWitnessUtxo)),
            "Signing failed with the wrong error type"
        );

        // restore the witness_utxo
        psbt.inputs[0].witness_utxo = witness_utxo;

        let result = wallet.sign(
            &mut psbt,
            SignOptions {
                allow_all_sighashes: true,
                ..Default::default()
            },
        );

        assert!(result.is_ok(), "Signing should have worked");
        assert!(
            result.unwrap(),
            "Should finalize the input since we can produce signatures"
        );

        let extracted = psbt.extract_tx();
        assert_eq!(
            *extracted.input[0].witness.to_vec()[0].last().unwrap(),
            sighash as u8,
            "The signature should have been made with the right sighash"
        );
    }

    #[test]
    fn test_spend_coinbase() {
        let descriptors = testutils!(@descriptors (get_test_wpkh()));
        let wallet = Wallet::new(
            &descriptors.0,
            None,
            Network::Regtest,
            AnyDatabase::Memory(MemoryDatabase::new()),
        )
        .unwrap();

        let confirmation_time = 5;

        crate::populate_test_db!(
            wallet.database.borrow_mut(),
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(confirmation_time),
            (@coinbase true)
        );
        let sync_time = SyncTime {
            block_time: BlockTime {
                height: confirmation_time,
                timestamp: 0,
            },
        };
        wallet
            .database
            .borrow_mut()
            .set_sync_time(sync_time)
            .unwrap();

        let not_yet_mature_time = confirmation_time + COINBASE_MATURITY - 1;
        let maturity_time = confirmation_time + COINBASE_MATURITY;

        let balance = wallet.get_balance().unwrap();
        assert_eq!(
            balance,
            Balance {
                immature: 25_000,
                trusted_pending: 0,
                untrusted_pending: 0,
                confirmed: 0
            }
        );

        // We try to create a transaction, only to notice that all
        // our funds are unspendable
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), balance.immature / 2)
            .current_height(confirmation_time);
        assert_matches!(
            builder.finish(),
            Err(Error::InsufficientFunds {
                needed: _,
                available: 0
            })
        );

        // Still unspendable...
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), balance.immature / 2)
            .current_height(not_yet_mature_time);
        assert_matches!(
            builder.finish(),
            Err(Error::InsufficientFunds {
                needed: _,
                available: 0
            })
        );

        // ...Now the coinbase is mature :)
        let sync_time = SyncTime {
            block_time: BlockTime {
                height: maturity_time,
                timestamp: 0,
            },
        };
        wallet
            .database
            .borrow_mut()
            .set_sync_time(sync_time)
            .unwrap();

        let balance = wallet.get_balance().unwrap();
        assert_eq!(
            balance,
            Balance {
                immature: 0,
                trusted_pending: 0,
                untrusted_pending: 0,
                confirmed: 25_000
            }
        );
        let mut builder = wallet.build_tx();
        builder
            .add_recipient(addr.script_pubkey(), balance.confirmed / 2)
            .current_height(maturity_time);
        builder.finish().unwrap();
    }

    #[test]
    fn test_allow_dust_limit() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_cltv());

        let addr = wallet.get_address(New).unwrap();

        let mut builder = wallet.build_tx();

        builder.add_recipient(addr.script_pubkey(), 0);

        assert_matches!(builder.finish(), Err(Error::OutputBelowDustLimit(0)));

        let mut builder = wallet.build_tx();

        builder
            .allow_dust(true)
            .add_recipient(addr.script_pubkey(), 0);

        assert!(builder.finish().is_ok());
    }

    #[test]
    fn test_fee_rate_sign_no_grinding_high_r() {
        // Our goal is to obtain a transaction with a signature with high-R (71 bytes
        // instead of 70). We then check that our fee rate and fee calculation is
        // alright.
        let (wallet, _, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
        let addr = wallet.get_address(New).unwrap();
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let mut builder = wallet.build_tx();
        let mut data = vec![0];
        builder
            .drain_to(addr.script_pubkey())
            .drain_wallet()
            .fee_rate(fee_rate)
            .add_data(&data);
        let (mut psbt, details) = builder.finish().unwrap();
        let (op_return_vout, _) = psbt
            .unsigned_tx
            .output
            .iter()
            .enumerate()
            .find(|(_n, i)| i.script_pubkey.is_op_return())
            .unwrap();

        let mut sig_len: usize = 0;
        // We try to sign many different times until we find a longer signature (71 bytes)
        while sig_len < 71 {
            // Changing the OP_RETURN data will make the signature change (but not the fee, until
            // data[0] is small enough)
            data[0] += 1;
            psbt.unsigned_tx.output[op_return_vout].script_pubkey = Script::new_op_return(&data);
            // Clearing the previous signature
            psbt.inputs[0].partial_sigs.clear();
            // Signing
            wallet
                .sign(
                    &mut psbt,
                    SignOptions {
                        remove_partial_sigs: false,
                        try_finalize: false,
                        allow_grinding: false,
                        ..Default::default()
                    },
                )
                .unwrap();
            // We only have one key in the partial_sigs map, this is a trick to retrieve it
            let key = psbt.inputs[0].partial_sigs.keys().next().unwrap();
            sig_len = psbt.inputs[0].partial_sigs[key].sig.serialize_der().len();
        }
        // Actually finalizing the transaction...
        wallet
            .sign(
                &mut psbt,
                SignOptions {
                    remove_partial_sigs: false,
                    allow_grinding: false,
                    ..Default::default()
                },
            )
            .unwrap();
        // ...and checking that everything is fine
        assert_fee_rate!(psbt, details.fee.unwrap_or(0), fee_rate);
    }

    #[test]
    fn test_fee_rate_sign_grinding_low_r() {
        // Our goal is to obtain a transaction with a signature with low-R (70 bytes)
        // by setting the `allow_grinding` signing option as true.
        // We then check that our fee rate and fee calculation is alright and that our
        // signature is 70 bytes.
        let (wallet, _, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
        let addr = wallet.get_address(New).unwrap();
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let mut builder = wallet.build_tx();
        builder
            .drain_to(addr.script_pubkey())
            .drain_wallet()
            .fee_rate(fee_rate);
        let (mut psbt, details) = builder.finish().unwrap();

        wallet
            .sign(
                &mut psbt,
                SignOptions {
                    remove_partial_sigs: false,
                    allow_grinding: true,
                    ..Default::default()
                },
            )
            .unwrap();

        let key = psbt.inputs[0].partial_sigs.keys().next().unwrap();
        let sig_len = psbt.inputs[0].partial_sigs[key].sig.serialize_der().len();
        assert_eq!(sig_len, 70);
        assert_fee_rate!(psbt, details.fee.unwrap_or(0), fee_rate);
    }

    #[cfg(feature = "test-hardware-signer")]
    #[test]
    fn test_create_signer() {
        use crate::wallet::hardwaresigner::HWISigner;
        use hwi::types::HWIChain;
        use hwi::HWIClient;

        let mut devices = HWIClient::enumerate().unwrap();
        if devices.is_empty() {
            panic!("No devices found!");
        }
        let device = devices.remove(0).unwrap();
        let client = HWIClient::get_client(&device, true, HWIChain::Regtest).unwrap();
        let descriptors = client.get_descriptors::<String>(None).unwrap();
        let custom_signer = HWISigner::from_device(&device, HWIChain::Regtest).unwrap();

        let (mut wallet, _, _) = get_funded_wallet(&descriptors.internal[0]);
        wallet.add_signer(
            KeychainKind::External,
            SignerOrdering(200),
            Arc::new(custom_signer),
        );

        let addr = wallet.get_address(LastUnused).unwrap();
        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let (mut psbt, _) = builder.finish().unwrap();

        let finalized = wallet.sign(&mut psbt, Default::default()).unwrap();
        assert!(finalized);
    }

    #[test]
    fn test_taproot_load_descriptor_duplicated_keys() {
        // Added after issue https://github.com/bitcoindevkit/bdk/issues/760
        //
        // Having the same key in multiple taproot leaves is safe and should be accepted by BDK

        let (wallet, _, _) = get_funded_wallet(get_test_tr_dup_keys());
        let addr = wallet.get_address(New).unwrap();

        assert_eq!(
            addr.to_string(),
            "bcrt1pvysh4nmh85ysrkpwtrr8q8gdadhgdejpy6f9v424a8v9htjxjhyqw9c5s5"
        );
    }
}
