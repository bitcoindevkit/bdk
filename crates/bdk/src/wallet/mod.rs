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
use crate::collections::{BTreeMap, HashMap, HashSet};
use alloc::{
    boxed::Box,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
pub use bdk_chain::keychain::Balance;
use bdk_chain::{
    indexed_tx_graph::IndexedAdditions,
    keychain::{KeychainTxOutIndex, LocalChangeSet, LocalUpdate},
    local_chain::{self, CannotConnectError, CheckPoint, CheckPointIter, LocalChain},
    tx_graph::{CanonicalTx, TxGraph},
    Append, BlockId, ChainPosition, ConfirmationTime, ConfirmationTimeAnchor, FullTxOut,
    IndexedTxGraph, Persist, PersistBackend,
};
use bitcoin::consensus::encode::serialize;
use bitcoin::secp256k1::Secp256k1;
use bitcoin::util::psbt;
use bitcoin::{
    Address, EcdsaSighashType, LockTime, Network, OutPoint, SchnorrSighashType, Script, Sequence,
    Transaction, TxOut, Txid, Witness,
};
use core::fmt;
use core::ops::Deref;
use miniscript::psbt::{PsbtExt, PsbtInputExt, PsbtInputSatisfier};

#[allow(unused_imports)]
use log::{debug, error, info, trace};

pub mod coin_selection;
pub mod export;
pub mod signer;
pub mod tx_builder;
pub(crate) mod utils;

#[cfg(feature = "hardware-signer")]
#[cfg_attr(docsrs, doc(cfg(feature = "hardware-signer")))]
pub mod hardwaresigner;

pub use utils::IsDust;

#[allow(deprecated)]
use coin_selection::DefaultCoinSelectionAlgorithm;
use signer::{SignOptions, SignerOrdering, SignersContainer, TransactionSigner};
use tx_builder::{BumpFee, CreateTx, FeePolicy, TxBuilder, TxParams};
use utils::{check_nsequence_rbf, After, Older, SecpCtx};

use crate::descriptor::policy::BuildSatisfaction;
use crate::descriptor::{
    calc_checksum, into_wallet_descriptor_checked, DerivedDescriptor, DescriptorMeta,
    ExtendedDescriptor, ExtractPolicy, IntoWalletDescriptor, Policy, XKeyUtils,
};
use crate::error::{Error, MiniscriptPsbtError};
use crate::psbt::PsbtUtils;
use crate::signer::SignerError;
use crate::types::*;
use crate::wallet::coin_selection::Excess::{Change, NoChange};

const COINBASE_MATURITY: u32 = 100;

/// A Bitcoin wallet
///
/// The `Wallet` struct acts as a way of coherently interfacing with output descriptors and related transactions.
/// Its main components are:
///
/// 1. output *descriptors* from which it can derive addresses.
/// 2. [`signer`]s that can contribute signatures to addresses instantiated from the descriptors.
///
/// [`signer`]: crate::signer
#[derive(Debug)]
pub struct Wallet<D = ()> {
    signers: Arc<SignersContainer>,
    change_signers: Arc<SignersContainer>,
    chain: LocalChain,
    indexed_graph: IndexedTxGraph<ConfirmationTimeAnchor, KeychainTxOutIndex<KeychainKind>>,
    persist: Persist<D, ChangeSet>,
    network: Network,
    secp: SecpCtx,
}

/// The update to a [`Wallet`] used in [`Wallet::apply_update`]. This is usually returned from blockchain data sources.
pub type Update = LocalUpdate<KeychainKind, ConfirmationTimeAnchor>;

/// The changeset produced internally by [`Wallet`] when mutated.
pub type ChangeSet = LocalChangeSet<KeychainKind, ConfirmationTimeAnchor>;

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

impl Wallet {
    /// Creates a wallet that does not persist data.
    pub fn new_no_persist<E: IntoWalletDescriptor>(
        descriptor: E,
        change_descriptor: Option<E>,
        network: Network,
    ) -> Result<Self, crate::descriptor::DescriptorError> {
        Self::new(descriptor, change_descriptor, (), network).map_err(|e| match e {
            NewError::Descriptor(e) => e,
            NewError::Persist(_) => unreachable!("no persistence so it can't fail"),
        })
    }
}

#[derive(Debug)]
/// Error returned from [`Wallet::new`]
pub enum NewError<P> {
    /// There was problem with the descriptors passed in
    Descriptor(crate::descriptor::DescriptorError),
    /// We were unable to load the wallet's data from the persistance backend
    Persist(P),
}

impl<P> fmt::Display for NewError<P>
where
    P: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NewError::Descriptor(e) => e.fmt(f),
            NewError::Persist(e) => {
                write!(f, "failed to load wallet from persistance backend: {}", e)
            }
        }
    }
}

