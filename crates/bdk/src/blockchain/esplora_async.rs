use async_trait::async_trait;
use std::boxed::Box;
use std::collections::BTreeMap;
use std::prelude::v1::{ToString, Vec};

use bdk_esplora::esplora_client::AsyncClient;
use bdk_esplora::esplora_client::Error as EsploraError;
use bdk_esplora::EsploraAsyncExt;
use bitcoin::{OutPoint, ScriptBuf, Transaction, Txid};

use crate::blockchain::async_traits::*;
use crate::blockchain::{EstimateFeeError, SpkSyncMode};
use crate::wallet::Update;
use crate::{FeeRate, Wallet};

pub struct EsploraAsync {
    client: AsyncClient,
    parallel_requests: usize,
}

#[async_trait]
impl Broadcast for EsploraAsync {
    type Error = EsploraError;

    async fn broadcast(&self, tx: &Transaction) -> Result<(), Self::Error> {
        self.client.broadcast(tx).await
    }
}

#[async_trait]
impl EstimateFee for EsploraAsync {
    type Error = EstimateFeeError<EsploraError>;
    type Target = usize;

    async fn estimate_fee(&self, target: Self::Target) -> Result<FeeRate, Self::Error> {
        let estimates = self
            .client
            .get_fee_estimates()
            .await
            .map_err(EstimateFeeError::ClientError)?;
        match estimates.get(target.to_string().as_str()) {
            None => Err(EstimateFeeError::InsufficientData),
            Some(rate) => {
                let fee_rate = FeeRate::from_sat_per_vb(*rate as f32);
                Ok(fee_rate)
            }
        }
    }
}

#[async_trait]
impl ScanSpks for EsploraAsync {
    type Error = EsploraError;

    async fn scan_spks(&self, wallet: &Wallet, stop_gap: usize) -> Result<Update, Self::Error> {
        let keychain_spks = wallet
            .spks_of_all_keychains()
            .into_iter()
            .collect::<BTreeMap<_, _>>();

        // The client scans keychain spks for transaction histories, stopping after `stop_gap` number of
        // unused spks is reached. It returns a `TxGraph` update (`graph_update`) and a structure that
        // represents the last active spk derivation indices of the keychains.
        let (graph_update, last_active_indices) = self
            .client
            .scan_txs_with_keychains(
                keychain_spks,
                core::iter::empty(),
                core::iter::empty(),
                stop_gap,
                self.parallel_requests,
            )
            .await?;

        let prev_tip = wallet.latest_checkpoint();
        let missing_heights = wallet.tx_graph().missing_heights(wallet.local_chain());

        let chain_update = self
            .client
            .update_local_chain(prev_tip.clone(), missing_heights)
            .await?;

        let update = Update {
            last_active_indices,
            graph: graph_update,
            chain: Some(chain_update),
        };
        Ok(update)
    }
}

#[async_trait]
impl ModalSyncSpks for EsploraAsync {
    type Error = EsploraError;
    type SyncMode = SpkSyncMode;

    async fn sync_spks(
        &self,
        wallet: &Wallet,
        sync_mode: Self::SyncMode,
    ) -> Result<Update, Self::Error> {
        // Spks, outpoints and txids we want updates on will be accumulated here.
        let mut spks: Box<Vec<ScriptBuf>> = Box::default();
        let mut outpoints: Box<dyn Iterator<Item = OutPoint> + Send> =
            Box::new(core::iter::empty());
        let mut txids: Box<dyn Iterator<Item = Txid> + Send> = Box::new(core::iter::empty());

        // Sync all SPKs know to the wallet
        if sync_mode.all_spks {
            let all_spks: Vec<ScriptBuf> = wallet
                .spk_index()
                .all_spks()
                .iter()
                .map(|((_keychain, _index), script_buf)| ScriptBuf::from(script_buf.as_script()))
                .collect();
            spks = Box::new(all_spks);
        }
        // Sync only unused SPKs
        else if sync_mode.unused_spks {
            let unused_spks: Vec<ScriptBuf> = wallet
                .spk_index()
                .unused_spks(..)
                .map(|((_keychain, _index), script)| ScriptBuf::from(script))
                .collect();
            spks = Box::new(unused_spks);
        }

        // Sync UTXOs
        if sync_mode.utxos {
            // We want to search for whether the UTXO is spent, and spent by which
            // transaction. We provide the outpoint of the UTXO to
            // `EsploraExt::update_tx_graph_without_keychain`.
            let utxo_outpoints = wallet.list_unspent().map(|utxo| utxo.outpoint);
            outpoints = Box::new(utxo_outpoints);
        };

        // Sync unconfirmed TX
        if sync_mode.unconfirmed_tx {
            // We want to search for whether the unconfirmed transaction is now confirmed.
            // We provide the unconfirmed txids to
            // `EsploraExt::update_tx_graph_without_keychain`.
            let unconfirmed_txids = wallet
                .transactions()
                .filter(|canonical_tx| !canonical_tx.chain_position.is_confirmed())
                .map(|canonical_tx| canonical_tx.tx_node.txid);
            txids = Box::new(unconfirmed_txids);
        }

        let graph_update = self
            .client
            .scan_txs(spks.into_iter(), txids, outpoints, self.parallel_requests)
            .await?;

        let prev_tip = wallet.latest_checkpoint();
        let missing_heights = wallet.tx_graph().missing_heights(wallet.local_chain());
        let chain_update = self
            .client
            .update_local_chain(prev_tip, missing_heights)
            .await?;

        let update = Update {
            // no update to active indices
            last_active_indices: BTreeMap::new(),
            graph: graph_update,
            chain: Some(chain_update),
        };
        Ok(update)
    }
}
