use std::collections::HashSet;

use log::{debug, info};

use bitcoin::{Address, Network, OutPoint, Transaction, Txid};

use bitcoincore_rpc::{
    json::{
        ImportMultiOptions, ImportMultiRequest, ImportMultiRequestScriptPubkey,
        ImportMultiRescanSince, ListTransactionResult,
    },
    jsonrpc, Client, Error as RpcError, RpcApi,
};

use super::*;
use crate::database::{BatchDatabase, BatchOperations, DatabaseUtils};
use crate::error::Error;
use crate::types::{ScriptType, TransactionDetails, UTXO};

const FINAL_SETTLEMENT_DEPTH: u32 = 100;

struct RpcBlockchainConfig {
    client: Client,
    wallet_name: String,
    network: Network,
    rescan_since: u64,
}

pub struct RpcBlockchain(Option<RpcBlockchainConfig>);

impl RpcBlockchain {
    pub fn new(params: Option<(Client, String, Network, u64)>) -> Self {
        Self(params.map(
            |(client, wallet_name, network, rescan_since)| RpcBlockchainConfig {
                client,
                wallet_name,
                network,
                rescan_since,
            },
        ))
    }

    fn index_is_synced<D: BatchDatabase + DatabaseUtils>(
        &mut self,
        index: u32,
        database: &mut D,
    ) -> Result<bool, Error> {
        let last_script = database
            .get_script_pubkey_from_path(ScriptType::External, index)?
            .ok_or(Error::MissingScriptPubkey)?;
        let config = self.0.as_mut().ok_or(Error::OfflineClient)?;
        let address = Address::from_script(&last_script, config.network).unwrap();
        let response = config.client.get_address_info(&address)?;
        // iswatchonly has never been optional: https://github.com/bitcoin/bitcoin/commit/b98bfc5ed0da1efef1eff552a7e1a7ce9caf130f#diff-df7d84ff2f53fcb2a0dc15a3a51e55ceR3691
        Ok(response
            .is_watchonly
            .expect("iswatchonly should always be present"))
    }

    fn needs_sync_or_rescan<D: BatchDatabase + DatabaseUtils>(
        &mut self,
        database: &mut D,
    ) -> Result<(bool, bool), Error> {
        // TODO: batching
        let synced = match database.get_last_index(ScriptType::External)? {
            None => false,
            Some(last_index) => self.index_is_synced(last_index, database)?,
        };
        let needs_rescan = self.index_is_synced(0, database)?;
        Ok((!synced, needs_rescan))
    }

    fn importmulti<D: BatchDatabase + DatabaseUtils>(
        &mut self,
        rescan_since: Option<u64>,
        database: &mut D,
    ) -> Result<(), Error> {
        let (timestamp, rescan) = match rescan_since {
            Some(timestamp) => (ImportMultiRescanSince::Timestamp(timestamp), Some(true)),
            None => (ImportMultiRescanSince::Now, None),
        };
        // TODO: batching
        self.0
            .as_mut()
            .ok_or(Error::OfflineClient)?
            .client
            .import_multi(
                &database
                    .iter_script_pubkeys(Some(ScriptType::External))?
                    .iter()
                    .map(|script_pubkey| ImportMultiRequest {
                        script_pubkey: Some(ImportMultiRequestScriptPubkey::Script(&script_pubkey)),
                        watchonly: Some(true),
                        timestamp,
                        ..Default::default()
                    })
                    .collect::<Vec<_>>(),
                Some(&ImportMultiOptions { rescan }),
            )?;
        info!("Addresses imported");
        Ok(())
    }

