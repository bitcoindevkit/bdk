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
//! This module defines the [`Wallet`].

use alloc::{
    boxed::Box,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use core::{cmp::Ordering, fmt, mem, ops::Deref};

use bdk_chain::{
    indexed_tx_graph,
    indexer::keychain_txout::KeychainTxOutIndex,
    local_chain::{ApplyHeaderError, CannotConnectError, CheckPoint, CheckPointIter, LocalChain},
    spk_client::{
        FullScanRequest, FullScanRequestBuilder, FullScanResponse, SyncRequest, SyncRequestBuilder,
        SyncResponse,
    },
    tx_graph::{CalculateFeeError, CanonicalTx, TxGraph, TxUpdate},
    BlockId, ChainPosition, ConfirmationBlockTime, DescriptorExt, FullTxOut, Indexed,
    IndexedTxGraph, Indexer, Merge,
};
use bitcoin::{
    absolute,
    consensus::encode::serialize,
    constants::genesis_block,
    psbt,
    secp256k1::Secp256k1,
    sighash::{EcdsaSighashType, TapSighashType},
    transaction, Address, Amount, Block, BlockHash, FeeRate, Network, OutPoint, Psbt, ScriptBuf,
    Sequence, Transaction, TxOut, Txid, Weight, Witness,
};
use miniscript::{
    descriptor::KeyMap,
    psbt::{PsbtExt, PsbtInputExt, PsbtInputSatisfier},
};
use rand_core::RngCore;

mod changeset;
pub mod coin_selection;
pub mod error;
pub mod export;
mod params;
mod persisted;
pub mod signer;
pub mod tx_builder;
pub(crate) mod utils;

use crate::collections::{BTreeMap, HashMap, HashSet};
use crate::descriptor::{
    check_wallet_descriptor, error::Error as DescriptorError, policy::BuildSatisfaction,
    DerivedDescriptor, DescriptorMeta, ExtendedDescriptor, ExtractPolicy, IntoWalletDescriptor,
    Policy, XKeyUtils,
};
use crate::psbt::PsbtUtils;
use crate::types::*;
use crate::wallet::{
    coin_selection::{DefaultCoinSelectionAlgorithm, Excess, InsufficientFunds},
    error::{BuildFeeBumpError, CreateTxError, MiniscriptPsbtError},
    signer::{SignOptions, SignerError, SignerOrdering, SignersContainer, TransactionSigner},
    tx_builder::{FeePolicy, TxBuilder, TxParams},
    utils::{check_nsequence_rbf, After, Older, SecpCtx},
};

// re-exports
pub use bdk_chain::Balance;
pub use changeset::ChangeSet;
pub use params::*;
pub use persisted::*;
pub use utils::IsDust;

/// A Bitcoin wallet
///
/// The `Wallet` acts as a way of coherently interfacing with output descriptors and related transactions.
/// Its main components are:
///
/// 1. output *descriptors* from which it can derive addresses.
/// 2. [`signer`]s that can contribute signatures to addresses instantiated from the descriptors.
///
/// The user is responsible for loading and writing wallet changes which are represented as
/// [`ChangeSet`]s (see [`take_staged`]). Also see individual functions and example for instructions
/// on when [`Wallet`] state needs to be persisted.
///
/// The `Wallet` descriptor (external) and change descriptor (internal) must not derive the same
/// script pubkeys. See [`KeychainTxOutIndex::insert_descriptor()`] for more details.
///
/// [`signer`]: crate::signer
/// [`take_staged`]: Wallet::take_staged
#[derive(Debug)]
pub struct Wallet {
    signers: Arc<SignersContainer>,
    change_signers: Arc<SignersContainer>,
    chain: LocalChain,
    indexed_graph: IndexedTxGraph<ConfirmationBlockTime, KeychainTxOutIndex<KeychainKind>>,
    stage: ChangeSet,
    network: Network,
    secp: SecpCtx,
}

/// An update to [`Wallet`].
///
/// It updates [`KeychainTxOutIndex`], [`bdk_chain::TxGraph`] and [`LocalChain`] atomically.
#[derive(Debug, Clone, Default)]
pub struct Update {
    /// Contains the last active derivation indices per keychain (`K`), which is used to update the
    /// [`KeychainTxOutIndex`].
    pub last_active_indices: BTreeMap<KeychainKind, u32>,

    /// Update for the wallet's internal [`TxGraph`].
    pub tx_update: TxUpdate<ConfirmationBlockTime>,

    /// Update for the wallet's internal [`LocalChain`].
    pub chain: Option<CheckPoint>,
}

impl From<FullScanResponse<KeychainKind>> for Update {
    fn from(value: FullScanResponse<KeychainKind>) -> Self {
        Self {
            last_active_indices: value.last_active_indices,
            tx_update: value.tx_update,
            chain: value.chain_update,
        }
    }
}

impl From<SyncResponse> for Update {
    fn from(value: SyncResponse) -> Self {
        Self {
            last_active_indices: BTreeMap::new(),
            tx_update: value.tx_update,
            chain: value.chain_update,
        }
    }
}

/// A derived address and the index it was found at.
/// For convenience this automatically derefs to `Address`
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// The error type when loading a [`Wallet`] from a [`ChangeSet`].
#[derive(Debug, PartialEq)]
pub enum LoadError {
    /// There was a problem with the passed-in descriptor(s).
    Descriptor(crate::descriptor::DescriptorError),
    /// Data loaded from persistence is missing network type.
    MissingNetwork,
    /// Data loaded from persistence is missing genesis hash.
    MissingGenesis,
    /// Data loaded from persistence is missing descriptor.
    MissingDescriptor(KeychainKind),
    /// Data loaded is unexpected.
    Mismatch(LoadMismatch),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Descriptor(e) => e.fmt(f),
            LoadError::MissingNetwork => write!(f, "loaded data is missing network type"),
            LoadError::MissingGenesis => write!(f, "loaded data is missing genesis hash"),
            LoadError::MissingDescriptor(k) => {
                write!(f, "loaded data is missing descriptor for keychain {k:?}")
            }
            LoadError::Mismatch(mismatch) => write!(f, "data mismatch: {mismatch:?}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for LoadError {}

/// Represents a mismatch with what is loaded and what is expected from [`LoadParams`].
#[derive(Debug, PartialEq)]
pub enum LoadMismatch {
    /// Network does not match.
    Network {
        /// The network that is loaded.
        loaded: Network,
        /// The expected network.
        expected: Network,
    },
    /// Genesis hash does not match.
    Genesis {
        /// The genesis hash that is loaded.
        loaded: BlockHash,
        /// The expected genesis hash.
        expected: BlockHash,
    },
    /// Descriptor's [`DescriptorId`](bdk_chain::DescriptorId) does not match.
    Descriptor {
        /// Keychain identifying the descriptor.
        keychain: KeychainKind,
        /// The loaded descriptor.
        loaded: Option<ExtendedDescriptor>,
        /// The expected descriptor.
        expected: Option<ExtendedDescriptor>,
    },
}

impl From<LoadMismatch> for LoadError {
    fn from(mismatch: LoadMismatch) -> Self {
        Self::Mismatch(mismatch)
    }
}

impl<E> From<LoadMismatch> for LoadWithPersistError<E> {
    fn from(mismatch: LoadMismatch) -> Self {
        Self::InvalidChangeSet(LoadError::Mismatch(mismatch))
    }
}

/// An error that may occur when applying a block to [`Wallet`].
#[derive(Debug)]
pub enum ApplyBlockError {
    /// Occurs when the update chain cannot connect with original chain.
    CannotConnect(CannotConnectError),
    /// Occurs when the `connected_to` hash does not match the hash derived from `block`.
    UnexpectedConnectedToHash {
        /// Block hash of `connected_to`.
        connected_to_hash: BlockHash,
        /// Expected block hash of `connected_to`, as derived from `block`.
        expected_hash: BlockHash,
    },
}

impl fmt::Display for ApplyBlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApplyBlockError::CannotConnect(err) => err.fmt(f),
            ApplyBlockError::UnexpectedConnectedToHash {
                expected_hash: block_hash,
                connected_to_hash: checkpoint_hash,
            } => write!(
                f,
                "`connected_to` hash {} differs from the expected hash {} (which is derived from `block`)",
                checkpoint_hash, block_hash
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ApplyBlockError {}

/// A `CanonicalTx` managed by a `Wallet`.
pub type WalletTx<'a> = CanonicalTx<'a, Arc<Transaction>, ConfirmationBlockTime>;

impl Wallet {
    /// Build a new single descriptor [`Wallet`].
    ///
    /// If you have previously created a wallet, use [`load`](Self::load) instead.
    ///
    /// # Note
    ///
    /// Only use this method when creating a wallet designed to be used with a single
    /// descriptor and keychain. Otherwise the recommended way to construct a new wallet is
    /// by using [`Wallet::create`]. It's worth noting that not all features are available
    /// with single descriptor wallets, for example setting a [`change_policy`] on [`TxBuilder`]
    /// and related methods such as [`do_not_spend_change`]. This is because all payments are
    /// received on the external keychain (including change), and without a change keychain
    /// BDK lacks enough information to distinguish between change and outside payments.
    ///
    /// Additionally because this wallet has no internal (change) keychain, all methods that
    /// require a [`KeychainKind`] as input, e.g. [`reveal_next_address`] should only be called
    /// using the [`External`] variant. In most cases passing [`Internal`] is treated as the
    /// equivalent of [`External`] but this behavior must not be relied on.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use bdk_wallet::Wallet;
    /// # use bitcoin::Network;
    /// # const EXTERNAL_DESC: &str = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)";
    /// # let temp_dir = tempfile::tempdir().expect("must create tempdir");
    /// # let file_path = temp_dir.path().join("store.db");
    /// // Create a wallet that is persisted to SQLite database.
    /// use bdk_wallet::rusqlite::Connection;
    /// let mut conn = Connection::open(file_path)?;
    /// let wallet = Wallet::create_single(EXTERNAL_DESC)
    ///     .network(Network::Testnet)
    ///     .create_wallet(&mut conn)?;
    /// # Ok::<_, anyhow::Error>(())
    /// ```
    /// [`change_policy`]: TxBuilder::change_policy
    /// [`do_not_spend_change`]: TxBuilder::do_not_spend_change
    /// [`External`]: KeychainKind::External
    /// [`Internal`]: KeychainKind::Internal
    /// [`reveal_next_address`]: Self::reveal_next_address
    pub fn create_single<D>(descriptor: D) -> CreateParams
    where
        D: IntoWalletDescriptor + Send + Clone + 'static,
    {
        CreateParams::new_single(descriptor)
    }

    /// Build a new [`Wallet`].
    ///
    /// If you have previously created a wallet, use [`load`](Self::load) instead.
    ///
    /// # Synopsis
    ///
    /// ```rust
    /// # use bdk_wallet::Wallet;
    /// # use bitcoin::Network;
    /// # fn main() -> anyhow::Result<()> {
    /// # const EXTERNAL_DESC: &str = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)";
    /// # const INTERNAL_DESC: &str = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/*)";
    /// // Create a non-persisted wallet.
    /// let wallet = Wallet::create(EXTERNAL_DESC, INTERNAL_DESC)
    ///     .network(Network::Testnet)
    ///     .create_wallet_no_persist()?;
    ///
    /// // Create a wallet that is persisted to SQLite database.
    /// # let temp_dir = tempfile::tempdir().expect("must create tempdir");
    /// # let file_path = temp_dir.path().join("store.db");
    /// use bdk_wallet::rusqlite::Connection;
    /// let mut conn = Connection::open(file_path)?;
    /// let wallet = Wallet::create(EXTERNAL_DESC, INTERNAL_DESC)
    ///     .network(Network::Testnet)
    ///     .create_wallet(&mut conn)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn create<D>(descriptor: D, change_descriptor: D) -> CreateParams
    where
        D: IntoWalletDescriptor + Send + Clone + 'static,
    {
        CreateParams::new(descriptor, change_descriptor)
    }

    /// Create a new [`Wallet`] with given `params`.
    ///
    /// Refer to [`Wallet::create`] for more.
    pub fn create_with_params(params: CreateParams) -> Result<Self, DescriptorError> {
        let secp = SecpCtx::new();
        let network = params.network;
        let genesis_hash = params
            .genesis_hash
            .unwrap_or(genesis_block(network).block_hash());
        let (chain, chain_changeset) = LocalChain::from_genesis_hash(genesis_hash);

        let (descriptor, mut descriptor_keymap) = (params.descriptor)(&secp, network)?;
        check_wallet_descriptor(&descriptor)?;
        descriptor_keymap.extend(params.descriptor_keymap);

        let signers = Arc::new(SignersContainer::build(
            descriptor_keymap,
            &descriptor,
            &secp,
        ));

        let (change_descriptor, change_signers) = match params.change_descriptor {
            Some(make_desc) => {
                let (change_descriptor, mut internal_keymap) = make_desc(&secp, network)?;
                check_wallet_descriptor(&change_descriptor)?;
                internal_keymap.extend(params.change_descriptor_keymap);
                let change_signers = Arc::new(SignersContainer::build(
                    internal_keymap,
                    &change_descriptor,
                    &secp,
                ));
                (Some(change_descriptor), change_signers)
            }
            None => (None, Arc::new(SignersContainer::new())),
        };

        let index = create_indexer(descriptor, change_descriptor, params.lookahead)?;

        let descriptor = index.get_descriptor(KeychainKind::External).cloned();
        let change_descriptor = index.get_descriptor(KeychainKind::Internal).cloned();
        let indexed_graph = IndexedTxGraph::new(index);
        let indexed_graph_changeset = indexed_graph.initial_changeset();

        let stage = ChangeSet {
            descriptor,
            change_descriptor,
            local_chain: chain_changeset,
            tx_graph: indexed_graph_changeset.tx_graph,
            indexer: indexed_graph_changeset.indexer,
            network: Some(network),
        };

        Ok(Wallet {
            signers,
            change_signers,
            network,
            chain,
            indexed_graph,
            stage,
            secp,
        })
    }

    /// Build [`Wallet`] by loading from persistence or [`ChangeSet`].
    ///
    /// Note that the descriptor secret keys are not persisted to the db. You can add
    /// signers after-the-fact with [`Wallet::add_signer`] or [`Wallet::set_keymap`]. You
    /// can also add keys when building the wallet by using [`LoadParams::keymap`]. Finally
    /// you can check the wallet's descriptors are what you expect with [`LoadParams::descriptor`]
    /// which will try to populate signers if [`LoadParams::extract_keys`] is enabled.
    ///
    /// # Synopsis
    ///
    /// ```rust,no_run
    /// # use bdk_wallet::{Wallet, ChangeSet, KeychainKind};
    /// # use bitcoin::{BlockHash, Network, hashes::Hash};
    /// # fn main() -> anyhow::Result<()> {
    /// # const EXTERNAL_DESC: &str = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/0/*)";
    /// # const INTERNAL_DESC: &str = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/1'/0'/1/*)";
    /// # let changeset = ChangeSet::default();
    /// // Load a wallet from changeset (no persistence).
    /// let wallet = Wallet::load()
    ///     .load_wallet_no_persist(changeset)?
    ///     .expect("must have data to load wallet");
    ///
    /// // Load a wallet that is persisted to SQLite database.
    /// # let temp_dir = tempfile::tempdir().expect("must create tempdir");
    /// # let file_path = temp_dir.path().join("store.db");
    /// # let external_keymap = Default::default();
    /// # let internal_keymap = Default::default();
    /// # let genesis_hash = BlockHash::all_zeros();
    /// let mut conn = bdk_wallet::rusqlite::Connection::open(file_path)?;
    /// let mut wallet = Wallet::load()
    ///     // check loaded descriptors matches these values and extract private keys
    ///     .descriptor(KeychainKind::External, Some(EXTERNAL_DESC))
    ///     .descriptor(KeychainKind::Internal, Some(INTERNAL_DESC))
    ///     .extract_keys()
    ///     // you can also manually add private keys
    ///     .keymap(KeychainKind::External, external_keymap)
    ///     .keymap(KeychainKind::Internal, internal_keymap)
    ///     // ensure loaded wallet's genesis hash matches this value
    ///     .check_genesis_hash(genesis_hash)
    ///     // set a lookahead for our indexer
    ///     .lookahead(101)
    ///     .load_wallet(&mut conn)?
    ///     .expect("must have data to load wallet");
    /// # Ok(())
    /// # }
    /// ```
    pub fn load() -> LoadParams {
        LoadParams::new()
    }

    /// Load [`Wallet`] from the given previously persisted [`ChangeSet`] and `params`.
    ///
    /// Refer to [`Wallet::load`] for more.
    pub fn load_with_params(
        changeset: ChangeSet,
        params: LoadParams,
    ) -> Result<Option<Self>, LoadError> {
        if changeset.is_empty() {
            return Ok(None);
        }
        let secp = Secp256k1::new();
        let network = changeset.network.ok_or(LoadError::MissingNetwork)?;
        let chain = LocalChain::from_changeset(changeset.local_chain)
            .map_err(|_| LoadError::MissingGenesis)?;

        if let Some(exp_network) = params.check_network {
            if network != exp_network {
                return Err(LoadError::Mismatch(LoadMismatch::Network {
                    loaded: network,
                    expected: exp_network,
                }));
            }
        }
        if let Some(exp_genesis_hash) = params.check_genesis_hash {
            if chain.genesis_hash() != exp_genesis_hash {
                return Err(LoadError::Mismatch(LoadMismatch::Genesis {
                    loaded: chain.genesis_hash(),
                    expected: exp_genesis_hash,
                }));
            }
        }

        let descriptor = changeset
            .descriptor
            .ok_or(LoadError::MissingDescriptor(KeychainKind::External))?;
        check_wallet_descriptor(&descriptor).map_err(LoadError::Descriptor)?;
        let mut external_keymap = params.descriptor_keymap;

        if let Some(expected) = params.check_descriptor {
            if let Some(make_desc) = expected {
                let (exp_desc, keymap) =
                    make_desc(&secp, network).map_err(LoadError::Descriptor)?;
                if descriptor.descriptor_id() != exp_desc.descriptor_id() {
                    return Err(LoadError::Mismatch(LoadMismatch::Descriptor {
                        keychain: KeychainKind::External,
                        loaded: Some(descriptor),
                        expected: Some(exp_desc),
                    }));
                }
                if params.extract_keys {
                    external_keymap.extend(keymap);
                }
            } else {
                return Err(LoadError::Mismatch(LoadMismatch::Descriptor {
                    keychain: KeychainKind::External,
                    loaded: Some(descriptor),
                    expected: None,
                }));
            }
        }
        let signers = Arc::new(SignersContainer::build(external_keymap, &descriptor, &secp));

        let mut change_descriptor = None;
        let mut internal_keymap = params.change_descriptor_keymap;

        match (changeset.change_descriptor, params.check_change_descriptor) {
            // empty signer
            (None, None) => {}
            (None, Some(expect)) => {
                // expected desc but none loaded
                if let Some(make_desc) = expect {
                    let (exp_desc, _) = make_desc(&secp, network).map_err(LoadError::Descriptor)?;
                    return Err(LoadError::Mismatch(LoadMismatch::Descriptor {
                        keychain: KeychainKind::Internal,
                        loaded: None,
                        expected: Some(exp_desc),
                    }));
                }
            }
            // nothing expected
            (Some(desc), None) => {
                check_wallet_descriptor(&desc).map_err(LoadError::Descriptor)?;
                change_descriptor = Some(desc);
            }
            (Some(desc), Some(expect)) => match expect {
                // expected none for existing
                None => {
                    return Err(LoadError::Mismatch(LoadMismatch::Descriptor {
                        keychain: KeychainKind::Internal,
                        loaded: Some(desc),
                        expected: None,
                    }))
                }
                // parameters must match
                Some(make_desc) => {
                    check_wallet_descriptor(&desc).map_err(LoadError::Descriptor)?;
                    let (exp_desc, keymap) =
                        make_desc(&secp, network).map_err(LoadError::Descriptor)?;
                    if desc.descriptor_id() != exp_desc.descriptor_id() {
                        return Err(LoadError::Mismatch(LoadMismatch::Descriptor {
                            keychain: KeychainKind::Internal,
                            loaded: Some(desc),
                            expected: Some(exp_desc),
                        }));
                    }
                    if params.extract_keys {
                        internal_keymap.extend(keymap);
                    }
                    change_descriptor = Some(desc);
                }
            },
        }

        let change_signers = match change_descriptor {
            Some(ref change_descriptor) => Arc::new(SignersContainer::build(
                internal_keymap,
                change_descriptor,
                &secp,
            )),
            None => Arc::new(SignersContainer::new()),
        };

        let index = create_indexer(descriptor, change_descriptor, params.lookahead)
            .map_err(LoadError::Descriptor)?;

        let mut indexed_graph = IndexedTxGraph::new(index);
        indexed_graph.apply_changeset(changeset.indexer.into());
        indexed_graph.apply_changeset(changeset.tx_graph.into());

        let stage = ChangeSet::default();

        Ok(Some(Wallet {
            signers,
            change_signers,
            chain,
            indexed_graph,
            stage,
            network,
            secp,
        }))
    }

    /// Get the Bitcoin network the wallet is using.
    pub fn network(&self) -> Network {
        self.network
    }

    /// Iterator over all keychains in this wallet
    pub fn keychains(&self) -> impl Iterator<Item = (KeychainKind, &ExtendedDescriptor)> {
        self.indexed_graph.index.keychains()
    }

    /// Peek an address of the given `keychain` at `index` without revealing it.
    ///
    /// For non-wildcard descriptors this returns the same address at every provided index.
    ///
    /// # Panics
    ///
    /// This panics when the caller requests for an address of derivation index greater than the
    /// [BIP32](https://github.com/bitcoin/bips/blob/master/bip-0032.mediawiki) max index.
    pub fn peek_address(&self, keychain: KeychainKind, mut index: u32) -> AddressInfo {
        let keychain = self.map_keychain(keychain);
        let mut spk_iter = self
            .indexed_graph
            .index
            .unbounded_spk_iter(keychain)
            .expect("keychain must exist");
        if !spk_iter.descriptor().has_wildcard() {
            index = 0;
        }
        let (index, spk) = spk_iter
            .nth(index as usize)
            .expect("derivation index is out of bounds");

        AddressInfo {
            index,
            address: Address::from_script(&spk, self.network).expect("must have address form"),
            keychain,
        }
    }

    /// Attempt to reveal the next address of the given `keychain`.
    ///
    /// This will increment the keychain's derivation index. If the keychain's descriptor doesn't
    /// contain a wildcard or every address is already revealed up to the maximum derivation
    /// index defined in [BIP32](https://github.com/bitcoin/bips/blob/master/bip-0032.mediawiki),
    /// then the last revealed address will be returned.
    ///
    /// **WARNING**: To avoid address reuse you must persist the changes resulting from one or more
    /// calls to this method before closing the wallet. For example:
    ///
    /// ```rust,no_run
    /// # use bdk_wallet::{LoadParams, ChangeSet, KeychainKind};
    /// use bdk_chain::rusqlite::Connection;
    /// let mut conn = Connection::open_in_memory().expect("must open connection");
    /// let mut wallet = LoadParams::new()
    ///     .load_wallet(&mut conn)
    ///     .expect("database is okay")
    ///     .expect("database has data");
    /// let next_address = wallet.reveal_next_address(KeychainKind::External);
    /// wallet.persist(&mut conn).expect("write is okay");
    ///
    /// // Now it's safe to show the user their next address!
    /// println!("Next address: {}", next_address.address);
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn reveal_next_address(&mut self, keychain: KeychainKind) -> AddressInfo {
        let keychain = self.map_keychain(keychain);
        let index = &mut self.indexed_graph.index;
        let stage = &mut self.stage;

        let ((index, spk), index_changeset) = index
            .reveal_next_spk(keychain)
            .expect("keychain must exist");

        stage.merge(index_changeset.into());

        AddressInfo {
            index,
            address: Address::from_script(spk.as_script(), self.network)
                .expect("must have address form"),
            keychain,
        }
    }

    /// Reveal addresses up to and including the target `index` and return an iterator
    /// of newly revealed addresses.
    ///
    /// If the target `index` is unreachable, we make a best effort to reveal up to the last
    /// possible index. If all addresses up to the given `index` are already revealed, then
    /// no new addresses are returned.
    ///
    /// **WARNING**: To avoid address reuse you must persist the changes resulting from one or more
    /// calls to this method before closing the wallet. See [`Wallet::reveal_next_address`].
    pub fn reveal_addresses_to(
        &mut self,
        keychain: KeychainKind,
        index: u32,
    ) -> impl Iterator<Item = AddressInfo> + '_ {
        let keychain = self.map_keychain(keychain);
        let (spks, index_changeset) = self
            .indexed_graph
            .index
            .reveal_to_target(keychain, index)
            .expect("keychain must exist");

        self.stage.merge(index_changeset.into());

        spks.into_iter().map(move |(index, spk)| AddressInfo {
            index,
            address: Address::from_script(&spk, self.network).expect("must have address form"),
            keychain,
        })
    }

    /// Get the next unused address for the given `keychain`, i.e. the address with the lowest
    /// derivation index that hasn't been used in a transaction.
    ///
    /// This will attempt to reveal a new address if all previously revealed addresses have
    /// been used, in which case the returned address will be the same as calling [`Wallet::reveal_next_address`].
    ///
    /// **WARNING**: To avoid address reuse you must persist the changes resulting from one or more
    /// calls to this method before closing the wallet. See [`Wallet::reveal_next_address`].
    pub fn next_unused_address(&mut self, keychain: KeychainKind) -> AddressInfo {
        let keychain = self.map_keychain(keychain);
        let index = &mut self.indexed_graph.index;

        let ((index, spk), index_changeset) = index
            .next_unused_spk(keychain)
            .expect("keychain must exist");

        self.stage
            .merge(indexed_tx_graph::ChangeSet::from(index_changeset).into());

        AddressInfo {
            index,
            address: Address::from_script(spk.as_script(), self.network)
                .expect("must have address form"),
            keychain,
        }
    }

    /// Marks an address used of the given `keychain` at `index`.
    ///
    /// Returns whether the given index was present and then removed from the unused set.
    pub fn mark_used(&mut self, keychain: KeychainKind, index: u32) -> bool {
        self.indexed_graph.index.mark_used(keychain, index)
    }

    /// Undoes the effect of [`mark_used`] and returns whether the `index` was inserted
    /// back into the unused set.
    ///
    /// Since this is only a superficial marker, it will have no effect if the address at the given
    /// `index` was actually used, i.e. the wallet has previously indexed a tx output for the
    /// derived spk.
    ///
    /// [`mark_used`]: Self::mark_used
    pub fn unmark_used(&mut self, keychain: KeychainKind, index: u32) -> bool {
        self.indexed_graph.index.unmark_used(keychain, index)
    }

    /// List addresses that are revealed but unused.
    ///
    /// Note if the returned iterator is empty you can reveal more addresses
    /// by using [`reveal_next_address`](Self::reveal_next_address) or
    /// [`reveal_addresses_to`](Self::reveal_addresses_to).
    pub fn list_unused_addresses(
        &self,
        keychain: KeychainKind,
    ) -> impl DoubleEndedIterator<Item = AddressInfo> + '_ {
        self.indexed_graph
            .index
            .unused_keychain_spks(self.map_keychain(keychain))
            .map(move |(index, spk)| AddressInfo {
                index,
                address: Address::from_script(spk.as_script(), self.network)
                    .expect("must have address form"),
                keychain,
            })
    }

    /// Return whether or not a `script` is part of this wallet (either internal or external)
    pub fn is_mine(&self, script: ScriptBuf) -> bool {
        self.indexed_graph.index.index_of_spk(script).is_some()
    }

    /// Finds how the wallet derived the script pubkey `spk`.
    ///
    /// Will only return `Some(_)` if the wallet has given out the spk.
    pub fn derivation_of_spk(&self, spk: ScriptBuf) -> Option<(KeychainKind, u32)> {
        self.indexed_graph.index.index_of_spk(spk).cloned()
    }

    /// Return the list of unspent outputs of this wallet
    pub fn list_unspent(&self) -> impl Iterator<Item = LocalOutput> + '_ {
        self.indexed_graph
            .graph()
            .filter_chain_unspents(
                &self.chain,
                self.chain.tip().block_id(),
                self.indexed_graph.index.outpoints().iter().cloned(),
            )
            .map(|((k, i), full_txo)| new_local_utxo(k, i, full_txo))
    }

    /// List all relevant outputs (includes both spent and unspent, confirmed and unconfirmed).
    ///
    /// To list only unspent outputs (UTXOs), use [`Wallet::list_unspent`] instead.
    pub fn list_output(&self) -> impl Iterator<Item = LocalOutput> + '_ {
        self.indexed_graph
            .graph()
            .filter_chain_txouts(
                &self.chain,
                self.chain.tip().block_id(),
                self.indexed_graph.index.outpoints().iter().cloned(),
            )
            .map(|((k, i), full_txo)| new_local_utxo(k, i, full_txo))
    }

    /// Get all the checkpoints the wallet is currently storing indexed by height.
    pub fn checkpoints(&self) -> CheckPointIter {
        self.chain.iter_checkpoints()
    }

    /// Returns the latest checkpoint.
    pub fn latest_checkpoint(&self) -> CheckPoint {
        self.chain.tip()
    }

    /// Get unbounded script pubkey iterators for both `Internal` and `External` keychains.
    ///
    /// This is intended to be used when doing a full scan of your addresses (e.g. after restoring
    /// from seed words). You pass the `BTreeMap` of iterators to a blockchain data source (e.g.
    /// electrum server) which will go through each address until it reaches a *stop gap*.
    ///
    /// Note carefully that iterators go over **all** script pubkeys on the keychains (not what
    /// script pubkeys the wallet is storing internally).
    pub fn all_unbounded_spk_iters(
        &self,
    ) -> BTreeMap<KeychainKind, impl Iterator<Item = Indexed<ScriptBuf>> + Clone> {
        self.indexed_graph.index.all_unbounded_spk_iters()
    }

    /// Get an unbounded script pubkey iterator for the given `keychain`.
    ///
    /// See [`all_unbounded_spk_iters`] for more documentation
    ///
    /// [`all_unbounded_spk_iters`]: Self::all_unbounded_spk_iters
    pub fn unbounded_spk_iter(
        &self,
        keychain: KeychainKind,
    ) -> impl Iterator<Item = Indexed<ScriptBuf>> + Clone {
        self.indexed_graph
            .index
            .unbounded_spk_iter(self.map_keychain(keychain))
            .expect("keychain must exist")
    }

    /// Returns the utxo owned by this wallet corresponding to `outpoint` if it exists in the
    /// wallet's database.
    pub fn get_utxo(&self, op: OutPoint) -> Option<LocalOutput> {
        let ((keychain, index), _) = self.indexed_graph.index.txout(op)?;
        self.indexed_graph
            .graph()
            .filter_chain_unspents(
                &self.chain,
                self.chain.tip().block_id(),
                core::iter::once(((), op)),
            )
            .map(|(_, full_txo)| new_local_utxo(keychain, index, full_txo))
            .next()
    }

    /// Inserts a [`TxOut`] at [`OutPoint`] into the wallet's transaction graph.
    ///
    /// This is used for providing a previous output's value so that we can use [`calculate_fee`]
    /// or [`calculate_fee_rate`] on a given transaction. Outputs inserted with this method will
    /// not be returned in [`list_unspent`] or [`list_output`].
    ///
    /// **WARNINGS:** This should only be used to add `TxOut`s that the wallet does not own. Only
    /// insert `TxOut`s that you trust the values for!
    ///
    /// You must persist the changes resulting from one or more calls to this method if you need
    /// the inserted `TxOut` data to be reloaded after closing the wallet.
    /// See [`Wallet::reveal_next_address`].
    ///
    /// [`calculate_fee`]: Self::calculate_fee
    /// [`calculate_fee_rate`]: Self::calculate_fee_rate
    /// [`list_unspent`]: Self::list_unspent
    /// [`list_output`]: Self::list_output
    pub fn insert_txout(&mut self, outpoint: OutPoint, txout: TxOut) {
        let additions = self.indexed_graph.insert_txout(outpoint, txout);
        self.stage.merge(additions.into());
    }

    /// Calculates the fee of a given transaction. Returns [`Amount::ZERO`] if `tx` is a coinbase transaction.
    ///
    /// To calculate the fee for a [`Transaction`] with inputs not owned by this wallet you must
    /// manually insert the TxOut(s) into the tx graph using the [`insert_txout`] function.
    ///
    /// Note `tx` does not have to be in the graph for this to work.
    ///
    /// # Examples
    ///
    /// ```rust, no_run
    /// # use bitcoin::Txid;
    /// # use bdk_wallet::Wallet;
    /// # let mut wallet: Wallet = todo!();
    /// # let txid:Txid = todo!();
    /// let tx = wallet.get_tx(txid).expect("transaction").tx_node.tx;
    /// let fee = wallet.calculate_fee(&tx).expect("fee");
    /// ```
    ///
    /// ```rust, no_run
    /// # use bitcoin::Psbt;
    /// # use bdk_wallet::Wallet;
    /// # let mut wallet: Wallet = todo!();
    /// # let mut psbt: Psbt = todo!();
    /// let tx = &psbt.clone().extract_tx().expect("tx");
    /// let fee = wallet.calculate_fee(tx).expect("fee");
    /// ```
    /// [`insert_txout`]: Self::insert_txout
    pub fn calculate_fee(&self, tx: &Transaction) -> Result<Amount, CalculateFeeError> {
        self.indexed_graph.graph().calculate_fee(tx)
    }

    /// Calculate the [`FeeRate`] for a given transaction.
    ///
    /// To calculate the fee rate for a [`Transaction`] with inputs not owned by this wallet you must
    /// manually insert the TxOut(s) into the tx graph using the [`insert_txout`] function.
    ///
    /// Note `tx` does not have to be in the graph for this to work.
    ///
    /// # Examples
    ///
    /// ```rust, no_run
    /// # use bitcoin::Txid;
    /// # use bdk_wallet::Wallet;
    /// # let mut wallet: Wallet = todo!();
    /// # let txid:Txid = todo!();
    /// let tx = wallet.get_tx(txid).expect("transaction").tx_node.tx;
    /// let fee_rate = wallet.calculate_fee_rate(&tx).expect("fee rate");
    /// ```
    ///
    /// ```rust, no_run
    /// # use bitcoin::Psbt;
    /// # use bdk_wallet::Wallet;
    /// # let mut wallet: Wallet = todo!();
    /// # let mut psbt: Psbt = todo!();
    /// let tx = &psbt.clone().extract_tx().expect("tx");
    /// let fee_rate = wallet.calculate_fee_rate(tx).expect("fee rate");
    /// ```
    /// [`insert_txout`]: Self::insert_txout
    pub fn calculate_fee_rate(&self, tx: &Transaction) -> Result<FeeRate, CalculateFeeError> {
        self.calculate_fee(tx).map(|fee| fee / tx.weight())
    }

    /// Compute the `tx`'s sent and received [`Amount`]s.
    ///
    /// This method returns a tuple `(sent, received)`. Sent is the sum of the txin amounts
    /// that spend from previous txouts tracked by this wallet. Received is the summation
    /// of this tx's outputs that send to script pubkeys tracked by this wallet.
    ///
    /// # Examples
    ///
    /// ```rust, no_run
    /// # use bitcoin::Txid;
    /// # use bdk_wallet::Wallet;
    /// # let mut wallet: Wallet = todo!();
    /// # let txid:Txid = todo!();
    /// let tx = wallet.get_tx(txid).expect("tx exists").tx_node.tx;
    /// let (sent, received) = wallet.sent_and_received(&tx);
    /// ```
    ///
    /// ```rust, no_run
    /// # use bitcoin::Psbt;
    /// # use bdk_wallet::Wallet;
    /// # let mut wallet: Wallet = todo!();
    /// # let mut psbt: Psbt = todo!();
    /// let tx = &psbt.clone().extract_tx().expect("tx");
    /// let (sent, received) = wallet.sent_and_received(tx);
    /// ```
    pub fn sent_and_received(&self, tx: &Transaction) -> (Amount, Amount) {
        self.indexed_graph.index.sent_and_received(tx, ..)
    }

    /// Get a single transaction from the wallet as a [`WalletTx`] (if the transaction exists).
    ///
    /// `WalletTx` contains the full transaction alongside meta-data such as:
    /// * Blocks that the transaction is [`Anchor`]ed in. These may or may not be blocks that exist
    ///   in the best chain.
    /// * The [`ChainPosition`] of the transaction in the best chain - whether the transaction is
    ///   confirmed or unconfirmed. If the transaction is confirmed, the anchor which proves the
    ///   confirmation is provided. If the transaction is unconfirmed, the unix timestamp of when
    ///   the transaction was last seen in the mempool is provided.
    ///
    /// ```rust, no_run
    /// use bdk_chain::Anchor;
    /// use bdk_wallet::{chain::ChainPosition, Wallet};
    /// # let wallet: Wallet = todo!();
    /// # let my_txid: bitcoin::Txid = todo!();
    ///
    /// let wallet_tx = wallet.get_tx(my_txid).expect("panic if tx does not exist");
    ///
    /// // get reference to full transaction
    /// println!("my tx: {:#?}", wallet_tx.tx_node.tx);
    ///
    /// // list all transaction anchors
    /// for anchor in wallet_tx.tx_node.anchors {
    ///     println!(
    ///         "tx is anchored by block of hash {}",
    ///         anchor.anchor_block().hash
    ///     );
    /// }
    ///
    /// // get confirmation status of transaction
    /// match wallet_tx.chain_position {
    ///     ChainPosition::Confirmed {
    ///         anchor,
    ///         transitively: None,
    ///     } => println!(
    ///         "tx is confirmed at height {}, we know this since {}:{} is in the best chain",
    ///         anchor.block_id.height, anchor.block_id.height, anchor.block_id.hash,
    ///     ),
    ///     ChainPosition::Confirmed {
    ///         anchor,
    ///         transitively: Some(_),
    ///     } => println!(
    ///         "tx is an ancestor of a tx anchored in {}:{}",
    ///         anchor.block_id.height, anchor.block_id.hash,
    ///     ),
    ///     ChainPosition::Unconfirmed { last_seen } => println!(
    ///         "tx is last seen at {:?}, it is unconfirmed as it is not anchored in the best chain",
    ///         last_seen,
    ///     ),
    /// }
    /// ```
    ///
    /// [`Anchor`]: bdk_chain::Anchor
    pub fn get_tx(&self, txid: Txid) -> Option<WalletTx> {
        let graph = self.indexed_graph.graph();
        graph
            .list_canonical_txs(&self.chain, self.chain.tip().block_id())
            .find(|tx| tx.tx_node.txid == txid)
    }

    /// Iterate over relevant and canonical transactions in the wallet.
    ///
    /// A transaction is relevant when it spends from or spends to at least one tracked output. A
    /// transaction is canonical when it is confirmed in the best chain, or does not conflict
    /// with any transaction confirmed in the best chain.
    ///
    /// To iterate over all transactions, including those that are irrelevant and not canonical, use
    /// [`TxGraph::full_txs`].
    ///
    /// To iterate over all canonical transactions, including those that are irrelevant, use
    /// [`TxGraph::list_canonical_txs`].
    pub fn transactions(&self) -> impl Iterator<Item = WalletTx> + '_ {
        let tx_graph = self.indexed_graph.graph();
        let tx_index = &self.indexed_graph.index;
        tx_graph
            .list_canonical_txs(&self.chain, self.chain.tip().block_id())
            .filter(|c_tx| tx_index.is_tx_relevant(&c_tx.tx_node.tx))
    }

    /// Array of relevant and canonical transactions in the wallet sorted with a comparator
    /// function.
    ///
    /// This is a helper method equivalent to collecting the result of [`Wallet::transactions`]
    /// into a [`Vec`] and then sorting it.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use bdk_wallet::{LoadParams, Wallet, WalletTx};
    /// # let mut wallet:Wallet = todo!();
    /// // Transactions by chain position: first unconfirmed then descending by confirmed height.
    /// let sorted_txs: Vec<WalletTx> =
    ///     wallet.transactions_sort_by(|tx1, tx2| tx2.chain_position.cmp(&tx1.chain_position));
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn transactions_sort_by<F>(&self, compare: F) -> Vec<WalletTx>
    where
        F: FnMut(&WalletTx, &WalletTx) -> Ordering,
    {
        let mut txs: Vec<WalletTx> = self.transactions().collect();
        txs.sort_unstable_by(compare);
        txs
    }

    /// Return the balance, separated into available, trusted-pending, untrusted-pending and immature
    /// values.
    pub fn balance(&self) -> Balance {
        self.indexed_graph.graph().balance(
            &self.chain,
            self.chain.tip().block_id(),
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

    /// Set the keymap for a given keychain.
    ///
    /// Note this does nothing if the given keychain has no descriptor because we won't
    /// know the context (segwit, taproot, etc) in which to create signatures.
    pub fn set_keymap(&mut self, keychain: KeychainKind, keymap: KeyMap) {
        let wallet_signers = match keychain {
            KeychainKind::External => Arc::make_mut(&mut self.signers),
            KeychainKind::Internal => Arc::make_mut(&mut self.change_signers),
        };
        if let Some(descriptor) = self.indexed_graph.index.get_descriptor(keychain) {
            *wallet_signers = SignersContainer::build(keymap, descriptor, &self.secp)
        }
    }

    /// Set the keymap for each keychain.
    pub fn set_keymaps(&mut self, keymaps: impl IntoIterator<Item = (KeychainKind, KeyMap)>) {
        for (keychain, keymap) in keymaps {
            self.set_keymap(keychain, keymap);
        }
    }

    /// Get the signers
    ///
    /// ## Example
    ///
    /// ```
    /// # use bdk_wallet::{Wallet, KeychainKind};
    /// # use bdk_wallet::bitcoin::Network;
    /// let descriptor = "wpkh(tprv8ZgxMBicQKsPe73PBRSmNbTfbcsZnwWhz5eVmhHpi31HW29Z7mc9B4cWGRQzopNUzZUT391DeDJxL2PefNunWyLgqCKRMDkU1s2s8bAfoSk/84'/1'/0'/0/*)";
    /// let change_descriptor = "wpkh(tprv8ZgxMBicQKsPe73PBRSmNbTfbcsZnwWhz5eVmhHpi31HW29Z7mc9B4cWGRQzopNUzZUT391DeDJxL2PefNunWyLgqCKRMDkU1s2s8bAfoSk/84'/1'/0'/1/*)";
    /// let wallet = Wallet::create(descriptor, change_descriptor)
    ///     .network(Network::Testnet)
    ///     .create_wallet_no_persist()?;
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
    /// # use bdk_wallet::*;
    /// # use bdk_wallet::ChangeSet;
    /// # use bdk_wallet::error::CreateTxError;
    /// # use anyhow::Error;
    /// # let descriptor = "wpkh(tpubD6NzVbkrYhZ4Xferm7Pz4VnjdcDPFyjVu5K4iZXQ4pVN8Cks4pHVowTBXBKRhX64pkRyJZJN5xAKj4UDNnLPb5p2sSKXhewoYx5GbTdUFWq/*)";
    /// # let mut wallet = doctest_wallet!();
    /// # let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap().assume_checked();
    /// let psbt = {
    ///    let mut builder =  wallet.build_tx();
    ///    builder
    ///        .add_recipient(to_address.script_pubkey(), Amount::from_sat(50_000));
    ///    builder.finish()?
    /// };
    ///
    /// // sign and broadcast ...
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    ///
    /// [`TxBuilder`]: crate::TxBuilder
    pub fn build_tx(&mut self) -> TxBuilder<'_, DefaultCoinSelectionAlgorithm> {
        TxBuilder {
            wallet: self,
            params: TxParams::default(),
            coin_selection: DefaultCoinSelectionAlgorithm::default(),
        }
    }

    pub(crate) fn create_tx<Cs: coin_selection::CoinSelectionAlgorithm>(
        &mut self,
        coin_selection: Cs,
        params: TxParams,
        rng: &mut impl RngCore,
    ) -> Result<Psbt, CreateTxError> {
        let keychains: BTreeMap<_, _> = self.indexed_graph.index.keychains().collect();
        let external_descriptor = keychains.get(&KeychainKind::External).expect("must exist");
        let internal_descriptor = keychains.get(&KeychainKind::Internal);

        let external_policy = external_descriptor
            .extract_policy(&self.signers, BuildSatisfaction::None, &self.secp)?
            .unwrap();
        let internal_policy = internal_descriptor
            .map(|desc| {
                Ok::<_, CreateTxError>(
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
            return Err(CreateTxError::SpendingPolicyRequired(
                KeychainKind::External,
            ));
        };
        // Same for the internal_policy path
        if let Some(internal_policy) = &internal_policy {
            if params.change_policy != tx_builder::ChangeSpendPolicy::ChangeForbidden
                && internal_policy.requires_path()
                && params.internal_policy_path.is_none()
            {
                return Err(CreateTxError::SpendingPolicyRequired(
                    KeychainKind::Internal,
                ));
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
                Ok::<_, CreateTxError>(
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

        let version = match params.version {
            Some(transaction::Version(0)) => return Err(CreateTxError::Version0),
            Some(transaction::Version::ONE) if requirements.csv.is_some() => {
                return Err(CreateTxError::Version1Csv)
            }
            Some(v) => v,
            None => transaction::Version::TWO,
        };

        // We use a match here instead of a unwrap_or_else as it's way more readable :)
        let current_height = match params.current_height {
            // If they didn't tell us the current height, we assume it's the latest sync height.
            None => {
                let tip_height = self.chain.tip().height();
                absolute::LockTime::from_height(tip_height).expect("invalid height")
            }
            Some(h) => h,
        };

        let lock_time = match params.locktime {
            // When no nLockTime is specified, we try to prevent fee sniping, if possible
            None => {
                // Fee sniping can be partially prevented by setting the timelock
                // to current_height. If we don't know the current_height,
                // we default to 0.
                let fee_sniping_height = current_height;

                // We choose the biggest between the required nlocktime and the fee sniping
                // height
                match requirements.timelock {
                    // No requirement, just use the fee_sniping_height
                    None => fee_sniping_height,
                    // There's a block-based requirement, but the value is lower than the fee_sniping_height
                    Some(value @ absolute::LockTime::Blocks(_)) if value < fee_sniping_height => {
                        fee_sniping_height
                    }
                    // There's a time-based requirement or a block-based requirement greater
                    // than the fee_sniping_height use that value
                    Some(value) => value,
                }
            }
            // Specific nLockTime required and we have no constraints, so just set to that value
            Some(x) if requirements.timelock.is_none() => x,
            // Specific nLockTime required and it's compatible with the constraints
            Some(x)
                if requirements.timelock.unwrap().is_same_unit(x)
                    && x >= requirements.timelock.unwrap() =>
            {
                x
            }
            // Invalid nLockTime required
            Some(x) => {
                return Err(CreateTxError::LockTime {
                    requested: x,
                    required: requirements.timelock.unwrap(),
                })
            }
        };

        // nSequence value for inputs
        // When not explicitly specified, defaults to 0xFFFFFFFD,
        // meaning RBF signaling is enabled
        let n_sequence = match (params.sequence, requirements.csv) {
            // Enable RBF by default
            (None, None) => Sequence::ENABLE_RBF_NO_LOCKTIME,
            // None requested, use required
            (None, Some(csv)) => csv,
            // Requested sequence is incompatible with requirements
            (Some(sequence), Some(csv)) if !check_nsequence_rbf(sequence, csv) => {
                return Err(CreateTxError::RbfSequenceCsv { sequence, csv })
            }
            // Use requested nSequence value
            (Some(sequence), _) => sequence,
        };

        let (fee_rate, mut fee_amount) = match params.fee_policy.unwrap_or_default() {
            //FIXME: see https://github.com/bitcoindevkit/bdk/issues/256
            FeePolicy::FeeAmount(fee) => {
                if let Some(previous_fee) = params.bumping_fee {
                    if fee < previous_fee.absolute {
                        return Err(CreateTxError::FeeTooLow {
                            required: previous_fee.absolute,
                        });
                    }
                }
                (FeeRate::ZERO, fee)
            }
            FeePolicy::FeeRate(rate) => {
                if let Some(previous_fee) = params.bumping_fee {
                    let required_feerate = FeeRate::from_sat_per_kwu(
                        previous_fee.rate.to_sat_per_kwu()
                            + FeeRate::BROADCAST_MIN.to_sat_per_kwu(), // +1 sat/vb
                    );
                    if rate < required_feerate {
                        return Err(CreateTxError::FeeRateTooLow {
                            required: required_feerate,
                        });
                    }
                }
                (rate, Amount::ZERO)
            }
        };

        let mut tx = Transaction {
            version,
            lock_time,
            input: vec![],
            output: vec![],
        };

        if params.manually_selected_only && params.utxos.is_empty() {
            return Err(CreateTxError::NoUtxosSelected);
        }

        let mut outgoing = Amount::ZERO;
        let recipients = params.recipients.iter().map(|(r, v)| (r, *v));

        for (index, (script_pubkey, value)) in recipients.enumerate() {
            if !params.allow_dust && value.is_dust(script_pubkey) && !script_pubkey.is_op_return() {
                return Err(CreateTxError::OutputBelowDustLimit(index));
            }

            let new_out = TxOut {
                script_pubkey: script_pubkey.clone(),
                value,
            };

            tx.output.push(new_out);

            outgoing += value;
        }

        fee_amount += fee_rate * tx.weight();

        let (required_utxos, optional_utxos) = {
            // NOTE: manual selection overrides unspendable
            let mut required: Vec<WeightedUtxo> = params.utxos.values().cloned().collect();
            let optional = self.filter_utxos(&params, current_height.to_consensus_u32());

            // if drain_wallet is true, all UTxOs are required
            if params.drain_wallet {
                required.extend(optional);
                (required, vec![])
            } else {
                (required, optional)
            }
        };

        // get drain script
        let mut drain_index = Option::<(KeychainKind, u32)>::None;
        let drain_script = match params.drain_to {
            Some(ref drain_recipient) => drain_recipient.clone(),
            None => {
                let change_keychain = self.map_keychain(KeychainKind::Internal);
                let (index, spk) = self
                    .indexed_graph
                    .index
                    .unused_keychain_spks(change_keychain)
                    .next()
                    .unwrap_or_else(|| {
                        let (next_index, _) = self
                            .indexed_graph
                            .index
                            .next_index(change_keychain)
                            .expect("keychain must exist");
                        let spk = self
                            .peek_address(change_keychain, next_index)
                            .script_pubkey();
                        (next_index, spk)
                    });
                drain_index = Some((change_keychain, index));
                spk
            }
        };

        let coin_selection = coin_selection
            .coin_select(
                required_utxos,
                optional_utxos,
                fee_rate,
                outgoing + fee_amount,
                &drain_script,
                rng,
            )
            .map_err(CreateTxError::CoinSelection)?;

        let excess = &coin_selection.excess;
        tx.input = coin_selection
            .selected
            .iter()
            .map(|u| bitcoin::TxIn {
                previous_output: u.outpoint(),
                script_sig: ScriptBuf::default(),
                sequence: u.sequence().unwrap_or(n_sequence),
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
                if let Excess::NoChange {
                    dust_threshold,
                    remaining_amount,
                    change_fee,
                } = excess
                {
                    return Err(CreateTxError::CoinSelection(InsufficientFunds {
                        needed: *dust_threshold,
                        available: remaining_amount
                            .checked_sub(*change_fee)
                            .unwrap_or_default(),
                    }));
                }
            } else {
                return Err(CreateTxError::NoRecipients);
            }
        }

        // if there's change, create and add a change output
        if let Excess::Change { amount, .. } = excess {
            // create drain output
            let drain_output = TxOut {
                value: *amount,
                script_pubkey: drain_script,
            };

            // TODO: We should pay attention when adding a new output: this might increase
            // the length of the "number of vouts" parameter by 2 bytes, potentially making
            // our feerate too low
            tx.output.push(drain_output);
        }

        // sort input/outputs according to the chosen algorithm
        params.ordering.sort_tx_with_aux_rand(&mut tx, rng);

        let psbt = self.complete_transaction(tx, coin_selection.selected, params)?;

        // recording changes to the change keychain
        if let (Excess::Change { .. }, Some((keychain, index))) = (excess, drain_index) {
            let (_, index_changeset) = self
                .indexed_graph
                .index
                .reveal_to_target(keychain, index)
                .expect("must not be None");
            self.stage.merge(index_changeset.into());
            self.mark_used(keychain, index);
        }

        Ok(psbt)
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
    /// # use bdk_wallet::*;
    /// # use bdk_wallet::ChangeSet;
    /// # use bdk_wallet::error::CreateTxError;
    /// # use anyhow::Error;
    /// # let descriptor = "wpkh(tpubD6NzVbkrYhZ4Xferm7Pz4VnjdcDPFyjVu5K4iZXQ4pVN8Cks4pHVowTBXBKRhX64pkRyJZJN5xAKj4UDNnLPb5p2sSKXhewoYx5GbTdUFWq/*)";
    /// # let mut wallet = doctest_wallet!();
    /// # let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap().assume_checked();
    /// let mut psbt = {
    ///     let mut builder = wallet.build_tx();
    ///     builder
    ///         .add_recipient(to_address.script_pubkey(), Amount::from_sat(50_000));
    ///     builder.finish()?
    /// };
    /// let _ = wallet.sign(&mut psbt, SignOptions::default())?;
    /// let tx = psbt.clone().extract_tx().expect("tx");
    /// // broadcast tx but it's taking too long to confirm so we want to bump the fee
    /// let mut psbt =  {
    ///     let mut builder = wallet.build_fee_bump(tx.compute_txid())?;
    ///     builder
    ///         .fee_rate(FeeRate::from_sat_per_vb(5).expect("valid feerate"));
    ///     builder.finish()?
    /// };
    ///
    /// let _ = wallet.sign(&mut psbt, SignOptions::default())?;
    /// let fee_bumped_tx = psbt.extract_tx();
    /// // broadcast fee_bumped_tx to replace original
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    // TODO: support for merging multiple transactions while bumping the fees
    pub fn build_fee_bump(
        &mut self,
        txid: Txid,
    ) -> Result<TxBuilder<'_, DefaultCoinSelectionAlgorithm>, BuildFeeBumpError> {
        let graph = self.indexed_graph.graph();
        let txout_index = &self.indexed_graph.index;
        let chain_tip = self.chain.tip().block_id();
        let chain_positions = graph
            .list_canonical_txs(&self.chain, chain_tip)
            .map(|canon_tx| (canon_tx.tx_node.txid, canon_tx.chain_position))
            .collect::<HashMap<Txid, _>>();

        let mut tx = graph
            .get_tx(txid)
            .ok_or(BuildFeeBumpError::TransactionNotFound(txid))?
            .as_ref()
            .clone();

        if chain_positions
            .get(&txid)
            .ok_or(BuildFeeBumpError::TransactionNotFound(txid))?
            .is_confirmed()
        {
            return Err(BuildFeeBumpError::TransactionConfirmed(txid));
        }

        if !tx
            .input
            .iter()
            .any(|txin| txin.sequence.to_consensus_u32() <= 0xFFFFFFFD)
        {
            return Err(BuildFeeBumpError::IrreplaceableTransaction(
                tx.compute_txid(),
            ));
        }

        let fee = self
            .calculate_fee(&tx)
            .map_err(|_| BuildFeeBumpError::FeeRateUnavailable)?;
        let fee_rate = self
            .calculate_fee_rate(&tx)
            .map_err(|_| BuildFeeBumpError::FeeRateUnavailable)?;

        // remove the inputs from the tx and process them
        let utxos = tx
            .input
            .drain(..)
            .map(|txin| -> Result<_, BuildFeeBumpError> {
                graph
                    // Get previous transaction
                    .get_tx(txin.previous_output.txid)
                    .ok_or(BuildFeeBumpError::UnknownUtxo(txin.previous_output))
                    // Get chain position
                    .and_then(|prev_tx| {
                        chain_positions
                            .get(&txin.previous_output.txid)
                            .cloned()
                            .ok_or(BuildFeeBumpError::UnknownUtxo(txin.previous_output))
                            .map(|chain_position| (prev_tx, chain_position))
                    })
                    .map(|(prev_tx, chain_position)| {
                        let txout = prev_tx.output[txin.previous_output.vout as usize].clone();
                        match txout_index.index_of_spk(txout.script_pubkey.clone()) {
                            Some(&(keychain, derivation_index)) => (
                                txin.previous_output,
                                WeightedUtxo {
                                    satisfaction_weight: self
                                        .public_descriptor(keychain)
                                        .max_weight_to_satisfy()
                                        .unwrap(),
                                    utxo: Utxo::Local(LocalOutput {
                                        outpoint: txin.previous_output,
                                        txout: txout.clone(),
                                        keychain,
                                        is_spent: true,
                                        derivation_index,
                                        chain_position,
                                    }),
                                },
                            ),
                            None => {
                                let satisfaction_weight = Weight::from_wu_usize(
                                    serialize(&txin.script_sig).len() * 4
                                        + serialize(&txin.witness).len(),
                                );

                                (
                                    txin.previous_output,
                                    WeightedUtxo {
                                        utxo: Utxo::Foreign {
                                            outpoint: txin.previous_output,
                                            sequence: txin.sequence,
                                            psbt_input: Box::new(psbt::Input {
                                                witness_utxo: txout
                                                    .script_pubkey
                                                    .witness_version()
                                                    .map(|_| txout.clone()),
                                                non_witness_utxo: Some(prev_tx.as_ref().clone()),
                                                ..Default::default()
                                            }),
                                        },
                                        satisfaction_weight,
                                    },
                                )
                            }
                        }
                    })
            })
            .collect::<Result<HashMap<OutPoint, WeightedUtxo>, BuildFeeBumpError>>()?;

        if tx.output.len() > 1 {
            let mut change_index = None;
            for (index, txout) in tx.output.iter().enumerate() {
                let change_keychain = self.map_keychain(KeychainKind::Internal);
                match txout_index.index_of_spk(txout.script_pubkey.clone()) {
                    Some((keychain, _)) if *keychain == change_keychain => {
                        change_index = Some(index)
                    }
                    _ => {}
                }
            }

            if let Some(change_index) = change_index {
                tx.output.remove(change_index);
            }
        }

        let params = TxParams {
            // TODO: figure out what rbf option should be?
            version: Some(tx.version),
            recipients: tx
                .output
                .into_iter()
                .map(|txout| (txout.script_pubkey, txout.value))
                .collect(),
            utxos,
            bumping_fee: Some(tx_builder::PreviousFee {
                absolute: fee,
                rate: fee_rate,
            }),
            ..Default::default()
        };

        Ok(TxBuilder {
            wallet: self,
            params,
            coin_selection: DefaultCoinSelectionAlgorithm::default(),
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
    /// # use bdk_wallet::*;
    /// # use bdk_wallet::ChangeSet;
    /// # use bdk_wallet::error::CreateTxError;
    /// # let descriptor = "wpkh(tpubD6NzVbkrYhZ4Xferm7Pz4VnjdcDPFyjVu5K4iZXQ4pVN8Cks4pHVowTBXBKRhX64pkRyJZJN5xAKj4UDNnLPb5p2sSKXhewoYx5GbTdUFWq/*)";
    /// # let mut wallet = doctest_wallet!();
    /// # let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap().assume_checked();
    /// let mut psbt = {
    ///     let mut builder = wallet.build_tx();
    ///     builder.add_recipient(to_address.script_pubkey(), Amount::from_sat(50_000));
    ///     builder.finish()?
    /// };
    /// let finalized = wallet.sign(&mut psbt, SignOptions::default())?;
    /// assert!(finalized, "we should have signed all the inputs");
    /// # Ok::<(),anyhow::Error>(())
    pub fn sign(&self, psbt: &mut Psbt, sign_options: SignOptions) -> Result<bool, SignerError> {
        // This adds all the PSBT metadata for the inputs, which will help us later figure out how
        // to derive our keys
        self.update_psbt_with_descriptor(psbt)
            .map_err(SignerError::MiniscriptPsbt)?;

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
            return Err(SignerError::MissingNonWitnessUtxo);
        }

        // If the user hasn't explicitly opted-in, refuse to sign the transaction unless every input
        // is using `SIGHASH_ALL` or `SIGHASH_DEFAULT` for taproot
        if !sign_options.allow_all_sighashes
            && !psbt.inputs.iter().all(|i| {
                i.sighash_type.is_none()
                    || i.sighash_type == Some(EcdsaSighashType::All.into())
                    || i.sighash_type == Some(TapSighashType::All.into())
                    || i.sighash_type == Some(TapSighashType::Default.into())
            })
        {
            return Err(SignerError::NonStandardSighash);
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
    pub fn policies(&self, keychain: KeychainKind) -> Result<Option<Policy>, DescriptorError> {
        let signers = match keychain {
            KeychainKind::External => &self.signers,
            KeychainKind::Internal => &self.change_signers,
        };

        self.public_descriptor(keychain).extract_policy(
            signers,
            BuildSatisfaction::None,
            &self.secp,
        )
    }

    /// Returns the descriptor used to create addresses for a particular `keychain`.
    ///
    /// It's the "public" version of the wallet's descriptor, meaning a new descriptor that has
    /// the same structure but with the all secret keys replaced by their corresponding public key.
    /// This can be used to build a watch-only version of a wallet.
    pub fn public_descriptor(&self, keychain: KeychainKind) -> &ExtendedDescriptor {
        self.indexed_graph
            .index
            .get_descriptor(self.map_keychain(keychain))
            .expect("keychain must exist")
    }

    /// Finalize a PSBT, i.e., for each input determine if sufficient data is available to pass
    /// validation and construct the respective `scriptSig` or `scriptWitness`. Please refer to
    /// [BIP174](https://github.com/bitcoin/bips/blob/master/bip-0174.mediawiki#Input_Finalizer),
    /// and [BIP371](https://github.com/bitcoin/bips/blob/master/bip-0371.mediawiki)
    /// for further information.
    ///
    /// Returns `true` if the PSBT could be finalized, and `false` otherwise.
    ///
    /// The [`SignOptions`] can be used to tweak the behavior of the finalizer.
    pub fn finalize_psbt(
        &self,
        psbt: &mut Psbt,
        sign_options: SignOptions,
    ) -> Result<bool, SignerError> {
        let tx = &psbt.unsigned_tx;
        let chain_tip = self.chain.tip().block_id();
        let prev_txids = tx
            .input
            .iter()
            .map(|txin| txin.previous_output.txid)
            .collect::<HashSet<Txid>>();
        let confirmation_heights = self
            .indexed_graph
            .graph()
            .list_canonical_txs(&self.chain, chain_tip)
            .filter(|canon_tx| prev_txids.contains(&canon_tx.tx_node.txid))
            // This is for a small performance gain. Although `.filter` filters out excess txs, it
            // will still consume the internal `CanonicalIter` entirely. Having a `.take` here
            // allows us to stop further unnecessary canonicalization.
            .take(prev_txids.len())
            .map(|canon_tx| {
                let txid = canon_tx.tx_node.txid;
                match canon_tx.chain_position {
                    ChainPosition::Confirmed { anchor, .. } => (txid, anchor.block_id.height),
                    ChainPosition::Unconfirmed { .. } => (txid, u32::MAX),
                }
            })
            .collect::<HashMap<Txid, u32>>();

        let mut finished = true;

        for (n, input) in tx.input.iter().enumerate() {
            let psbt_input = &psbt
                .inputs
                .get(n)
                .ok_or(SignerError::InputIndexOutOfRange)?;
            if psbt_input.final_script_sig.is_some() || psbt_input.final_script_witness.is_some() {
                continue;
            }
            let confirmation_height = confirmation_heights
                .get(&input.previous_output.txid)
                .copied();
            let current_height = sign_options
                .assume_height
                .unwrap_or_else(|| self.chain.tip().height());

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
                    self.indexed_graph.index.keychains().find_map(|(_, desc)| {
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
                            After::new(Some(current_height), false),
                            Older::new(Some(current_height), confirmation_height, false),
                        ),
                    ) {
                        Ok(_) => {
                            // Set the UTXO fields, final script_sig and witness
                            // and clear everything else.
                            let psbt_input = psbt
                                .inputs
                                .get_mut(n)
                                .ok_or(SignerError::InputIndexOutOfRange)?;
                            let original = mem::take(psbt_input);
                            psbt_input.non_witness_utxo = original.non_witness_utxo;
                            psbt_input.witness_utxo = original.witness_utxo;
                            if !tmp_input.script_sig.is_empty() {
                                psbt_input.final_script_sig = Some(tmp_input.script_sig);
                            }
                            if !tmp_input.witness.is_empty() {
                                psbt_input.final_script_witness = Some(tmp_input.witness);
                            }
                        }
                        Err(_) => finished = false,
                    }
                }
                None => finished = false,
            }
        }

        // Clear derivation paths from outputs
        if finished {
            for output in &mut psbt.outputs {
                output.bip32_derivation.clear();
                output.tap_key_origins.clear();
            }
        }

        Ok(finished)
    }

    /// Return the secp256k1 context used for all signing operations
    pub fn secp_ctx(&self) -> &SecpCtx {
        &self.secp
    }

    /// The derivation index of this wallet. It will return `None` if it has not derived any addresses.
    /// Otherwise, it will return the index of the highest address it has derived.
    pub fn derivation_index(&self, keychain: KeychainKind) -> Option<u32> {
        self.indexed_graph.index.last_revealed_index(keychain)
    }

    /// The index of the next address that you would get if you were to ask the wallet for a new address
    pub fn next_derivation_index(&self, keychain: KeychainKind) -> u32 {
        self.indexed_graph
            .index
            .next_index(self.map_keychain(keychain))
            .expect("keychain must exist")
            .0
    }

    /// Informs the wallet that you no longer intend to broadcast a tx that was built from it.
    ///
    /// This frees up the change address used when creating the tx for use in future transactions.
    // TODO: Make this free up reserved utxos when that's implemented
    pub fn cancel_tx(&mut self, tx: &Transaction) {
        let txout_index = &mut self.indexed_graph.index;
        for txout in &tx.output {
            if let Some((keychain, index)) = txout_index.index_of_spk(txout.script_pubkey.clone()) {
                // NOTE: unmark_used will **not** make something unused if it has actually been used
                // by a tx in the tracker. It only removes the superficial marking.
                txout_index.unmark_used(*keychain, *index);
            }
        }
    }

    fn get_descriptor_for_txout(&self, txout: &TxOut) -> Option<DerivedDescriptor> {
        let &(keychain, child) = self
            .indexed_graph
            .index
            .index_of_spk(txout.script_pubkey.clone())?;
        let descriptor = self.public_descriptor(keychain);
        descriptor.at_derivation_index(child).ok()
    }

    /// Given the options returns the list of utxos that must be used to form the
    /// transaction and any further that may be used if needed.
    fn filter_utxos(&self, params: &TxParams, current_height: u32) -> Vec<WeightedUtxo> {
        if params.manually_selected_only {
            vec![]
        // only process optional UTxOs if manually_selected_only is false
        } else {
            self.indexed_graph
                .graph()
                // get all unspent UTxOs from wallet
                // NOTE: the UTxOs returned by the following method already belong to wallet as the
                // call chain uses get_tx_node infallibly
                .filter_chain_unspents(
                    &self.chain,
                    self.chain.tip().block_id(),
                    self.indexed_graph.index.outpoints().iter().cloned(),
                )
                // only create LocalOutput if UTxO is mature
                .filter_map(move |((k, i), full_txo)| {
                    full_txo
                        .is_mature(current_height)
                        .then(|| new_local_utxo(k, i, full_txo))
                })
                // only process UTxOs not selected manually, they will be considered later in the chain
                // NOTE: this avoid UTxOs in both required and optional list
                .filter(|may_spend| !params.utxos.contains_key(&may_spend.outpoint))
                // only add to optional UTxOs those which satisfy the change policy if we reuse change
                .filter(|local_output| {
                    self.keychains().count() == 1
                        || params.change_policy.is_satisfied_by(local_output)
                })
                // only add to optional UTxOs those marked as spendable
                .filter(|local_output| !params.unspendable.contains(&local_output.outpoint))
                // if bumping fees only add to optional UTxOs those confirmed
                .filter(|local_output| {
                    params.bumping_fee.is_none() || local_output.chain_position.is_confirmed()
                })
                .map(|utxo| WeightedUtxo {
                    satisfaction_weight: self
                        .public_descriptor(utxo.keychain)
                        .max_weight_to_satisfy()
                        .unwrap(),
                    utxo: Utxo::Local(utxo),
                })
                .collect()
        }
    }

    fn complete_transaction(
        &self,
        tx: Transaction,
        selected: Vec<Utxo>,
        params: TxParams,
    ) -> Result<Psbt, CreateTxError> {
        let mut psbt = Psbt::from_unsigned_tx(tx)?;

        if params.add_global_xpubs {
            let all_xpubs = self
                .keychains()
                .flat_map(|(_, desc)| desc.get_extended_keys())
                .collect::<Vec<_>>();

            for xpub in all_xpubs {
                let origin = match xpub.origin {
                    Some(origin) => origin,
                    None if xpub.xkey.depth == 0 => {
                        (xpub.root_fingerprint(&self.secp), vec![].into())
                    }
                    _ => return Err(CreateTxError::MissingKeyOrigin(xpub.xkey.to_string())),
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
                                CreateTxError::UnknownUtxo => psbt::Input {
                                    sighash_type: params.sighash,
                                    ..psbt::Input::default()
                                },
                                _ => return Err(e),
                            },
                        }
                }
                Utxo::Foreign {
                    outpoint,
                    psbt_input: foreign_psbt_input,
                    ..
                } => {
                    let is_taproot = foreign_psbt_input
                        .witness_utxo
                        .as_ref()
                        .map(|txout| txout.script_pubkey.is_p2tr())
                        .unwrap_or(false);
                    if !is_taproot
                        && !params.only_witness_utxo
                        && foreign_psbt_input.non_witness_utxo.is_none()
                    {
                        return Err(CreateTxError::MissingNonWitnessUtxo(outpoint));
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
        utxo: LocalOutput,
        sighash_type: Option<psbt::PsbtSighashType>,
        only_witness_utxo: bool,
    ) -> Result<psbt::Input, CreateTxError> {
        // Try to find the prev_script in our db to figure out if this is internal or external,
        // and the derivation index
        let &(keychain, child) = self
            .indexed_graph
            .index
            .index_of_spk(utxo.txout.script_pubkey)
            .ok_or(CreateTxError::UnknownUtxo)?;

        let mut psbt_input = psbt::Input {
            sighash_type,
            ..psbt::Input::default()
        };

        let desc = self.public_descriptor(keychain);
        let derived_descriptor = desc
            .at_derivation_index(child)
            .expect("child can't be hardened");

        psbt_input
            .update_with_descriptor_unchecked(&derived_descriptor)
            .map_err(MiniscriptPsbtError::Conversion)?;

        let prev_output = utxo.outpoint;
        if let Some(prev_tx) = self.indexed_graph.graph().get_tx(prev_output.txid) {
            if desc.is_witness() || desc.is_taproot() {
                psbt_input.witness_utxo = Some(prev_tx.output[prev_output.vout as usize].clone());
            }
            if !desc.is_taproot() && (!desc.is_witness() || !only_witness_utxo) {
                psbt_input.non_witness_utxo = Some(prev_tx.as_ref().clone());
            }
        }
        Ok(psbt_input)
    }

    fn update_psbt_with_descriptor(&self, psbt: &mut Psbt) -> Result<(), MiniscriptPsbtError> {
        // We need to borrow `psbt` mutably within the loops, so we have to allocate a vec for all
        // the input utxos and outputs
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
                self.indexed_graph.index.index_of_spk(out.script_pubkey)
            {
                let desc = self.public_descriptor(keychain);
                let desc = desc
                    .at_derivation_index(child)
                    .expect("child can't be hardened");

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
    /// Internally calls [`Self::public_descriptor`] to fetch the right descriptor
    pub fn descriptor_checksum(&self, keychain: KeychainKind) -> String {
        self.public_descriptor(keychain)
            .to_string()
            .split_once('#')
            .unwrap()
            .1
            .to_string()
    }

    /// Applies an update to the wallet and stages the changes (but does not persist them).
    ///
    /// Usually you create an `update` by interacting with some blockchain data source and inserting
    /// transactions related to your wallet into it.
    ///
    /// After applying updates you should persist the staged wallet changes. For an example of how
    /// to persist staged wallet changes see [`Wallet::reveal_next_address`].
    #[cfg(feature = "std")]
    #[cfg_attr(docsrs, doc(cfg(feature = "std")))]
    pub fn apply_update(&mut self, update: impl Into<Update>) -> Result<(), CannotConnectError> {
        use std::time::*;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time now must surpass epoch anchor");
        self.apply_update_at(update, now.as_secs())
    }

    /// Applies an `update` alongside a `seen_at` timestamp and stages the changes.
    ///
    /// `seen_at` represents when the update is seen (in unix seconds). It is used to determine the
    /// `last_seen`s for all transactions in the update which have no corresponding anchor(s). The
    /// `last_seen` value is used internally to determine precedence of conflicting unconfirmed
    /// transactions (where the transaction with the lower `last_seen` value is omitted from the
    /// canonical history).
    ///
    /// Use [`apply_update`](Wallet::apply_update) to have the `seen_at` value automatically set to
    /// the current time.
    pub fn apply_update_at(
        &mut self,
        update: impl Into<Update>,
        seen_at: u64,
    ) -> Result<(), CannotConnectError> {
        let update = update.into();
        let mut changeset = match update.chain {
            Some(chain_update) => ChangeSet::from(self.chain.apply_update(chain_update)?),
            None => ChangeSet::default(),
        };

        let index_changeset = self
            .indexed_graph
            .index
            .reveal_to_target_multi(&update.last_active_indices);
        changeset.merge(index_changeset.into());
        changeset.merge(
            self.indexed_graph
                .apply_update_at(update.tx_update, Some(seen_at))
                .into(),
        );
        self.stage.merge(changeset);
        Ok(())
    }

    /// Get a reference of the staged [`ChangeSet`] that is yet to be committed (if any).
    pub fn staged(&self) -> Option<&ChangeSet> {
        if self.stage.is_empty() {
            None
        } else {
            Some(&self.stage)
        }
    }

    /// Get a mutable reference of the staged [`ChangeSet`] that is yet to be committed (if any).
    pub fn staged_mut(&mut self) -> Option<&mut ChangeSet> {
        if self.stage.is_empty() {
            None
        } else {
            Some(&mut self.stage)
        }
    }

    /// Take the staged [`ChangeSet`] to be persisted now (if any).
    pub fn take_staged(&mut self) -> Option<ChangeSet> {
        self.stage.take()
    }

    /// Get a reference to the inner [`TxGraph`].
    pub fn tx_graph(&self) -> &TxGraph<ConfirmationBlockTime> {
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

    /// Introduces a `block` of `height` to the wallet, and tries to connect it to the
    /// `prev_blockhash` of the block's header.
    ///
    /// This is a convenience method that is equivalent to calling [`apply_block_connected_to`]
    /// with `prev_blockhash` and `height-1` as the `connected_to` parameter.
    ///
    /// [`apply_block_connected_to`]: Self::apply_block_connected_to
    pub fn apply_block(&mut self, block: &Block, height: u32) -> Result<(), CannotConnectError> {
        let connected_to = match height.checked_sub(1) {
            Some(prev_height) => BlockId {
                height: prev_height,
                hash: block.header.prev_blockhash,
            },
            None => BlockId {
                height,
                hash: block.block_hash(),
            },
        };
        self.apply_block_connected_to(block, height, connected_to)
            .map_err(|err| match err {
                ApplyHeaderError::InconsistentBlocks => {
                    unreachable!("connected_to is derived from the block so must be consistent")
                }
                ApplyHeaderError::CannotConnect(err) => err,
            })
    }

    /// Applies relevant transactions from `block` of `height` to the wallet, and connects the
    /// block to the internal chain.
    ///
    /// The `connected_to` parameter informs the wallet how this block connects to the internal
    /// [`LocalChain`]. Relevant transactions are filtered from the `block` and inserted into the
    /// internal [`TxGraph`].
    ///
    /// **WARNING**: You must persist the changes resulting from one or more calls to this method
    /// if you need the inserted block data to be reloaded after closing the wallet.
    /// See [`Wallet::reveal_next_address`].
    pub fn apply_block_connected_to(
        &mut self,
        block: &Block,
        height: u32,
        connected_to: BlockId,
    ) -> Result<(), ApplyHeaderError> {
        let mut changeset = ChangeSet::default();
        changeset.merge(
            self.chain
                .apply_header_connected_to(&block.header, height, connected_to)?
                .into(),
        );
        changeset.merge(
            self.indexed_graph
                .apply_block_relevant(block, height)
                .into(),
        );
        self.stage.merge(changeset);
        Ok(())
    }

    /// Apply relevant unconfirmed transactions to the wallet.
    ///
    /// Transactions that are not relevant are filtered out.
    ///
    /// This method takes in an iterator of `(tx, last_seen)` where `last_seen` is the timestamp of
    /// when the transaction was last seen in the mempool. This is used for conflict resolution
    /// when there is conflicting unconfirmed transactions. The transaction with the later
    /// `last_seen` is prioritized.
    ///
    /// **WARNING**: You must persist the changes resulting from one or more calls to this method
    /// if you need the applied unconfirmed transactions to be reloaded after closing the wallet.
    /// See [`Wallet::reveal_next_address`].
    pub fn apply_unconfirmed_txs<T: Into<Arc<Transaction>>>(
        &mut self,
        unconfirmed_txs: impl IntoIterator<Item = (T, u64)>,
    ) {
        let indexed_graph_changeset = self
            .indexed_graph
            .batch_insert_relevant_unconfirmed(unconfirmed_txs);
        self.stage.merge(indexed_graph_changeset.into());
    }

    /// Used internally to ensure that all methods requiring a [`KeychainKind`] will use a
    /// keychain with an associated descriptor. For example in case the wallet was created
    /// with only one keychain, passing [`KeychainKind::Internal`] here will instead return
    /// [`KeychainKind::External`].
    fn map_keychain(&self, keychain: KeychainKind) -> KeychainKind {
        if self.keychains().count() == 1 {
            KeychainKind::External
        } else {
            keychain
        }
    }
}

/// Methods to construct sync/full-scan requests for spk-based chain sources.
impl Wallet {
    /// Create a partial [`SyncRequest`] for this wallet for all revealed spks.
    ///
    /// This is the first step when performing a spk-based wallet partial sync, the returned
    /// [`SyncRequest`] collects all revealed script pubkeys from the wallet keychain needed to
    /// start a blockchain sync with a spk based blockchain client.
    pub fn start_sync_with_revealed_spks(&self) -> SyncRequestBuilder<(KeychainKind, u32)> {
        use bdk_chain::keychain_txout::SyncRequestBuilderExt;
        SyncRequest::builder()
            .chain_tip(self.chain.tip())
            .revealed_spks_from_indexer(&self.indexed_graph.index, ..)
    }

    /// Create a [`FullScanRequest] for this wallet.
    ///
    /// This is the first step when performing a spk-based wallet full scan, the returned
    /// [`FullScanRequest] collects iterators for the wallet's keychain script pub keys needed to
    /// start a blockchain full scan with a spk based blockchain client.
    ///
    /// This operation is generally only used when importing or restoring a previously used wallet
    /// in which the list of used scripts is not known.
    pub fn start_full_scan(&self) -> FullScanRequestBuilder<KeychainKind> {
        use bdk_chain::keychain_txout::FullScanRequestBuilderExt;
        FullScanRequest::builder()
            .chain_tip(self.chain.tip())
            .spks_from_indexer(&self.indexed_graph.index)
    }
}

impl AsRef<bdk_chain::tx_graph::TxGraph<ConfirmationBlockTime>> for Wallet {
    fn as_ref(&self) -> &bdk_chain::tx_graph::TxGraph<ConfirmationBlockTime> {
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
) -> Result<String, DescriptorError>
where
    T: IntoWalletDescriptor,
{
    //TODO check descriptors contains only public keys
    let descriptor = descriptor
        .into_wallet_descriptor(secp, network)?
        .0
        .to_string();
    let mut wallet_name = descriptor.split_once('#').unwrap().1.to_string();
    if let Some(change_descriptor) = change_descriptor {
        let change_descriptor = change_descriptor
            .into_wallet_descriptor(secp, network)?
            .0
            .to_string();
        wallet_name.push_str(change_descriptor.split_once('#').unwrap().1);
    }

    Ok(wallet_name)
}

fn new_local_utxo(
    keychain: KeychainKind,
    derivation_index: u32,
    full_txo: FullTxOut<ConfirmationBlockTime>,
) -> LocalOutput {
    LocalOutput {
        outpoint: full_txo.outpoint,
        txout: full_txo.txout,
        is_spent: full_txo.spent_by.is_some(),
        chain_position: full_txo.chain_position,
        keychain,
        derivation_index,
    }
}

fn create_indexer(
    descriptor: ExtendedDescriptor,
    change_descriptor: Option<ExtendedDescriptor>,
    lookahead: u32,
) -> Result<KeychainTxOutIndex<KeychainKind>, DescriptorError> {
    let mut indexer = KeychainTxOutIndex::<KeychainKind>::new(lookahead);

    // let (descriptor, keymap) = descriptor;
    // let signers = Arc::new(SignersContainer::build(keymap, &descriptor, secp));
    assert!(indexer
        .insert_descriptor(KeychainKind::External, descriptor)
        .expect("first descriptor introduced must succeed"));

    // let (descriptor, keymap) = change_descriptor;
    // let change_signers = Arc::new(SignersContainer::build(keymap, &descriptor, secp));
    if let Some(change_descriptor) = change_descriptor {
        assert!(indexer
            .insert_descriptor(KeychainKind::Internal, change_descriptor)
            .map_err(|e| {
                use bdk_chain::indexer::keychain_txout::InsertDescriptorError;
                match e {
                    InsertDescriptorError::DescriptorAlreadyAssigned { .. } => {
                        crate::descriptor::error::Error::ExternalAndInternalAreTheSame
                    }
                    InsertDescriptorError::KeychainAlreadyAssigned { .. } => {
                        unreachable!("this is the first time we're assigning internal")
                    }
                }
            })?);
    }

    Ok(indexer)
}

/// Transforms a [`FeeRate`] to `f64` with unit as sat/vb.
#[macro_export]
#[doc(hidden)]
macro_rules! floating_rate {
    ($rate:expr) => {{
        use $crate::bitcoin::constants::WITNESS_SCALE_FACTOR;
        // sat_kwu / 250.0 -> sat_vb
        $rate.to_sat_per_kwu() as f64 / ((1000 / WITNESS_SCALE_FACTOR) as f64)
    }};
}

#[macro_export]
#[doc(hidden)]
/// Macro for getting a wallet for use in a doctest
macro_rules! doctest_wallet {
    () => {{
        use $crate::bitcoin::{BlockHash, Transaction, absolute, TxOut, Network, hashes::Hash};
        use $crate::chain::{ConfirmationBlockTime, BlockId, TxGraph, tx_graph};
        use $crate::{Update, KeychainKind, Wallet};
        use $crate::test_utils::*;
        let descriptor = "tr([73c5da0a/86'/0'/0']tprv8fMn4hSKPRC1oaCPqxDb1JWtgkpeiQvZhsr8W2xuy3GEMkzoArcAWTfJxYb6Wj8XNNDWEjfYKK4wGQXh3ZUXhDF2NcnsALpWTeSwarJt7Vc/0/*)";
        let change_descriptor = "tr([73c5da0a/86'/0'/0']tprv8fMn4hSKPRC1oaCPqxDb1JWtgkpeiQvZhsr8W2xuy3GEMkzoArcAWTfJxYb6Wj8XNNDWEjfYKK4wGQXh3ZUXhDF2NcnsALpWTeSwarJt7Vc/1/*)";

        let mut wallet = Wallet::create(descriptor, change_descriptor)
            .network(Network::Regtest)
            .create_wallet_no_persist()
            .unwrap();
        let address = wallet.peek_address(KeychainKind::External, 0).address;
        let tx = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![],
            output: vec![TxOut {
                value: Amount::from_sat(500_000),
                script_pubkey: address.script_pubkey(),
            }],
        };
        let txid = tx.compute_txid();
        let block_id = BlockId { height: 500, hash: BlockHash::all_zeros() };
        insert_checkpoint(&mut wallet, block_id);
        insert_checkpoint(&mut wallet, BlockId { height: 1_000, hash: BlockHash::all_zeros() });
        insert_tx(&mut wallet, tx);
        let anchor = ConfirmationBlockTime {
            confirmation_time: 50_000,
            block_id,
        };
        insert_anchor(&mut wallet, txid, anchor);
        wallet
    }}
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_utils::get_test_tr_single_sig_xprv_and_change_desc;
    use crate::test_utils::insert_tx;

    #[test]
    fn not_duplicated_utxos_across_optional_and_required() {
        let (external_desc, internal_desc) = get_test_tr_single_sig_xprv_and_change_desc();

        // create new wallet
        let mut wallet = Wallet::create(external_desc, internal_desc)
            .network(Network::Testnet)
            .create_wallet_no_persist()
            .unwrap();

        let two_output_tx = Transaction {
            input: vec![],
            output: vec![
                TxOut {
                    script_pubkey: wallet
                        .next_unused_address(KeychainKind::External)
                        .script_pubkey(),
                    value: Amount::from_sat(25_000),
                },
                TxOut {
                    script_pubkey: wallet
                        .next_unused_address(KeychainKind::External)
                        .script_pubkey(),
                    value: Amount::from_sat(75_000),
                },
            ],
            version: transaction::Version::non_standard(0),
            lock_time: absolute::LockTime::ZERO,
        };

        let txid = two_output_tx.compute_txid();
        insert_tx(&mut wallet, two_output_tx);

        let mut params = TxParams::default();
        let output = wallet.get_utxo(OutPoint { txid, vout: 0 }).unwrap();
        params.utxos.insert(
            output.outpoint,
            WeightedUtxo {
                satisfaction_weight: wallet
                    .public_descriptor(output.keychain)
                    .max_weight_to_satisfy()
                    .unwrap(),
                utxo: Utxo::Local(output),
            },
        );
        // enforce selection of first output in transaction
        let received = wallet.filter_utxos(&params, wallet.latest_checkpoint().block_id().height);
        // notice expected doesn't include the first output from two_output_tx as it should be
        // filtered out
        let expected = vec![wallet
            .get_utxo(OutPoint { txid, vout: 1 })
            .map(|utxo| WeightedUtxo {
                satisfaction_weight: wallet
                    .public_descriptor(utxo.keychain)
                    .max_weight_to_satisfy()
                    .unwrap(),
                utxo: Utxo::Local(utxo),
            })
            .unwrap()];

        assert_eq!(expected, received);
    }
}