/// An error that may occur when inserting a transaction into [`Wallet`].
#[derive(Debug)]
pub enum InsertTxError {
    /// The error variant that occurs when the caller attempts to insert a transaction with a
    /// confirmation height that is greater than the internal chain tip.
    ConfirmationHeightCannotBeGreaterThanTip {
        /// The internal chain's tip height.
        tip_height: Option<u32>,
        /// The introduced transaction's confirmation height.
        tx_height: u32,
    },
}

#[cfg(feature = "std")]
impl<P: core::fmt::Display + core::fmt::Debug> std::error::Error for NewError<P> {}

impl<D> Wallet<D> {
    /// Create a wallet from a `descriptor` (and an optional `change_descriptor`) and load related
    /// transaction data from `db`.
    pub fn new<E: IntoWalletDescriptor>(
        descriptor: E,
        change_descriptor: Option<E>,
        mut db: D,
        network: Network,
    ) -> Result<Self, NewError<D::LoadError>>
    where
        D: PersistBackend<ChangeSet>,
    {
        let secp = Secp256k1::new();
        let mut chain = LocalChain::default();
        let mut indexed_graph =
            IndexedTxGraph::<ConfirmationTimeAnchor, KeychainTxOutIndex<KeychainKind>>::default();

        let (descriptor, keymap) = into_wallet_descriptor_checked(descriptor, &secp, network)
            .map_err(NewError::Descriptor)?;
        indexed_graph
            .index
            .add_keychain(KeychainKind::External, descriptor.clone());
        let signers = Arc::new(SignersContainer::build(keymap, &descriptor, &secp));
        let change_signers = match change_descriptor {
            Some(desc) => {
                let (change_descriptor, change_keymap) =
                    into_wallet_descriptor_checked(desc, &secp, network)
                        .map_err(NewError::Descriptor)?;

                let change_signers = Arc::new(SignersContainer::build(
                    change_keymap,
                    &change_descriptor,
                    &secp,
                ));

                indexed_graph
                    .index
                    .add_keychain(KeychainKind::Internal, change_descriptor);

                change_signers
            }
            None => Arc::new(SignersContainer::new()),
        };

        let changeset = db.load_from_persistence().map_err(NewError::Persist)?;
        chain.apply_changeset(&changeset.chain_changeset);
        indexed_graph.apply_additions(changeset.indexed_additions);

        let persist = Persist::new(db);

        Ok(Wallet {
            signers,
            change_signers,
            network,
            chain,
            indexed_graph,
            persist,
            secp,
        })
    }

    /// Get the Bitcoin network the wallet is using.
    pub fn network(&self) -> Network {
        self.network
    }

    /// Iterator over all keychains in this wallet
    pub fn keychains(&self) -> &BTreeMap<KeychainKind, ExtendedDescriptor> {
        self.indexed_graph.index.keychains()
    }

    /// Return a derived address using the external descriptor, see [`AddressIndex`] for
    /// available address index selection strategies. If none of the keys in the descriptor are derivable
    /// (i.e. does not end with /*) then the same address will always be returned for any [`AddressIndex`].
    pub fn get_address(&mut self, address_index: AddressIndex) -> AddressInfo
    where
        D: PersistBackend<ChangeSet>,
    {
        self._get_address(KeychainKind::External, address_index)
            .expect("persistence backend must not fail")
    }

    /// Return a derived address using the internal (change) descriptor.
    ///
    /// If the wallet doesn't have an internal descriptor it will use the external descriptor.
    ///
    /// see [`AddressIndex`] for available address index selection strategies. If none of the keys
    /// in the descriptor are derivable (i.e. does not end with /*) then the same address will always
    /// be returned for any [`AddressIndex`].
    pub fn get_internal_address(&mut self, address_index: AddressIndex) -> AddressInfo
    where
        D: PersistBackend<ChangeSet>,
    {
        self._get_address(KeychainKind::Internal, address_index)
            .expect("persistence backend must not fail")
    }

    /// Return a derived address using the specified `keychain` (external/internal).
    ///
    /// If `keychain` is [`KeychainKind::External`], external addresses will be derived (used for
    /// receiving funds).
    ///
    /// If `keychain` is [`KeychainKind::Internal`], internal addresses will be derived (used for
    /// creating change outputs). If the wallet does not have an internal keychain, it will use the
    /// external keychain to derive change outputs.
    ///
    /// See [`AddressIndex`] for available address index selection strategies. If none of the keys
    /// in the descriptor are derivable (i.e. does not end with /*) then the same address will
    /// always be returned for any [`AddressIndex`].
    fn _get_address(
        &mut self,
        keychain: KeychainKind,
        address_index: AddressIndex,
    ) -> Result<AddressInfo, D::WriteError>
    where
        D: PersistBackend<ChangeSet>,
    {
        let keychain = self.map_keychain(keychain);
        let txout_index = &mut self.indexed_graph.index;
        let (index, spk, additions) = match address_index {
            AddressIndex::New => {
                let ((index, spk), index_additions) = txout_index.reveal_next_spk(&keychain);
                (index, spk.clone(), Some(index_additions))
            }
            AddressIndex::LastUnused => {
                let ((index, spk), index_additions) = txout_index.next_unused_spk(&keychain);
                (index, spk.clone(), Some(index_additions))
            }
            AddressIndex::Peek(index) => {
                let (index, spk) = txout_index
                    .spks_of_keychain(&keychain)
                    .take(index as usize + 1)
                    .last()
                    .unwrap();
                (index, spk, None)
            }
        };

        if let Some(additions) = additions {
            self.persist
                .stage(ChangeSet::from(IndexedAdditions::from(additions)));
            self.persist.commit()?;
        }

        Ok(AddressInfo {
            index,
            address: Address::from_script(&spk, self.network)
                .expect("descriptor must have address form"),
            keychain,
        })
    }

