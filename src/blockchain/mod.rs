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

//! Blockchain backends
//!
//! This module provides the implementation of a few commonly-used backends like
//! [Electrum](crate::blockchain::electrum), [Esplora](crate::blockchain::esplora) and
//! [Compact Filters/Neutrino](crate::blockchain::compact_filters), along with a generalized trait
//! [`Blockchain`] that can be implemented to build customized backends.

use std::cell::RefCell;
use std::collections::HashSet;
use std::ops::Deref;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use bitcoin::{BlockHash, Transaction, Txid};

use crate::database::BatchDatabase;
use crate::error::Error;
use crate::wallet::{wallet_name_from_descriptor, Wallet};
use crate::{FeeRate, KeychainKind};

#[cfg(any(
    feature = "electrum",
    feature = "esplora",
    feature = "compact_filters",
    feature = "rpc"
))]
pub mod any;
mod script_sync;

#[cfg(any(
    feature = "electrum",
    feature = "esplora",
    feature = "compact_filters",
    feature = "rpc"
))]
pub use any::{AnyBlockchain, AnyBlockchainConfig};

#[cfg(feature = "electrum")]
#[cfg_attr(docsrs, doc(cfg(feature = "electrum")))]
pub mod electrum;
#[cfg(feature = "electrum")]
pub use self::electrum::ElectrumBlockchain;
#[cfg(feature = "electrum")]
pub use self::electrum::ElectrumBlockchainConfig;

#[cfg(feature = "rpc")]
#[cfg_attr(docsrs, doc(cfg(feature = "rpc")))]
pub mod rpc;
#[cfg(feature = "rpc")]
pub use self::rpc::RpcBlockchain;
#[cfg(feature = "rpc")]
pub use self::rpc::RpcConfig;

#[cfg(feature = "esplora")]
#[cfg_attr(docsrs, doc(cfg(feature = "esplora")))]
pub mod esplora;
#[cfg(feature = "esplora")]
pub use self::esplora::EsploraBlockchain;

#[cfg(feature = "compact_filters")]
#[cfg_attr(docsrs, doc(cfg(feature = "compact_filters")))]
pub mod compact_filters;

#[cfg(feature = "compact_filters")]
pub use self::compact_filters::CompactFiltersBlockchain;

/// Capabilities that can be supported by a [`Blockchain`] backend
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Can recover the full history of a wallet and not only the set of currently spendable UTXOs
    FullHistory,
    /// Can fetch any historical transaction given its txid
    GetAnyTx,
    /// Can compute accurate fees for the transactions found during sync
    AccurateFees,
}

/// Trait that defines the actions that must be supported by a blockchain backend
#[maybe_async]
pub trait Blockchain: WalletSync + GetHeight + GetTx + GetBlockHash {
    /// Return the set of [`Capability`] supported by this backend
    fn get_capabilities(&self) -> HashSet<Capability>;
    /// Broadcast a transaction
    fn broadcast(&self, tx: &Transaction) -> Result<(), Error>;
    /// Estimate the fee rate required to confirm a transaction in a given `target` of blocks
    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error>;
}

/// Trait for getting the current height of the blockchain.
#[maybe_async]
pub trait GetHeight {
    /// Return the current height
    fn get_height(&self) -> Result<u32, Error>;
}

#[maybe_async]
/// Trait for getting a transaction by txid
pub trait GetTx {
    /// Fetch a transaction given its txid
    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error>;
}

#[maybe_async]
/// Trait for getting block hash by block height
pub trait GetBlockHash {
    /// fetch block hash given its height
    fn get_block_hash(&self, height: u64) -> Result<BlockHash, Error>;
}

/// Trait for blockchains that can sync by updating the database directly.
#[maybe_async]
pub trait WalletSync {
    /// Setup the backend and populate the internal database for the first time
    ///
    /// This method is the equivalent of [`Self::wallet_sync`], but it's guaranteed to only be
    /// called once, at the first [`Wallet::sync`](crate::wallet::Wallet::sync).
    ///
    /// The rationale behind the distinction between `sync` and `setup` is that some custom backends
    /// might need to perform specific actions only the first time they are synced.
    ///
    /// For types that do not have that distinction, only this method can be implemented, since
    /// [`WalletSync::wallet_sync`] defaults to calling this internally if not overridden.
    /// Populate the internal database with transactions and UTXOs
    fn wallet_setup<D: BatchDatabase>(
        &self,
        database: &RefCell<D>,
        progress_update: Box<dyn Progress>,
    ) -> Result<(), Error>;