    fn list_transactions<D: BatchDatabase + DatabaseUtils>(
        &mut self,
        database: &mut D,
    ) -> Result<Vec<(Transaction, Option<u32>)>, Error> {
        // Any height lower than "buried_height" is "settled"
        let db_tip_height = database.iter_txs(false)?.iter().fold(0, |acc, tx| {
            if let Some(height) = tx.height {
                std::cmp::max(acc, height)
            } else {
                acc
            }
        });
        let buried_height = if db_tip_height > FINAL_SETTLEMENT_DEPTH {
            db_tip_height - FINAL_SETTLEMENT_DEPTH
        } else {
            0
        };

        let ltx_count = 20;
        let mut ltx_skip = 0;

        // fetch unique list of wallet transactions
        let mut wallet_txs: Vec<ListTransactionResult> = vec![];
        loop {
            let mut settled_tx_observed = false;

            // Fetch next chunk of transactions
            let wallet_txs_chunk = self
                .0
                .as_mut()
                .ok_or(Error::OfflineClient)?
                .client
                .list_transactions(None, Some(ltx_count), Some(ltx_skip), Some(true))?;

            for wallet_tx in &wallet_txs_chunk {
                // Record whether we observed "settled" transaction and should stop iterating
                if (wallet_tx.info.blockheight.unwrap_or(std::i32::MAX) as u32) < buried_height {
                    settled_tx_observed = true;
                }

                // Collect transactions we don't have yet
                if wallet_txs
                    .iter()
                    .all(|wtx| wtx.info.txid != wallet_tx.info.txid)
                {
                    // FIXME
                    wallet_txs.push(wallet_tx.clone())
                }
            }

            // Stop fetching if chunk wasn't full or one transaction was "settled"
            let no_more = wallet_txs_chunk.len() < ltx_count;
            if no_more || settled_tx_observed {
                break;
            }

            // Update RPC cursor for next iteration
            ltx_skip += ltx_count;
        }

        // Sort wallet transactions from oldest to newest
        wallet_txs.sort_by(|a, b| b.info.confirmations.cmp(&a.info.confirmations));

        // Map wallet transactions to full transactions (TODO: batching)
        let mut txs: Vec<Transaction> = vec![];
        for wallet_tx in &wallet_txs {
            let tx = self
                .0
                .as_mut()
                .ok_or(Error::OfflineClient)?
                .client
                .get_transaction(&wallet_tx.info.txid, Some(true))?
                .transaction()?;
            txs.push(tx);
        }

        // Map wallet transactions to block heights (TODO: batching)
        let mut heights: Vec<Option<u32>> = vec![];
        for wallet_tx in &wallet_txs {
            let height = match wallet_tx.info.blockhash {
                Some(blockhash) => Some(
                    self.0
                        .as_mut()
                        .ok_or(Error::OfflineClient)?
                        .client
                        .get_block_info(&blockhash)?
                        .height as u32,
                ),
                None => None,
            };
            heights.push(height);
        }

        Ok(txs
            .iter()
            .zip(heights.iter())
            .map(|(tx, height)| (tx.to_owned(), height.to_owned())) // FIXME
            .collect())
    }
}

impl Blockchain for RpcBlockchain {
    fn offline() -> Self {
        RpcBlockchain(None)
    }

    fn is_online(&self) -> bool {
        self.0.is_some()
    }
}

impl OnlineBlockchain for RpcBlockchain {
    fn get_capabilities(&self) -> HashSet<Capability> {
        vec![].into_iter().collect()
    }

    fn setup<D: BatchDatabase + DatabaseUtils, P: Progress>(
        &mut self,
        _stop_gap: Option<usize>,
        _database: &mut D,
        _progress_update: P,
    ) -> Result<(), Error> {
        let config = self.0.as_mut().ok_or(Error::OfflineClient)?;

        // Check we support their node (rust-bitcoincore-rpc supports 0.18.0 and up)
        let version = config.client.version()?;
        if version < 180000 {
            return Err(Error::BitcoinRpcUnsupportedVersion);
        }

        // Attempt to load watch-only wallet
        // FIXME: use listwallets once rust-bitcoincore-rpc supports it
        match config.client.load_wallet(&config.wallet_name) {
            Ok(_) => info!("Loaded watch-only wallet: \"{}\"", &config.wallet_name),
            Err(load_wallet_err) => {
                // Return if watch-only wallet already exists
                if let RpcError::JsonRpc(jsonrpc::error::Error::Rpc(ref load_wallet_err)) =
                    load_wallet_err
                {
                    if load_wallet_err.message == format!("Wallet file verification failed: Error loading wallet {}. Duplicate -wallet filename specified.", config.wallet_name) {
                            info!("Watch-only wallet already loaded: \"{}\"", &config.wallet_name);
                            return Ok(());
                        }
                }
                // Otherwise, create a watch-only wallet
                match config.client.create_wallet(&config.wallet_name, Some(true)) {
                    Ok(_) => {
                        info!("Created watch-only wallet: \"{}\"", &config.wallet_name);
                    }
                    Err(error) => {
                        // FIXME
                        panic!(
                            "couldn't create watch-only bitcoin core wallet: {}",
                            error.to_string()
                        );
                    }
                };
            }
        };

        Ok(())
    }

