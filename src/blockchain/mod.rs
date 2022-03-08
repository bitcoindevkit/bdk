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

use std::collections::HashSet;
use std::sync::mpsc::{channel, Receiver, Sender};

use bitcoin::{Transaction, Txid};

use crate::database::BatchDatabase;
use crate::error::Error;
use crate::FeeRate;

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
pub trait Blockchain: WalletSync + GetHeight + GetTx {
    /// Return the set of [`Capability`] supported by this backend
    fn get_capabilities(&self) -> HashSet<Capability>;
    /// Broadcast a transaction
    fn broadcast(&self, tx: &Transaction) -> Result<(), Error>;
    /// Estimate the fee rate required to confirm a transaction in a given `target` of blocks
    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error>;
}

/// Trait for getting the current height of the blockchain.
pub trait GetHeight {
    /// Return the current height
    fn get_height(&self) -> Result<u32, Error>;
}

/// Trait for getting a transaction by txid
pub trait GetTx {
    /// Fetch a transaction given its txid
    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error>;
}

/// Trait for blockchains that can sync by updating the database directly.
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
        database: &mut D,
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
        database: &mut D,
        progress_update: Box<dyn Progress>,
    ) -> Result<(), Error> {
        self.wallet_setup(database, progress_update)
    }
}

/// Trait for [`Blockchain`] types that can be created given a configuration
pub trait ConfigurableBlockchain: Blockchain + Sized {
    /// Type that contains the configuration
    type Config: std::fmt::Debug;

    /// Create a new instance given a configuration
    fn from_config(config: &Self::Config) -> Result<Self, Error>;
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

// TODO: Do we need this?
// impl<T: Blockchain> Blockchain for Arc<T> {
//     fn get_capabilities(&self) -> HashSet<Capability> {
//         maybe_await!(self.deref().get_capabilities())
//     }

//     fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
//         maybe_await!(self.deref().broadcast(tx))
//     }

//     fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
//         maybe_await!(self.deref().estimate_fee(target))
//     }
// }

// impl<T: GetTx> GetTx for Arc<T> {
//     fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
//         maybe_await!(self.deref().get_tx(txid))
//     }
// }

// impl<T: GetHeight> GetHeight for Arc<T> {
//     fn get_height(&self) -> Result<u32, Error> {
//         maybe_await!(self.deref().get_height())
//     }
// }

// impl<T: WalletSync> WalletSync for Arc<T> {
//     fn wallet_setup<D: BatchDatabase>(
//         &self,
//         database: &mut D,
//         progress_update: Box<dyn Progress>,
//     ) -> Result<(), Error> {
//         maybe_await!(self.deref().wallet_setup(database, progress_update))
//     }

//     fn wallet_sync<D: BatchDatabase>(
//         &self,
//         database: &mut D,
//         progress_update: Box<dyn Progress>,
//     ) -> Result<(), Error> {
//         maybe_await!(self.deref().wallet_sync(database, progress_update))
//     }
// }
