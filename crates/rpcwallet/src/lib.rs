use bdk_chain::{
    bitcoin::{BlockHash, Script, Transaction, Txid},
    chain_graph::{ChainGraph, NewError},
    keychain::KeychainScan,
    sparse_chain::{self, ChainPosition, SparseChain},
    tx_graph::TxGraph,
    BlockId, ConfirmationTime, TxHeight,
};
use bitcoincore_rpc::{
    bitcoincore_rpc_json::{
        ImportMultiOptions, ImportMultiRequest, ImportMultiRequestScriptPubkey,
        ImportMultiRescanSince, ImportMultiResultError,
    },
    Auth, Client, RpcApi,
};
use serde_json::Value;

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Debug,
    ops::Deref,
};

pub use bitcoincore_rpc;

/// Bitcoin Core RPC related errors.
#[derive(Debug)]
pub enum RpcError {
    Client(bitcoincore_rpc::Error),
    ImportMulti(ImportMultiResultError),
    General(String),
}

impl core::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::Client(e) => write!(f, "{}", e),
            RpcError::ImportMulti(e) => write!(f, "{:?}", e),
            RpcError::General(s) => write!(f, "{}", s),
        }
    }
}

impl std::error::Error for RpcError {}

impl From<bitcoincore_rpc::Error> for RpcError {
    fn from(e: bitcoincore_rpc::Error) -> Self {
        Self::Client(e)
    }
}

impl From<ImportMultiResultError> for RpcError {
    fn from(value: ImportMultiResultError) -> Self {
        Self::ImportMulti(value)
    }
}

pub trait RpcWalletExt {
    /// Fetch the latest block height.
    fn get_tip(&self) -> Result<(u32, BlockHash), RpcError>;

    /// Scan the blockchain (via RPC wallet) for the data specified. This returns a [`SparseChain`]
    /// which can be transformed into a [`KeychainScan`] after we find all the missing full
    /// transactions.
    ///
    /// - `local_chain`: the most recent block hashes present locally
    /// - `spks`: Script pubkeys that we wanna scan for
    fn scan(
        &self,
        local_chain: &BTreeMap<u32, BlockHash>,
        spks: impl IntoIterator<Item = Script>,
    ) -> Result<SparseChain, RpcError>;
}

pub struct RpcClient {
    pub client: Client,
}

impl Deref for RpcClient {
    type Target = Client;
    fn deref(&self) -> &Self::Target {
        &self.client
    }
}

/// Return list of bitcoin core wallet directories
fn list_wallet_dirs(client: &Client) -> Result<Vec<String>, RpcError> {
    #[derive(serde::Deserialize)]
    struct Name {
        name: String,
    }
    #[derive(serde::Deserialize)]
    struct CallResult {
        wallets: Vec<Name>,
    }

    let result: CallResult = client.call("listwalletdir", &[])?;
    Ok(result.wallets.into_iter().map(|n| n.name).collect())
}

/// Import script_pubkeys into bitcoin core wallet.
pub fn import_multi<'a>(
    client: &Client,
    scripts: impl Iterator<Item = &'a Script>,
) -> Result<(), RpcError> {
    let requests = scripts
        .map(|script| ImportMultiRequest {
            timestamp: ImportMultiRescanSince::Now,
            script_pubkey: Some(ImportMultiRequestScriptPubkey::Script(script)),
            watchonly: Some(true),
            ..Default::default()
        })
        .collect::<Vec<_>>();

    let options = ImportMultiOptions {
        rescan: Some(false),
    };
    for import_multi_result in client.import_multi(&requests, Some(&options))? {
        if let Some(err) = import_multi_result.error {
            return Err(err.into());
        }
    }
    Ok(())
}

impl RpcClient {
    /// Create a new RpcClient connection with the given details.
    /// `wallet-name` can be anything, but user needs to ensure that they are specifying
    /// a fixed wallet name for fixed set of descriptors in the keychain tracker.
    ///
    /// If a new wallet name is used at connection time, while the same keychain has been scanned
    /// previously with a different wallet name, this will create a fresh wallet in bitcoin core and
    /// rescan the blockchain from genesis.
    pub fn new<'a>(wallet_name: String, url: String, auth: Auth) -> Result<RpcClient, RpcError> {
        let wallet_url = format!("{}/wallet/{}", url, wallet_name);
        let client = Client::new(wallet_url.as_str(), auth.clone().into())?;
        let rpc_version = client.version()?;
        println!("rpc connection established. Core version : {}", rpc_version);
        println!("connected to '{}' with auth: {:?}", wallet_url, auth);