    /// Return whether or not a `script` is part of this wallet (either internal or external)
    pub fn is_mine(&self, script: &Script) -> bool {
        self.indexed_graph.index.index_of_spk(script).is_some()
    }

    /// Finds how the wallet derived the script pubkey `spk`.
    ///
    /// Will only return `Some(_)` if the wallet has given out the spk.
    pub fn derivation_of_spk(&self, spk: &Script) -> Option<(KeychainKind, u32)> {
        self.indexed_graph.index.index_of_spk(spk).copied()
    }

    /// Return the list of unspent outputs of this wallet
    pub fn list_unspent(&self) -> impl Iterator<Item = LocalUtxo> + '_ {
        self.indexed_graph
            .graph()
            .filter_chain_unspents(
                &self.chain,
                self.chain.tip().map(|cp| cp.block_id()).unwrap_or_default(),
                self.indexed_graph.index.outpoints().iter().cloned(),
            )
            .map(|((k, i), full_txo)| new_local_utxo(k, i, full_txo))
    }

    /// Get all the checkpoints the wallet is currently storing indexed by height.
    pub fn checkpoints(&self) -> CheckPointIter {
        self.chain.iter_checkpoints(None)
    }

    /// Returns the latest checkpoint.
    pub fn latest_checkpoint(&self) -> Option<CheckPoint> {
        self.chain.tip()
    }

    /// Returns a iterators of all the script pubkeys for the `Internal` and External` variants in `KeychainKind`.
    ///
    /// This is inteded to be used when doing a full scan of your addresses (e.g. after restoring
    /// from seed words). You pass the `BTreeMap` of iterators to a blockchain data source (e.g.
    /// electrum server) which will go through each address until it reaches a *stop grap*.
    ///
    /// Note carefully that iterators go over **all** script pubkeys on the keychains (not what
    /// script pubkeys the wallet is storing internally).
    pub fn spks_of_all_keychains(
        &self,
    ) -> BTreeMap<KeychainKind, impl Iterator<Item = (u32, Script)> + Clone> {
        self.indexed_graph.index.spks_of_all_keychains()
    }

    /// Gets an iterator over all the script pubkeys in a single keychain.
    ///
    /// See [`spks_of_all_keychains`] for more documentation
    ///
    /// [`spks_of_all_keychains`]: Self::spks_of_all_keychains
    pub fn spks_of_keychain(
        &self,
        keychain: KeychainKind,
    ) -> impl Iterator<Item = (u32, Script)> + Clone {
        self.indexed_graph.index.spks_of_keychain(&keychain)
    }

    /// Returns the utxo owned by this wallet corresponding to `outpoint` if it exists in the
    /// wallet's database.
    pub fn get_utxo(&self, op: OutPoint) -> Option<LocalUtxo> {
        let (&spk_i, _) = self.indexed_graph.index.txout(op)?;
        self.indexed_graph
            .graph()
            .filter_chain_unspents(
                &self.chain,
                self.chain.tip().map(|cp| cp.block_id()).unwrap_or_default(),
                core::iter::once((spk_i, op)),
            )
            .map(|((k, i), full_txo)| new_local_utxo(k, i, full_txo))
            .next()
    }

    /// Return a single transactions made and received by the wallet
    ///
    /// Optionally fill the [`TransactionDetails::transaction`] field with the raw transaction if
    /// `include_raw` is `true`.
    pub fn get_tx(&self, txid: Txid, include_raw: bool) -> Option<TransactionDetails> {
        let graph = self.indexed_graph.graph();

        let canonical_tx = CanonicalTx {
            observed_as: graph.get_chain_position(
                &self.chain,
                self.chain.tip().map(|cp| cp.block_id()).unwrap_or_default(),
                txid,
            )?,
            node: graph.get_tx_node(txid)?,
        };

        Some(new_tx_details(
            &self.indexed_graph,
            canonical_tx,
            include_raw,
        ))
    }

    /// Add a new checkpoint to the wallet's internal view of the chain.
    /// This stages but does not [`commit`] the change.
    ///
    /// Returns whether anything changed with the insertion (e.g. `false` if checkpoint was already
    /// there).
    ///
    /// [`commit`]: Self::commit
    pub fn insert_checkpoint(
        &mut self,
        block_id: BlockId,
    ) -> Result<bool, local_chain::InsertBlockError>
    where
        D: PersistBackend<ChangeSet>,
    {
        let (_, changeset) = self.chain.get_or_insert(block_id)?;
        let changed = !changeset.is_empty();
        self.persist.stage(changeset.into());
        Ok(changed)
    }

    /// Add a transaction to the wallet's internal view of the chain. This stages but does not
    /// [`commit`] the change.
    ///
    /// Returns whether anything changed with the transaction insertion (e.g. `false` if the
    /// transaction was already inserted at the same position).
    ///
    /// A `tx` can be rejected if `position` has a height greater than the [`latest_checkpoint`].
    /// Therefore you should use [`insert_checkpoint`] to insert new checkpoints before manually
    /// inserting new transactions.
    ///
    /// **WARNING:** If `position` is confirmed, we anchor the `tx` to a the lowest checkpoint that
    /// is >= the `position`'s height. The caller is responsible for ensuring the `tx` exists in our
    /// local view of the best chain's history.
    ///
    /// [`commit`]: Self::commit
    /// [`latest_checkpoint`]: Self::latest_checkpoint
    /// [`insert_checkpoint`]: Self::insert_checkpoint
    pub fn insert_tx(
        &mut self,
        tx: Transaction,
        position: ConfirmationTime,
    ) -> Result<bool, InsertTxError>
    where
        D: PersistBackend<ChangeSet>,
    {
        let (anchor, last_seen) = match position {
            ConfirmationTime::Confirmed { height, time } => {
                // anchor tx to checkpoint with lowest height that is >= position's height
                let anchor = self
                    .chain
                    .checkpoints()
                    .range(height..)
                    .next()
                    .ok_or(InsertTxError::ConfirmationHeightCannotBeGreaterThanTip {
                        tip_height: self.chain.tip().map(|b| b.height()),
                        tx_height: height,
                    })
                    .map(|(&_, cp)| ConfirmationTimeAnchor {
                        anchor_block: cp.block_id(),
                        confirmation_height: height,
                        confirmation_time: time,
                    })?;

                (Some(anchor), None)
            }
            ConfirmationTime::Unconfirmed { last_seen } => (None, Some(last_seen)),
        };

        let changeset: ChangeSet = self.indexed_graph.insert_tx(&tx, anchor, last_seen).into();
        let changed = !changeset.is_empty();
        self.persist.stage(changeset);
        Ok(changed)
    }

    /// Iterate over the transactions in the wallet.
    pub fn transactions(
        &self,
    ) -> impl Iterator<Item = CanonicalTx<'_, Transaction, ConfirmationTimeAnchor>> + '_ {
        self.indexed_graph.graph().list_chain_txs(
            &self.chain,
            self.chain.tip().map(|cp| cp.block_id()).unwrap_or_default(),
        )
    }

    /// Return the balance, separated into available, trusted-pending, untrusted-pending and immature
    /// values.
    pub fn get_balance(&self) -> Balance {
        self.indexed_graph.graph().balance(
            &self.chain,
            self.chain.tip().map(|cp| cp.block_id()).unwrap_or_default(),
            self.indexed_graph.index.outpoints().iter().cloned(),
            |&(k, _), _| k == KeychainKind::Internal,
        )
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
    /// let wallet = Wallet::new_no_persist("wpkh(tprv8ZgxMBicQKsPe73PBRSmNbTfbcsZnwWhz5eVmhHpi31HW29Z7mc9B4cWGRQzopNUzZUT391DeDJxL2PefNunWyLgqCKRMDkU1s2s8bAfoSk/84'/0'/0'/0/*)", None, Network::Testnet)?;
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
    /// # let descriptor = "wpkh(tpubD6NzVbkrYhZ4Xferm7Pz4VnjdcDPFyjVu5K4iZXQ4pVN8Cks4pHVowTBXBKRhX64pkRyJZJN5xAKj4UDNnLPb5p2sSKXhewoYx5GbTdUFWq/*)";
    /// # let mut wallet = doctest_wallet!();
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
    pub fn build_tx(&mut self) -> TxBuilder<'_, D, DefaultCoinSelectionAlgorithm, CreateTx> {
        TxBuilder {
            wallet: alloc::rc::Rc::new(core::cell::RefCell::new(self)),
            params: TxParams::default(),
            coin_selection: DefaultCoinSelectionAlgorithm::default(),
            phantom: core::marker::PhantomData,
        }
    }

    pub(crate) fn create_tx<Cs: coin_selection::CoinSelectionAlgorithm>(
        &mut self,
        coin_selection: Cs,
        params: TxParams,
    ) -> Result<(psbt::PartiallySignedTransaction, TransactionDetails), Error>
    where
        D: PersistBackend<ChangeSet>,
    {
        let external_descriptor = self
            .indexed_graph
            .index
            .keychains()
            .get(&KeychainKind::External)
            .expect("must exist");
        let internal_descriptor = self
            .indexed_graph
            .index
            .keychains()
            .get(&KeychainKind::Internal);

        let external_policy = external_descriptor
            .extract_policy(&self.signers, BuildSatisfaction::None, &self.secp)?
            .unwrap();
        let internal_policy = internal_descriptor
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
            None => self
                .chain
                .tip()
                .map(|cp| LockTime::from_height(cp.height()).expect("Invalid height")),
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

            if self.is_mine(script_pubkey) {
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
            && internal_descriptor.is_none()
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
        );

        // get drain script
        let drain_script = match params.drain_to {
            Some(ref drain_recipient) => drain_recipient.clone(),
            None => {
                let change_keychain = self.map_keychain(KeychainKind::Internal);
                let ((index, spk), index_additions) =
                    self.indexed_graph.index.next_unused_spk(&change_keychain);
                let spk = spk.clone();
                self.indexed_graph.index.mark_used(&change_keychain, index);
                self.persist
                    .stage(ChangeSet::from(IndexedAdditions::from(index_additions)));
                self.persist.commit().expect("TODO");
                spk
            }
        };

        let coin_selection = coin_selection.coin_select(
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
                if self.is_mine(&drain_script) {
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
            confirmation_time: ConfirmationTime::Unconfirmed { last_seen: 0 },
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
    /// # let descriptor = "wpkh(tpubD6NzVbkrYhZ4Xferm7Pz4VnjdcDPFyjVu5K4iZXQ4pVN8Cks4pHVowTBXBKRhX64pkRyJZJN5xAKj4UDNnLPb5p2sSKXhewoYx5GbTdUFWq/*)";
    /// # let mut wallet = doctest_wallet!();
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
        &mut self,
        txid: Txid,
    ) -> Result<TxBuilder<'_, D, DefaultCoinSelectionAlgorithm, BumpFee>, Error> {
        let graph = self.indexed_graph.graph();
        let txout_index = &self.indexed_graph.index;
        let chain_tip = self.chain.tip().map(|cp| cp.block_id()).unwrap_or_default();

        let mut tx = graph
            .get_tx(txid)
            .ok_or(Error::TransactionNotFound)?
            .clone();

        let pos = graph
            .get_chain_position(&self.chain, chain_tip, txid)
            .ok_or(Error::TransactionNotFound)?;
        if let ChainPosition::Confirmed(_) = pos {
            return Err(Error::TransactionConfirmed);
        }

        if !tx
            .input
            .iter()
            .any(|txin| txin.sequence.to_consensus_u32() <= 0xFFFFFFFD)
        {
            return Err(Error::IrreplaceableTransaction);
        }

        let fee = graph.calculate_fee(&tx).ok_or(Error::FeeRateUnavailable)?;
        if fee < 0 {
            // It's available but it's wrong so let's say it's unavailable
            return Err(Error::FeeRateUnavailable)?;
        }
        let fee = fee as u64;
        let feerate = FeeRate::from_wu(fee, tx.weight());

        // remove the inputs from the tx and process them
        let original_txin = tx.input.drain(..).collect::<Vec<_>>();
        let original_utxos = original_txin
            .iter()
            .map(|txin| -> Result<_, Error> {
                let prev_tx = graph
                    .get_tx(txin.previous_output.txid)
                    .ok_or(Error::UnknownUtxo)?;
                let txout = &prev_tx.output[txin.previous_output.vout as usize];

                let confirmation_time: ConfirmationTime = graph
                    .get_chain_position(&self.chain, chain_tip, txin.previous_output.txid)
                    .ok_or(Error::UnknownUtxo)?
                    .cloned()
                    .into();

                let weighted_utxo = match txout_index.index_of_spk(&txout.script_pubkey) {
                    Some(&(keychain, derivation_index)) => {
                        let satisfaction_weight = self
                            .get_descriptor_for_keychain(keychain)
                            .max_satisfaction_weight()
                            .unwrap();
                        WeightedUtxo {
                            utxo: Utxo::Local(LocalUtxo {
                                outpoint: txin.previous_output,
                                txout: txout.clone(),
                                keychain,
                                is_spent: true,
                                derivation_index,
                                confirmation_time,
                            }),
                            satisfaction_weight,
                        }
                    }
                    None => {
                        let satisfaction_weight =
                            serialize(&txin.script_sig).len() * 4 + serialize(&txin.witness).len();
                        WeightedUtxo {
                            satisfaction_weight,
                            utxo: Utxo::Foreign {
                                outpoint: txin.previous_output,
                                psbt_input: Box::new(psbt::Input {
                                    witness_utxo: Some(txout.clone()),
                                    non_witness_utxo: Some(prev_tx.clone()),
                                    ..Default::default()
                                }),
                            },
                        }
                    }
                };

                Ok(weighted_utxo)
            })
            .collect::<Result<Vec<_>, _>>()?;

        if tx.output.len() > 1 {
            let mut change_index = None;
            for (index, txout) in tx.output.iter().enumerate() {
                let change_type = self.map_keychain(KeychainKind::Internal);
                match txout_index.index_of_spk(&txout.script_pubkey) {
                    Some(&(keychain, _)) if keychain == change_type => change_index = Some(index),
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
                absolute: fee,
                rate: feerate.as_sat_per_vb(),
            }),
            ..Default::default()
        };

        Ok(TxBuilder {
            wallet: alloc::rc::Rc::new(core::cell::RefCell::new(self)),
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
    /// # let descriptor = "wpkh(tpubD6NzVbkrYhZ4Xferm7Pz4VnjdcDPFyjVu5K4iZXQ4pVN8Cks4pHVowTBXBKRhX64pkRyJZJN5xAKj4UDNnLPb5p2sSKXhewoYx5GbTdUFWq/*)";
    /// # let mut wallet = doctest_wallet!();
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
        let signers = match keychain {
            KeychainKind::External => &self.signers,
            KeychainKind::Internal => &self.change_signers,
        };

        match self.public_descriptor(keychain) {
            Some(desc) => Ok(desc.extract_policy(signers, BuildSatisfaction::None, &self.secp)?),
            None => Ok(None),
        }
    }

    /// Return the "public" version of the wallet's descriptor, meaning a new descriptor that has
    /// the same structure but with every secret key removed
    ///
    /// This can be used to build a watch-only version of a wallet
    pub fn public_descriptor(&self, keychain: KeychainKind) -> Option<&ExtendedDescriptor> {
        self.indexed_graph.index.keychains().get(&keychain)
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
        let chain_tip = self.chain.tip().map(|cp| cp.block_id()).unwrap_or_default();

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
            let confirmation_height = self
                .indexed_graph
                .graph()
                .get_chain_position(&self.chain, chain_tip, input.previous_output.txid)
                .map(|observed_as| match observed_as {
                    ChainPosition::Confirmed(a) => a.confirmation_height,
                    ChainPosition::Unconfirmed(_) => u32::MAX,
                });
            let current_height = sign_options
                .assume_height
                .or(self.chain.tip().map(|b| b.height()));

            debug!(
                "Input #{} - {}, using `confirmation_height` = {:?}, `current_height` = {:?}",
                n, input.previous_output, confirmation_height, current_height
            );

            // - Try to derive the descriptor by looking at the txout. If it's in our database, we
            //   know exactly which `keychain` to use, and which derivation index it is
            // - If that fails, try to derive it by looking at the psbt input: the complete logic
            //   is in `src/descriptor/mod.rs`, but it will basically look at `bip32_derivation`,
            //   `redeem_script` and `witness_script` to determine the right derivation
            // - If that also fails, it will try it on the internal descriptor, if present
            let desc = psbt
                .get_utxo_for(n)
                .and_then(|txout| self.get_descriptor_for_txout(&txout))
                .or_else(|| {
                    self.indexed_graph
                        .index
                        .keychains()
                        .iter()
                        .find_map(|(_, desc)| {
                            desc.derive_from_psbt_input(
                                psbt_input,
                                psbt.get_utxo_for(n),
                                &self.secp,
                            )
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
                            Older::new(current_height, confirmation_height, false),
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
        self.public_descriptor(self.map_keychain(keychain))
            .expect("we mapped it to external if it doesn't exist")
    }

    /// The derivation index of this wallet. It will return `None` if it has not derived any addresses.
    /// Otherwise, it will return the index of the highest address it has derived.
    pub fn derivation_index(&self, keychain: KeychainKind) -> Option<u32> {
        self.indexed_graph.index.last_revealed_index(&keychain)
    }

    /// The index of the next address that you would get if you were to ask the wallet for a new address
    pub fn next_derivation_index(&self, keychain: KeychainKind) -> u32 {
        self.indexed_graph.index.next_index(&keychain).0
    }

    /// Informs the wallet that you no longer intend to broadcast a tx that was built from it.
    ///
    /// This frees up the change address used when creating the tx for use in future transactions.
    // TODO: Make this free up reserved utxos when that's implemented
    pub fn cancel_tx(&mut self, tx: &Transaction) {
        let txout_index = &mut self.indexed_graph.index;
        for txout in &tx.output {
            if let Some(&(keychain, index)) = txout_index.index_of_spk(&txout.script_pubkey) {
                // NOTE: unmark_used will **not** make something unused if it has actually been used
                // by a tx in the tracker. It only removes the superficial marking.
                txout_index.unmark_used(&keychain, index);
            }
        }
    }

    fn map_keychain(&self, keychain: KeychainKind) -> KeychainKind {
        if keychain == KeychainKind::Internal
            && self.public_descriptor(KeychainKind::Internal).is_none()
        {
            KeychainKind::External
        } else {
            keychain
        }
    }

    fn get_descriptor_for_txout(&self, txout: &TxOut) -> Option<DerivedDescriptor> {
        let &(keychain, child) = self
            .indexed_graph
            .index
            .index_of_spk(&txout.script_pubkey)?;
        let descriptor = self.get_descriptor_for_keychain(keychain);
        Some(descriptor.at_derivation_index(child))
    }

    fn get_available_utxos(&self) -> Vec<(LocalUtxo, usize)> {
        self.list_unspent()
            .map(|utxo| {
                let keychain = utxo.keychain;
                (
                    utxo,
                    self.get_descriptor_for_keychain(keychain)
                        .max_satisfaction_weight()
                        .unwrap(),
                )
            })
            .collect()
    }

    /// Given the options returns the list of utxos that must be used to form the
    /// transaction and any further that may be used if needed.
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
    ) -> (Vec<WeightedUtxo>, Vec<WeightedUtxo>) {
        let chain_tip = self.chain.tip().map(|cp| cp.block_id()).unwrap_or_default();
        //    must_spend <- manually selected utxos
        //    may_spend  <- all other available utxos
        let mut may_spend = self.get_available_utxos();

        may_spend.retain(|may_spend| {
            !manually_selected
                .iter()
                .any(|manually_selected| manually_selected.utxo.outpoint() == may_spend.0.outpoint)
        });
        let mut must_spend = manually_selected;

        // NOTE: we are intentionally ignoring `unspendable` here. i.e manual
        // selection overrides unspendable.
        if manual_only {
            return (must_spend, vec![]);
        }

        let satisfies_confirmed = may_spend
            .iter()
            .map(|u| -> bool {
                let txid = u.0.outpoint.txid;
                let tx = match self.indexed_graph.graph().get_tx(txid) {
                    Some(tx) => tx,
                    None => return false,
                };
                let confirmation_time: ConfirmationTime = match self
                    .indexed_graph
                    .graph()
                    .get_chain_position(&self.chain, chain_tip, txid)
                {
                    Some(observed_as) => observed_as.cloned().into(),
                    None => return false,
                };

                // Whether the UTXO is mature and, if needed, confirmed
                let mut spendable = true;
                if must_only_use_confirmed_tx && !confirmation_time.is_confirmed() {
                    return false;
                }
                if tx.is_coin_base() {
                    debug_assert!(
                        confirmation_time.is_confirmed(),
                        "coinbase must always be confirmed"
                    );
                    if let Some(current_height) = current_height {
                        match confirmation_time {
                            ConfirmationTime::Confirmed { height, .. } => {
                                // https://github.com/bitcoin/bitcoin/blob/c5e67be03bb06a5d7885c55db1f016fbf2333fe3/src/validation.cpp#L373-L375
                                spendable &=
                                    (current_height.saturating_sub(height)) >= COINBASE_MATURITY;
                            }
                            ConfirmationTime::Unconfirmed { .. } => spendable = false,
                        }
                    }
                }
                spendable
            })
            .collect::<Vec<_>>();

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

        (must_spend, may_spend)
    }

    fn complete_transaction(
        &self,
        tx: Transaction,
        selected: Vec<Utxo>,
        params: TxParams,
    ) -> Result<psbt::PartiallySignedTransaction, Error> {
        let mut psbt = psbt::PartiallySignedTransaction::from_unsigned_tx(tx)?;

        if params.add_global_xpubs {
            let all_xpubs = self
                .keychains()
                .iter()
                .flat_map(|(_, desc)| desc.get_extended_keys())
                .collect::<Vec<_>>();

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
        let &(keychain, child) = self
            .indexed_graph
            .index
            .index_of_spk(&utxo.txout.script_pubkey)
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
        if let Some(prev_tx) = self.indexed_graph.graph().get_tx(prev_output.txid) {
            if desc.is_witness() || desc.is_taproot() {
                psbt_input.witness_utxo = Some(prev_tx.output[prev_output.vout as usize].clone());
            }
            if !desc.is_taproot() && (!desc.is_witness() || !only_witness_utxo) {
                psbt_input.non_witness_utxo = Some(prev_tx.clone());
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
            if let Some(&(keychain, child)) =
                self.indexed_graph.index.index_of_spk(&out.script_pubkey)
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

    /// Applies an update to the wallet and stages the changes (but does not [`commit`] them).
    ///
    /// This returns whether the `update` resulted in any changes.
    ///
    /// Usually you create an `update` by interacting with some blockchain data source and inserting
    /// transactions related to your wallet into it.
    ///
    /// [`commit`]: Self::commit
    pub fn apply_update(&mut self, update: Update) -> Result<bool, CannotConnectError>
    where
        D: PersistBackend<ChangeSet>,
    {
        let mut changeset = ChangeSet::from(self.chain.apply_update(update.tip)?);
        let (_, index_additions) = self
            .indexed_graph
            .index
            .reveal_to_target_multi(&update.keychain);
        changeset.append(ChangeSet::from(IndexedAdditions::from(index_additions)));
        changeset.append(self.indexed_graph.apply_update(update.graph).into());

        let changed = !changeset.is_empty();
        self.persist.stage(changeset);
        Ok(changed)
    }

    /// Commits all curently [`staged`] changed to the persistence backend returning and error when
    /// this fails.
    ///
    /// This returns whether the `update` resulted in any changes.
    ///
    /// [`staged`]: Self::staged
    pub fn commit(&mut self) -> Result<bool, D::WriteError>
    where
        D: PersistBackend<ChangeSet>,
    {
        self.persist.commit().map(|c| c.is_some())
    }

    /// Returns the changes that will be staged with the next call to [`commit`].
    ///
    /// [`commit`]: Self::commit
    pub fn staged(&self) -> &ChangeSet
    where
        D: PersistBackend<ChangeSet>,
    {
        self.persist.staged()
    }

    /// Get a reference to the inner [`TxGraph`].
    pub fn tx_graph(&self) -> &TxGraph<ConfirmationTimeAnchor> {
        self.indexed_graph.graph()
    }

    /// Get a reference to the inner [`KeychainTxOutIndex`].
    pub fn spk_index(&self) -> &KeychainTxOutIndex<KeychainKind> {
        &self.indexed_graph.index
    }

    /// Get a reference to the inner [`LocalChain`].
    pub fn local_chain(&self) -> &LocalChain {
        &self.chain
    }
}

impl<D> AsRef<bdk_chain::tx_graph::TxGraph<ConfirmationTimeAnchor>> for Wallet<D> {
    fn as_ref(&self) -> &bdk_chain::tx_graph::TxGraph<ConfirmationTimeAnchor> {
        self.indexed_graph.graph()
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

fn new_local_utxo(
    keychain: KeychainKind,
    derivation_index: u32,
    full_txo: FullTxOut<ConfirmationTimeAnchor>,
) -> LocalUtxo {
    LocalUtxo {
        outpoint: full_txo.outpoint,
        txout: full_txo.txout,
        is_spent: full_txo.spent_by.is_some(),
        confirmation_time: full_txo.chain_position.into(),
        keychain,
        derivation_index,
    }
}

fn new_tx_details(
    indexed_graph: &IndexedTxGraph<ConfirmationTimeAnchor, KeychainTxOutIndex<KeychainKind>>,
    canonical_tx: CanonicalTx<'_, Transaction, ConfirmationTimeAnchor>,
    include_raw: bool,
) -> TransactionDetails {
    let graph = indexed_graph.graph();
    let index = &indexed_graph.index;
    let tx = canonical_tx.node.tx;

    let received = tx
        .output
        .iter()
        .map(|txout| {
            if index.index_of_spk(&txout.script_pubkey).is_some() {
                txout.value
            } else {
                0
            }
        })
        .sum();

    let sent = tx
        .input
        .iter()
        .map(|txin| {
            if let Some((_, txout)) = index.txout(txin.previous_output) {
                txout.value
            } else {
                0
            }
        })
        .sum();

    let inputs = tx
        .input
        .iter()
        .map(|txin| {
            graph
                .get_txout(txin.previous_output)
                .map(|txout| txout.value)
        })
        .sum::<Option<u64>>();
    let outputs = tx.output.iter().map(|txout| txout.value).sum();
    let fee = inputs.map(|inputs| inputs.saturating_sub(outputs));

    TransactionDetails {
        transaction: if include_raw { Some(tx.clone()) } else { None },
        txid: canonical_tx.node.txid,
        received,
        sent,
        fee,
        confirmation_time: canonical_tx.observed_as.cloned().into(),
    }
}

#[macro_export]
#[doc(hidden)]
/// Macro for getting a wallet for use in a doctest
macro_rules! doctest_wallet {
    () => {{
        use $crate::bitcoin::{BlockHash, Transaction, PackedLockTime, TxOut, Network, hashes::Hash};
        use $crate::chain::{ConfirmationTime, BlockId};
        use $crate::wallet::{AddressIndex, Wallet};
        let descriptor = "tr([73c5da0a/86'/0'/0']tprv8fMn4hSKPRC1oaCPqxDb1JWtgkpeiQvZhsr8W2xuy3GEMkzoArcAWTfJxYb6Wj8XNNDWEjfYKK4wGQXh3ZUXhDF2NcnsALpWTeSwarJt7Vc/0/*)";
        let change_descriptor = "tr([73c5da0a/86'/0'/0']tprv8fMn4hSKPRC1oaCPqxDb1JWtgkpeiQvZhsr8W2xuy3GEMkzoArcAWTfJxYb6Wj8XNNDWEjfYKK4wGQXh3ZUXhDF2NcnsALpWTeSwarJt7Vc/1/*)";

        let mut wallet = Wallet::new_no_persist(
            descriptor,
            Some(change_descriptor),
            Network::Regtest,
        )
        .unwrap();
        let address = wallet.get_address(AddressIndex::New).address;
        let tx = Transaction {
            version: 1,
            lock_time: PackedLockTime(0),
            input: vec![],
            output: vec![TxOut {
                value: 500_000,
                script_pubkey: address.script_pubkey(),
            }],
        };
        let _ = wallet.insert_checkpoint(BlockId { height: 1_000, hash: BlockHash::all_zeros() });
        let _ = wallet.insert_tx(tx.clone(), ConfirmationTime::Confirmed {
            height: 500,
            time: 50_000
        });

        wallet
    }}
}
