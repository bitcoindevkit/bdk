// Magical Bitcoin Library
// Written in 2020 by
//     Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020 Magical Bitcoin
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Blockchain backends
//!
//! This module provides the implementation of a few commonly-used backends like
//! [Electrum](crate::blockchain::electrum), [Esplora](crate::blockchain::esplora) and
//! [Compact Filters/Neutrino](crate::blockchain::compact_filters), along with two generalized
//! traits [`Blockchain`] and [`OnlineBlockchain`] that can be implemented to build customized
//! backends.
//!
//! Types that only implement the [`Blockchain`] trait can be used as backends for [`Wallet`](crate::wallet::Wallet)s, but any
//! action that requires interacting with the blockchain won't be available ([`Wallet::sync`](crate::wallet::Wallet::sync) and
//! [`Wallet::broadcast`](crate::wallet::Wallet::broadcast)). This allows the creation of physically air-gapped wallets, that have no
//! ability to contact the outside world. An example of an offline-only client is [`OfflineBlockchain`].
//!
//! Types that also implement [`OnlineBlockchain`] will make the two aforementioned actions
//! available.

use std::collections::HashSet;
use std::ops::Deref;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use bitcoin::{Transaction, Txid};

use crate::database::BatchDatabase;
use crate::error::Error;
use crate::FeeRate;

pub(crate) mod utils;

#[cfg(feature = "electrum")]
#[cfg_attr(docsrs, doc(cfg(feature = "electrum")))]
pub mod electrum;
#[cfg(feature = "electrum")]
pub use self::electrum::ElectrumBlockchain;

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

/// Capabilities that can be supported by an [`OnlineBlockchain`] backend
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Can recover the full history of a wallet and not only the set of currently spendable UTXOs
    FullHistory,
    /// Can fetch any historical transaction given its txid
    GetAnyTx,
    /// Can compute accurate fees for the transactions found during sync
    AccurateFees,
}

/// Base trait for a blockchain backend
///
/// This trait is always required, even for "air-gapped" backends that don't actually make any
/// external call. Clients that have the ability to make external calls must also implement `OnlineBlockchain`.
pub trait Blockchain {
    /// Return whether or not the client has the ability to fullfill requests
    ///
    /// This should always be `false` for offline-only types, and can be true for types that also
    /// implement [`OnlineBlockchain`], if they have the ability to fullfill requests.
    fn is_online(&self) -> bool;

    /// Create a new instance of the client that is offline-only
    ///
    /// For types that also implement [`OnlineBlockchain`], this means creating an instance that
    /// returns [`Error::OfflineClient`](crate::error::Error::OfflineClient) if any of the "online"
    /// methods are called.
    ///
    /// This is generally implemented by wrapping the client in an [`Option`] that has [`Option::None`] value
    /// when created with this method, and is [`Option::Some`] if properly instantiated.
    fn offline() -> Self;
}

/// Type that only implements [`Blockchain`] and is always offline
pub struct OfflineBlockchain;
impl Blockchain for OfflineBlockchain {
    fn offline() -> Self {
        OfflineBlockchain
    }

    fn is_online(&self) -> bool {
        false
    }
}

/// Trait that defines the actions that must be supported by an online [`Blockchain`]
#[maybe_async]
pub trait OnlineBlockchain: Blockchain {
    /// Return the set of [`Capability`] supported by this backend
    fn get_capabilities(&self) -> HashSet<Capability>;

    /// Setup the backend and populate the internal database for the first time
    ///
    /// This method is the equivalent of [`OnlineBlockchain::sync`], but it's guaranteed to only be
    /// called once, at the first [`Wallet::sync`](crate::wallet::Wallet::sync).
    ///
    /// The rationale behind the distinction between `sync` and `setup` is that some custom backends
    /// might need to perform specific actions only the first time they are synced.
    ///
    /// For types that do not have that distinction, only this method can be implemented, since
    /// [`OnlineBlockchain::sync`] defaults to calling this internally if not overridden.
    fn setup<D: BatchDatabase, P: 'static + Progress>(
        &self,
        stop_gap: Option<usize>,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error>;
    /// Populate the internal database with transactions and UTXOs
    ///
    /// If not overridden, it defaults to calling [`OnlineBlockchain::setup`] internally.
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
    fn sync<D: BatchDatabase, P: 'static + Progress>(
        &self,
        stop_gap: Option<usize>,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error> {
        maybe_await!(self.setup(stop_gap, database, progress_update))
    }

    /// Fetch a transaction from the blockchain given its txid
    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error>;
    /// Broadcast a transaction
    fn broadcast(&self, tx: &Transaction) -> Result<(), Error>;

    /// Return the current height
    fn get_height(&self) -> Result<u32, Error>;
    /// Estimate the fee rate required to confirm a transaction in a given `target` of blocks
    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error>;
}

/// Data sent with a progress update over a [`channel`]
pub type ProgressData = (f32, Option<String>);

/// Trait for types that can receive and process progress updates during [`OnlineBlockchain::sync`] and
/// [`OnlineBlockchain::setup`]
pub trait Progress: Send {
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
        if progress < 0.0 || progress > 100.0 {
            return Err(Error::InvalidProgressValue(progress));
        }

        self.send((progress, message))
            .map_err(|_| Error::ProgressUpdateError)
    }
}

/// Type that implements [`Progress`] and drops every update received
#[derive(Clone)]
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
#[derive(Clone)]
pub struct LogProgress;

/// Create a nwe instance of [`LogProgress`]
pub fn log_progress() -> LogProgress {
    LogProgress
}

impl Progress for LogProgress {
    fn update(&self, progress: f32, message: Option<String>) -> Result<(), Error> {
        log::info!("Sync {:.3}%: `{}`", progress, message.unwrap_or("".into()));

        Ok(())
    }
}

impl<T: Blockchain> Blockchain for Arc<T> {
    fn is_online(&self) -> bool {
        self.deref().is_online()
    }

    fn offline() -> Self {
        Arc::new(T::offline())
    }
}

#[maybe_async]
impl<T: OnlineBlockchain> OnlineBlockchain for Arc<T> {
    fn get_capabilities(&self) -> HashSet<Capability> {
        maybe_await!(self.deref().get_capabilities())
    }

    fn setup<D: BatchDatabase, P: 'static + Progress>(
        &self,
        stop_gap: Option<usize>,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error> {
        maybe_await!(self.deref().setup(stop_gap, database, progress_update))
    }

    fn sync<D: BatchDatabase, P: 'static + Progress>(
        &self,
        stop_gap: Option<usize>,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error> {
        maybe_await!(self.deref().sync(stop_gap, database, progress_update))
    }

    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        maybe_await!(self.deref().get_tx(txid))
    }
    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        maybe_await!(self.deref().broadcast(tx))
    }

    fn get_height(&self) -> Result<u32, Error> {
        maybe_await!(self.deref().get_height())
    }
    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
        maybe_await!(self.deref().estimate_fee(target))
    }
}