    /// If not overridden, it defaults to calling [`Self::wallet_setup`] internally.
    ///
    /// This method should implement the logic required to iterate over the list of the wallet's
    /// script_pubkeys using [`Database::iter_script_pubkeys`] and look for relevant transactions
    /// in the blockchain to populate the database with [`BatchOperations::set_tx`] and
    /// [`BatchOperations::set_utxo`].
    ///
    /// This method should also take care of removing UTXOs that are seen as spent in the
    /// blockchain, using [`BatchOperations::del_utxo`].
    ///
    /// The `progress_update` object can be used to give the caller updates about the progress by using
    /// [`Progress::update`].
    ///
    /// [`Database::iter_script_pubkeys`]: crate::database::Database::iter_script_pubkeys
    /// [`BatchOperations::set_tx`]: crate::database::BatchOperations::set_tx
    /// [`BatchOperations::set_utxo`]: crate::database::BatchOperations::set_utxo
    /// [`BatchOperations::del_utxo`]: crate::database::BatchOperations::del_utxo
    fn wallet_sync<D: BatchDatabase>(
        &self,
        database: &RefCell<D>,
        progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        maybe_await!(self.wallet_setup(database, progress_update))
    }
}

/// Trait for [`Blockchain`] types that can be created given a configuration
pub trait ConfigurableBlockchain: Blockchain + Sized {
    /// Type that contains the configuration
    type Config: std::fmt::Debug;

    /// Create a new instance given a configuration
    fn from_config(config: &Self::Config) -> Result<Self, Error>;
}

/// Trait for blockchains that don't contain any state
///
/// Statless blockchains can be used to sync multiple wallets with different descriptors.
///
/// [`BlockchainFactory`] is automatically implemented for `Arc<T>` where `T` is a stateless
/// blockchain.
pub trait StatelessBlockchain: Blockchain {}

/// Trait for a factory of blockchains that share the underlying connection or configuration
#[cfg_attr(
    not(feature = "async-interface"),
    doc = r##"
## Example

This example shows how to sync multiple walles and return the sum of their balances

```no_run
# use bdk::Error;
# use bdk::blockchain::*;
# use bdk::database::*;
# use bdk::wallet::*;
# use bdk::*;
fn sum_of_balances<B: BlockchainFactory>(blockchain_factory: B, wallets: &[Wallet<MemoryDatabase>]) -> Result<Balance, Error> {
    Ok(wallets
        .iter()
        .map(|w| -> Result<_, Error> {
            blockchain_factory.sync_wallet(&w, None, SyncOptions::default())?;
            w.get_balance()
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .sum())
}
```
"##
)]
pub trait BlockchainFactory {
    /// The type returned when building a blockchain from this factory
    type Inner: Blockchain;

    /// Build a new blockchain for the given descriptor wallet_name
    ///
    /// If `override_skip_blocks` is `None`, the returned blockchain will inherit the number of blocks
    /// from the factory. Since it's not possible to override the value to `None`, set it to
    /// `Some(0)` to rescan from the genesis.
    fn build(
        &self,
        wallet_name: &str,
        override_skip_blocks: Option<u32>,
    ) -> Result<Self::Inner, Error>;

    /// Build a new blockchain for a given wallet
    ///
    /// Internally uses [`wallet_name_from_descriptor`] to derive the name, and then calls
    /// [`BlockchainFactory::build`] to create the blockchain instance.
    fn build_for_wallet<D: BatchDatabase>(
        &self,
        wallet: &Wallet<D>,
        override_skip_blocks: Option<u32>,
    ) -> Result<Self::Inner, Error> {
        let wallet_name = wallet_name_from_descriptor(
            wallet.public_descriptor(KeychainKind::External)?.unwrap(),
            wallet.public_descriptor(KeychainKind::Internal)?,
            wallet.network(),
            wallet.secp_ctx(),
        )?;
        self.build(&wallet_name, override_skip_blocks)
    }