    fn sync<D: BatchDatabase + DatabaseUtils, P: Progress>(
        &mut self,
        _stop_gap: Option<usize>,
        database: &mut D,
        _progress_update: P,
    ) -> Result<(), Error> {
        // If node doesn't recognize our most recent address, sync everything
        // If node doesn't recognize first address, rescan from genesis
        let (needs_sync_or_rescan, needs_rescan) = self.needs_sync_or_rescan(database)?;
        if needs_sync_or_rescan {
            if needs_rescan {
                let rescan_since = self.0.as_mut().ok_or(Error::OfflineClient)?.rescan_since;
                self.importmulti(Some(rescan_since), database)?;
            } else {
                self.importmulti(None, database)?;
            }
        }

        let mut updates = database.begin_batch();

        for (tx, height) in self.list_transactions(database)?.iter() {
            // Process inputs, remove spent UTXOs
            let mut sent: u64 = 0;
            for (i, input) in tx.input.iter().enumerate() {
                if let Some(previous_output) =
                    database.get_previous_output(&input.previous_output)?
                {
                    if database.is_mine(&previous_output.script_pubkey)? {
                        sent += previous_output.value;
                        debug!("{}:{} is mine, removing utxo", tx.txid(), i);
                        updates.del_utxo(&input.previous_output)?;
                    }
                }
            }

            // Process outputs, save new UTXOs
            let mut received: u64 = 0;
            for (i, output) in tx.output.iter().enumerate() {
                if let Some((_, _)) = database.get_path_from_script_pubkey(&output.script_pubkey)? {
                    debug!("{} output #{} is mine, adding utxo", tx.txid(), i);
                    updates.set_utxo(&UTXO {
                        outpoint: OutPoint::new(tx.txid(), i as u32),
                        txout: output.clone(),
                    })?;
                    received += output.value;
                }
            }

            // Save the transaction
            let details = TransactionDetails {
                transaction: Some(tx.clone()),
                txid: tx.txid(),
                height: *height,
                received,
                sent,
                timestamp: 0,
            };
            debug!("Saving tx: {}", tx.clone().txid());
            updates.set_tx(&details)?;
        }

        database.commit_batch(updates)?;
        info!("Saved transactions");

        Ok(())
    }

    fn get_tx(&mut self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        let response = self
            .0
            .as_mut()
            .ok_or(Error::OfflineClient)?
            .client
            .get_transaction(txid, Some(true))?;
        let tx = response.transaction()?;
        Ok(Some(tx))
    }

    fn broadcast(&mut self, tx: &Transaction) -> Result<(), Error> {
        self.0
            .as_mut()
            .ok_or(Error::OfflineClient)?
            .client
            .send_raw_transaction(tx)?;
        Ok(())
    }

