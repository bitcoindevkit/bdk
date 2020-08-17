use std::collections::HashSet;
use std::ops::Deref;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use bitcoin::{Transaction, Txid};

use crate::database::{BatchDatabase, DatabaseUtils};
use crate::error::Error;
use crate::FeeRate;

pub mod utils;

#[cfg(feature = "electrum")]
pub mod electrum;
#[cfg(feature = "electrum")]
pub use self::electrum::ElectrumBlockchain;

#[cfg(feature = "esplora")]
pub mod esplora;
#[cfg(feature = "esplora")]
pub use self::esplora::EsploraBlockchain;

#[cfg(feature = "compact_filters")]
pub mod compact_filters;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    FullHistory,
    GetAnyTx,
    AccurateFees,
}

pub trait Blockchain {
    fn is_online(&self) -> bool;

    fn offline() -> Self;
}

pub struct OfflineBlockchain;
impl Blockchain for OfflineBlockchain {
    fn offline() -> Self {
        OfflineBlockchain
    }

    fn is_online(&self) -> bool {
        false
    }
}

#[maybe_async]
pub trait OnlineBlockchain: Blockchain {
    fn get_capabilities(&self) -> HashSet<Capability>;

    fn setup<D: BatchDatabase + DatabaseUtils, P: 'static + Progress>(
        &self,
        stop_gap: Option<usize>,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error>;
    fn sync<D: BatchDatabase + DatabaseUtils, P: 'static + Progress>(
        &self,
        stop_gap: Option<usize>,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error> {
        maybe_await!(self.setup(stop_gap, database, progress_update))
    }

    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error>;
    fn broadcast(&self, tx: &Transaction) -> Result<(), Error>;

    fn get_height(&self) -> Result<u32, Error>;
    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error>;
}

pub type ProgressData = (f32, Option<String>);

pub trait Progress: Send {
    fn update(&self, progress: f32, message: Option<String>) -> Result<(), Error>;
}

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

#[derive(Clone)]
pub struct NoopProgress;

pub fn noop_progress() -> NoopProgress {
    NoopProgress
}

impl Progress for NoopProgress {
    fn update(&self, _progress: f32, _message: Option<String>) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct LogProgress;

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

    fn setup<D: BatchDatabase + DatabaseUtils, P: 'static + Progress>(
        &self,
        stop_gap: Option<usize>,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error> {
        maybe_await!(self.deref().setup(stop_gap, database, progress_update))
    }

    fn sync<D: BatchDatabase + DatabaseUtils, P: 'static + Progress>(
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