    /// Use [`BlockchainFactory::build_for_wallet`] to get a blockchain, then sync the wallet
    ///
    /// This can be used when a new blockchain would only be used to sync a wallet and then
    /// immediately dropped. Keep in mind that specific blockchain factories may perform slow
    /// operations to build a blockchain for a given wallet, so if a wallet needs to be synced
    /// often it's recommended to use [`BlockchainFactory::build_for_wallet`] to reuse the same
    /// blockchain multiple times.
    #[cfg(not(feature = "async-interface"))]
    #[cfg_attr(docsrs, doc(cfg(not(feature = "async-interface"))))]
    fn sync_wallet<D: BatchDatabase>(
        &self,
        wallet: &Wallet<D>,
        override_skip_blocks: Option<u32>,
        sync_options: crate::wallet::SyncOptions,
    ) -> Result<(), Error> {
        let blockchain = self.build_for_wallet(wallet, override_skip_blocks)?;
        wallet.sync(&blockchain, sync_options)
    }
}

impl<T: StatelessBlockchain> BlockchainFactory for Arc<T> {
    type Inner = Self;

    fn build(&self, _wallet_name: &str, _override_skip_blocks: Option<u32>) -> Result<Self, Error> {
        Ok(Arc::clone(self))
    }
}

/// Data sent with a progress update over a [`channel`]
pub type ProgressData = (f32, Option<String>);

/// Trait for types that can receive and process progress updates during [`WalletSync::wallet_sync`] and
/// [`WalletSync::wallet_setup`]
pub trait Progress: Send + 'static + core::fmt::Debug {
    /// Send a new progress update
    ///
    /// The `progress` value should be in the range 0.0 - 100.0, and the `message` value is an
    /// optional text message that can be displayed to the user.
    fn update(&self, progress: f32, message: Option<String>) -> Result<(), Error>;
}

/// Shortcut to create a [`channel`] (pair of [`Sender`] and [`Receiver`]) that can transport [`ProgressData`]
pub fn progress() -> (Sender<ProgressData>, Receiver<ProgressData>) {
    channel()
}

impl Progress for Sender<ProgressData> {
    fn update(&self, progress: f32, message: Option<String>) -> Result<(), Error> {
        if !(0.0..=100.0).contains(&progress) {
            return Err(Error::InvalidProgressValue(progress));
        }

        self.send((progress, message))
            .map_err(|_| Error::ProgressUpdateError)
    }
}

/// Type that implements [`Progress`] and drops every update received
#[derive(Clone, Copy, Default, Debug)]
pub struct NoopProgress;

/// Create a new instance of [`NoopProgress`]
pub fn noop_progress() -> NoopProgress {
    NoopProgress
}

impl Progress for NoopProgress {
    fn update(&self, _progress: f32, _message: Option<String>) -> Result<(), Error> {
        Ok(())
    }
}

/// Type that implements [`Progress`] and logs at level `INFO` every update received
#[derive(Clone, Copy, Default, Debug)]
pub struct LogProgress;

/// Create a new instance of [`LogProgress`]
pub fn log_progress() -> LogProgress {
    LogProgress
}

impl Progress for LogProgress {
    fn update(&self, progress: f32, message: Option<String>) -> Result<(), Error> {
        log::info!(
            "Sync {:.3}%: `{}`",
            progress,
            message.unwrap_or_else(|| "".into())
        );

        Ok(())
    }
}

#[maybe_async]
impl<T: Blockchain> Blockchain for Arc<T> {
    fn get_capabilities(&self) -> HashSet<Capability> {
        maybe_await!(self.deref().get_capabilities())
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        maybe_await!(self.deref().broadcast(tx))
    }

    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
        maybe_await!(self.deref().estimate_fee(target))
    }
}

#[maybe_async]
impl<T: GetTx> GetTx for Arc<T> {
    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        maybe_await!(self.deref().get_tx(txid))
    }
}

#[maybe_async]
impl<T: GetHeight> GetHeight for Arc<T> {
    fn get_height(&self) -> Result<u32, Error> {
        maybe_await!(self.deref().get_height())
    }
}

#[maybe_async]
impl<T: GetBlockHash> GetBlockHash for Arc<T> {
    fn get_block_hash(&self, height: u64) -> Result<BlockHash, Error> {
        maybe_await!(self.deref().get_block_hash(height))
    }
}

#[maybe_async]
impl<T: WalletSync> WalletSync for Arc<T> {
    fn wallet_setup<D: BatchDatabase>(
        &self,
        database: &RefCell<D>,
        progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        maybe_await!(self.deref().wallet_setup(database, progress_update))
    }

    fn wallet_sync<D: BatchDatabase>(
        &self,
        database: &RefCell<D>,
        progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        maybe_await!(self.deref().wallet_sync(database, progress_update))
    }
}