    fn get_height(&mut self) -> Result<usize, Error> {
        let info = self
            .0
            .as_mut()
            .ok_or(Error::OfflineClient)?
            .client
            .get_blockchain_info()?;
        Ok(info.blocks as usize)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::descriptor::*;
    use crate::{sled, Wallet};
    use bitcoin::util::bip32::ExtendedPrivKey;
    use bitcoin::{Amount, Network};
    use bitcoincore_rpc::{Auth, Client};
    use dirs::home_dir;

    use std::str::FromStr;

    use rand::distributions::Alphanumeric;
    use rand::{thread_rng, Rng, RngCore};

    fn rand_str() -> String {
        thread_rng().sample_iter(&Alphanumeric).take(10).collect()
    }

    fn make_descriptors() -> (String, String) {
        let mut seed = vec![0u8; 16];
        thread_rng().fill_bytes(seed.as_mut_slice());

        let network = Network::Bitcoin;
        let sk = ExtendedPrivKey::new_master(network, &seed).unwrap();
        let external = format!("wpkh({}/0/*)", sk.to_string());
        let internal = format!("wpkh({}/1/*)", sk.to_string());
        (external, internal)
    }

    #[test]
    fn test_rpc_sync() -> Result<(), Error> {
        // Create a random wallet name
        let wallet_name = rand_str();

        // Create database
        let mut db_file = std::env::temp_dir();
        db_file.push(&wallet_name);
        db_file.push("database.sled");
        let database = sled::open(db_file.clone()).unwrap();
        let mut tree = database.open_tree("rpc").unwrap();

        // Create blockchain client
        let wallet_url = String::from(format!("http://127.0.0.1:18443/wallet/{}", wallet_name));
        let default_url = String::from("http://127.0.0.1:18443/wallet/");
        let path = std::path::PathBuf::from(format!(
            "{}/.bitcoin/regtest/.cookie",
            home_dir().unwrap().to_str().unwrap()
        ));
        let auth = Auth::CookieFile(path);
        let wallet_client = Client::new(wallet_url.clone(), auth.clone()).unwrap();
        let default_client = Client::new(default_url, auth.clone()).unwrap();
        let mut blockchain = RpcBlockchain::new(Some((
            wallet_client,
            wallet_name.clone(),
            Network::Regtest,
            0,
        )));

        // Run the setup
        blockchain.setup(None, &mut tree, NoopProgress)?;

        // Mine 150 blocks to default wallet
        let default_addr = default_client.get_new_address(None, None).unwrap();
        default_client
            .generate_to_address(150, &default_addr)
            .unwrap();

        // Send 1 BTC to each of first 21 wallet addresses, so that we need multiple
        // listtransactions calls
        let (desc_ext, desc_int) = make_descriptors();
        let extended = ExtendedDescriptor::from_str(&desc_ext).unwrap();
        for index in 0..21 {
            let derived = extended.derive(index).unwrap();
            let address = derived.address(Network::Regtest).unwrap();
            let amount = Amount::from_btc(1.0).unwrap();
            default_client
                .send_to_address(&address, amount, None, None, None, None, None, None)
                .unwrap();
        }

        // Mine another block so ^^ are confirmed
        default_client
            .generate_to_address(1, &default_addr)
            .unwrap();

        // Sync the wallet
        let wallet = Wallet::new(
            &desc_ext,
            Some(&desc_int),
            Network::Regtest,
            tree.clone(),
            blockchain,
        )
        .unwrap();
        wallet.sync(None, None).unwrap();

        // Check that RPC and database show same transactions
        let wallet_txs = wallet.list_transactions(false).unwrap();
        assert_eq!(21, wallet_txs.len());

        // Check unspents
        let wallet_unspent = wallet.list_unspent().unwrap();
        assert_eq!(21, wallet_unspent.len());

        // Check balances
        let wallet_client = Client::new(wallet_url, auth.clone()).unwrap();
        let wallet_balance = Amount::from_sat(wallet.get_balance().unwrap());
        let rpc_balance = wallet_client.get_balance(None, Some(true)).unwrap();
        assert_eq!(wallet_balance, rpc_balance);

        // Spend one utxo back to default wallet, mine a block, sync wallet
        let (psbt, _) = wallet
            .create_tx(
                vec![(default_addr.clone(), 100000000)],
                false,
                1.0 * 1e-5,
                None,
                None,
                None,
            )
            .unwrap();
        let (psbt, _) = wallet.sign(psbt, None).unwrap();
        let tx = psbt.extract_tx();
        wallet.broadcast(tx.clone()).unwrap();
        default_client
            .generate_to_address(1, &default_addr)
            .unwrap();
        wallet.sync(None, None).unwrap();

        // One more transaction, one less utxo
        assert_eq!(22, wallet.list_transactions(false).unwrap().len());
        assert_eq!(20, wallet.list_unspent().unwrap().len());

        let input_amount: u64 = tx
            .input
            .iter()
            .map(|i| {
                tree.get_previous_output(&i.previous_output)
                    .unwrap()
                    .unwrap()
                    .value
            })
            .sum();
        let output_amount: u64 = tx.output.iter().map(|o| o.value as u64).sum();
        let fee = input_amount - output_amount;
        assert_eq!(
            wallet_balance - Amount::from_btc(1.0).unwrap() - Amount::from_sat(fee),
            Amount::from_sat(wallet.get_balance().unwrap())
        );

        Ok(())
    }
}
