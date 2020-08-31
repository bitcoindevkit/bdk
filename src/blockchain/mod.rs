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

    fn setup<D: BatchDatabase, P: 'static + Progress>(
        &self,
        stop_gap: Option<usize>,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error>;
    fn sync<D: BatchDatabase, P: 'static + Progress>(
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