        if client.list_wallets()?.contains(&wallet_name.to_string()) {
            println!("wallet already loaded: {}", wallet_name);
        } else if list_wallet_dirs(&client)?.contains(&wallet_name.to_string()) {
            println!("wallet wasn't loaded. Loading wallet : {}", wallet_name);
            client.load_wallet(&wallet_name)?;
            println!("wallet loaded: {}", wallet_name);
        } else {
            // TODO: move back to api call when https://github.com/rust-bitcoin/rust-bitcoincore-rpc/issues/225 is closed
            // TODO: Legacy wallet will be deprecated in core in future. Find out solution with descriptor wallet
            // while avoiding the issue documented in https://github.com/bitcoindevkit/bdk/issues/859
            let args = [
                Value::String(String::from(&wallet_name)),
                Value::Bool(true),  // disable_private_keys
                Value::Bool(false), // blank
                Value::Null,        // passphrase
                Value::Bool(false), // avoid reuse
                Value::Bool(false), // descriptor
            ];
            let _: Value = client.call("createwallet", &args)?;
            println!("wallet created: {}", wallet_name);
        }

        Ok(RpcClient { client })
    }
}

impl RpcWalletExt for RpcClient {
    fn get_tip(&self) -> Result<(u32, BlockHash), RpcError> {
        Ok(self
            .client
            .get_blockchain_info()
            .map(|i| (i.blocks as u32, i.best_block_hash))?)
    }

    fn scan(
        &self,
        local_chain: &BTreeMap<u32, BlockHash>,
        spks: impl IntoIterator<Item = Script>,
    ) -> Result<SparseChain<TxHeight>, RpcError> {
        let spks = spks.into_iter().collect::<Vec<_>>();
        import_multi(&self, spks.iter())?;
        println!("Imported spks: {:?}", spks);

        let mut sparse_chain = SparseChain::default();

        let mut last_common_height = 0;
        for (&height, &original_hash) in local_chain.iter().rev() {
            let update_block_id = BlockId {
                height,
                hash: self.client.get_block_hash(height as u64)?,
            };
            let _ = sparse_chain
                .insert_checkpoint(update_block_id)
                .expect("should not collide");
            if update_block_id.hash == original_hash {
                last_common_height = update_block_id.height;
                break;
            }
        }

        match self.rescan_blockchain(Some(last_common_height as usize), None) {
            Err(bitcoincore_rpc::Error::JsonRpc(bitcoincore_rpc::jsonrpc::Error::Transport(_))) => {
                println!("Resource unreachable error hit, carrying on")
            }
            Ok(_) => {
                // All good. Do nothing
            }
            Err(e) => return Err(e.into()),
        }

        // Insert the new tip so new transactions will be accepted into the sparse chain.
        let tip = self.get_tip().map(|(height, hash)| BlockId {
            height: height as u32,
            hash,
        })?;
        if let Err(failure) = sparse_chain.insert_checkpoint(tip) {
            match failure {
                sparse_chain::InsertCheckpointError::HashNotMatching { .. } => {
                    // There has been a re-org before we even begin scanning addresses.
                    // Just recursively call (this should never happen).
                    return self.scan(local_chain, spks);
                }
            }
        }

        // Fetch the transactions
        let page_size = 1000; // Core has 1000 page size limit
        let mut page = 0;

        let _ = self
            .client
            .rescan_blockchain(Some(last_common_height as usize), None);

        let mut txids_to_update = Vec::new();

        loop {
            let list_tx_result = self.client
                .list_transactions(None, Some(page_size), Some(page * page_size), Some(true))?
                .iter()
                .filter(|item|
                    // filter out conflicting transactions - only accept transactions that are already
                    // confirmed, or exists in mempool
                    item.info.confirmations > 0 || self.client.get_mempool_entry(&item.info.txid).is_ok())
                .map(|list_result| {
                    let chain_index = match list_result.info.blockheight {
                        Some(height) if height <= tip.height => TxHeight::Confirmed(height),
                        _ => TxHeight::Unconfirmed,
                    };
                    (chain_index, list_result.info.txid)
                })
                .collect::<HashSet<(TxHeight, Txid)>>();

            txids_to_update.extend(list_tx_result.iter());

            if list_tx_result.len() < page_size {
                break;
            }
            page += 1;
        }

        for (index, txid) in txids_to_update {
            if let Err(failure) = sparse_chain.insert_tx(txid, index) {
                match failure {
                    sparse_chain::InsertTxError::TxTooHigh { .. } => {
                        unreachable!("We should not encounter this error as we ensured tx_height <= tip.height");
                    }
                    sparse_chain::InsertTxError::TxMovedUnexpectedly { .. } => {
                        /* This means there is a reorg, we will handle this situation below */
                    }
                }
            }
        }

        // Check for Reorg during the above sync process, recursively call the scan if detected.
        let our_latest = sparse_chain.latest_checkpoint().expect("must exist");
        if our_latest.hash != self.client.get_block_hash(our_latest.height as u64)? {
            return self.scan(local_chain, spks);
        }

        Ok(sparse_chain)
    }
}

