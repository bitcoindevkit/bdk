use std::collections::HashSet;

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use bitcoin::{Script, Transaction, Txid};

use electrum_client::{Client, ElectrumApi};

use self::utils::{ELSGetHistoryRes, ELSListUnspentRes, ElectrumLikeSync};
use super::*;
use crate::database::{BatchDatabase, DatabaseUtils};
use crate::error::Error;
use crate::FeeRate;

pub struct ElectrumBlockchain(Option<Client>);

impl std::convert::From<Client> for ElectrumBlockchain {
    fn from(client: Client) -> Self {
        ElectrumBlockchain(Some(client))
    }
}

impl Blockchain for ElectrumBlockchain {
    fn offline() -> Self {
        ElectrumBlockchain(None)
    }

    fn is_online(&self) -> bool {
        self.0.is_some()
    }
}

impl OnlineBlockchain for ElectrumBlockchain {
    fn get_capabilities(&self) -> HashSet<Capability> {
        vec![Capability::FullHistory, Capability::GetAnyTx]
            .into_iter()
            .collect()
    }

    fn setup<D: BatchDatabase + DatabaseUtils, P: Progress>(
        &self,
        stop_gap: Option<usize>,
        database: &mut D,
        progress_update: P,
    ) -> Result<(), Error> {
        self.0
            .as_ref()
            .ok_or(Error::OfflineClient)?
            .electrum_like_setup(stop_gap, database, progress_update)
    }

    fn get_tx(&self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        Ok(self
            .0
            .as_ref()
            .ok_or(Error::OfflineClient)?
            .transaction_get(txid)
            .map(Option::Some)?)
    }

    fn broadcast(&self, tx: &Transaction) -> Result<(), Error> {
        Ok(self
            .0
            .as_ref()
            .ok_or(Error::OfflineClient)?
            .transaction_broadcast(tx)
            .map(|_| ())?)
    }

    fn get_height(&self) -> Result<u32, Error> {
        // TODO: unsubscribe when added to the client, or is there a better call to use here?

        Ok(self
            .0
            .as_ref()
            .ok_or(Error::OfflineClient)?
            .block_headers_subscribe()
            .map(|data| data.height as u32)?)
    }

    fn estimate_fee(&self, target: usize) -> Result<FeeRate, Error> {
        Ok(FeeRate::from_btc_per_kvb(
            self.0
                .as_ref()
                .ok_or(Error::OfflineClient)?
                .estimate_fee(target)? as f32,
        ))
    }
}

impl ElectrumLikeSync for Client {
    fn els_batch_script_get_history<'s, I: IntoIterator<Item = &'s Script>>(
        &self,
        scripts: I,
    ) -> Result<Vec<Vec<ELSGetHistoryRes>>, Error> {
        self.batch_script_get_history(scripts)
            .map(|v| {
                v.into_iter()
                    .map(|v| {
                        v.into_iter()
                            .map(
                                |electrum_client::GetHistoryRes {
                                     height, tx_hash, ..
                                 }| ELSGetHistoryRes {
                                    height,
                                    tx_hash,
                                },
                            )
                            .collect()
                    })
                    .collect()
            })
            .map_err(Error::Electrum)
    }

    fn els_batch_script_list_unspent<'s, I: IntoIterator<Item = &'s Script>>(
        &self,
        scripts: I,
    ) -> Result<Vec<Vec<ELSListUnspentRes>>, Error> {
        self.batch_script_list_unspent(scripts)
            .map(|v| {
                v.into_iter()
                    .map(|v| {
                        v.into_iter()
                            .map(
                                |electrum_client::ListUnspentRes {
                                     height,
                                     tx_hash,
                                     tx_pos,
                                     ..
                                 }| ELSListUnspentRes {
                                    height,
                                    tx_hash,
                                    tx_pos,
                                },
                            )
                            .collect()
                    })
                    .collect()
            })
            .map_err(Error::Electrum)
    }

    fn els_transaction_get(&self, txid: &Txid) -> Result<Transaction, Error> {
        self.transaction_get(txid).map_err(Error::Electrum)
    }
}