pub fn into_confirmation_time(
    client: &RpcClient,
    sparsechain: SparseChain<TxHeight>,
) -> Result<SparseChain<ConfirmationTime>, RpcError> {
    let heights = sparsechain
        .range_txids_by_height(..TxHeight::Unconfirmed)
        .map(|(h, _)| match h {
            TxHeight::Confirmed(h) => *h,
            _ => unreachable!("already filtered out unconfirmed"),
        })
        .collect::<Vec<u32>>();

    let height_to_time: HashMap<u32, u64> = heights
        .clone()
        .into_iter()
        .map(|ht| {
            // TODO: This is very inefficient. But Bitcoin Core doesn't provide
            // an easy batch alternate. Figure out ways of caching data required for different chain position types.
            let hash = client.get_block_hash(ht as u64)?;
            let header = client.get_block_header(&hash)?;
            let time = header.time;
            Ok((ht, time as u64))
        })
        .collect::<Result<HashMap<u32, u64>, RpcError>>()?;

    let mut new_update =
        SparseChain::<ConfirmationTime>::from_checkpoints(sparsechain.range_checkpoints(..));

    for &(tx_height, txid) in sparsechain.txids() {
        let conf_time = match tx_height {
            TxHeight::Confirmed(height) => ConfirmationTime::Confirmed {
                height,
                time: height_to_time[&height],
            },
            TxHeight::Unconfirmed => ConfirmationTime::Unconfirmed,
        };
        let _ = new_update.insert_tx(txid, conf_time).expect("must insert");
    }

    Ok(new_update)
}

/// Return a list of missing full transactions that are required to [`inflate_update`].
///
/// [`inflate_update`]: bdk_chain::chain_graph::ChainGraph::inflate_update
pub fn missing_full_txs<G>(response: &SparseChain, existing_graph: G) -> Vec<Txid>
where
    G: AsRef<TxGraph>,
{
    response
        .txids()
        .filter(|(_, txid)| existing_graph.as_ref().get_tx(*txid).is_none())
        .map(|(_, txid)| *txid)
        .collect()
}

/// Transform the [`SparseChain`] into a [`KeychainScan`], which can be applied to a
/// `tracker`.
///
/// This will fail if there are missing full transactions not provided via `new_txs`.
pub fn into_keychain_scan<K, P, CG>(
    response: SparseChain<P>,
    new_txs: Vec<Transaction>,
    chain_graph: CG,
) -> Result<KeychainScan<K, P>, NewError<P>>
where
    K: Ord + Clone + Debug,
    P: ChainPosition,
    CG: AsRef<ChainGraph<P>>,
{
    Ok(KeychainScan {
        update: chain_graph.as_ref().inflate_update(response, new_txs)?,
        last_active_indices: BTreeMap::<K, u32>::new(),
    })
}
