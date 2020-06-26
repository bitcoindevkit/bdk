use std::collections::HashSet;

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use bitcoin::{
    util::bip32::{ChildNumber, DerivationPath},
    Address, Network, OutPoint, Transaction, Txid,
};

use bitcoincore_rpc::{
    json::{
        ImportMultiOptions, ImportMultiRequest, ImportMultiRequestScriptPubkey,
        ImportMultiRescanSince,
    },
    Client, RpcApi,
};

use super::*;
use crate::database::{BatchDatabase, BatchOperations, DatabaseUtils};
use crate::error::Error;
use crate::types::{ScriptType, TransactionDetails, UTXO};

pub struct BitcoinRpcBlockchain(Option<Client>);

impl BitcoinRpcBlockchain {
    // FIXME: why can I just say BitcoinRpcBlockchain(Some(client))?
    pub fn new(client: Option<Client>) -> Self {
        Self(client)
    }
    async fn index_is_synced<D: BatchDatabase + DatabaseUtils>(
        &mut self,
        index: u32,
        database: &mut D,
    ) -> Result<bool, Error> {
        let networks = vec![Network::Bitcoin, Network::Testnet, Network::Regtest];
        let path = DerivationPath::from(vec![ChildNumber::Normal { index }]);
        let last_script = database
            .get_script_pubkey_from_path(ScriptType::External, &path)?
            .unwrap();
        let synced = networks
            .iter()
            .map(|network| {
                let address = Address::from_script(&last_script, *network).unwrap();
                let response = self.0.as_mut().unwrap().get_address_info(&address);
                if let Ok(result) = response {
                    if let Some(is_watchonly) = result.is_watchonly {
                        return is_watchonly;
                    }
                }
                false
            })
            .any(|x| x);
        Ok(synced)
    }

    async fn needs_sync<D: BatchDatabase + DatabaseUtils>(
        &mut self,
        database: &mut D,
    ) -> Result<(bool, bool), Error> {
        let synced = match database.get_last_index(ScriptType::External)? {
            None => false,
            Some(last_index) => self.index_is_synced(last_index, database).await?,
        };
        let needs_rescan = self.index_is_synced(0, database).await?;
        Ok((!synced, needs_rescan))
    }

    async fn importmulti<D: BatchDatabase + DatabaseUtils>(
        &mut self,
        rescan_since: Option<u64>,
        database: &mut D,
    ) -> Result<(), Error> {
        let (timestamp, rescan) = match rescan_since {
            Some(timestamp) => (ImportMultiRescanSince::Timestamp(timestamp), Some(true)),
            None => (ImportMultiRescanSince::Now, None),
        };
        self.0.as_mut().unwrap().import_multi(
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
        info!("Successfully imported addresses over bitcoin RPC");
        Ok(())
    }
}

impl Blockchain for BitcoinRpcBlockchain {
    fn offline() -> Self {
        BitcoinRpcBlockchain(None)
    }

    fn is_online(&self) -> bool {
        self.0.is_some()
    }
}

#[async_trait(?Send)]
impl OnlineBlockchain for BitcoinRpcBlockchain {
    async fn get_capabilities(&self) -> HashSet<Capability> {
        vec![].into_iter().collect()
    }

    async fn setup<D: BatchDatabase + DatabaseUtils, P: Progress>(
        &mut self,
        _stop_gap: Option<usize>,
        database: &mut D,
        _progress_update: P,
    ) -> Result<(), Error> {
        // If node doesn't recognize our most recent address, sync everything
        // If node doesn't recognize first address, rescan from genesis
        let (needs_sync, needs_rescan) = self.needs_sync(database).await?;
        if needs_sync {
            if needs_rescan {
                self.importmulti(Some(0), database).await?;
            } else {
                self.importmulti(None, database).await?;
            }
        }

        let ltx_count = 20;
        let mut ltx_skip = 0;
        let mut updates = database.begin_batch();
        let mut processed_txids: Vec<Txid> = vec![];

        loop {
            // Fetch next chunk of transactions
            let ltxs = self.0.as_mut().unwrap().list_transactions(
                None,
                Some(ltx_count),
                Some(ltx_skip),
                Some(true),
            )?;

            for ltx in ltxs.iter() {
                let height = self
                    .0
                    .as_mut()
                    .unwrap()
                    .get_block_info(&ltx.info.blockhash.unwrap()) // FIXME: don't unwrap
                    .unwrap()
                    .height;

                // Only process transactions once
                if processed_txids.contains(&ltx.info.txid) {
                    continue;
                }

                // Get full transactions (we don't have transaction hex yet)
                let tx = self
                    .0
                    .as_mut()
                    .unwrap()
                    .get_transaction(&ltx.info.txid, Some(true))
                    .unwrap()
                    .transaction()
                    .unwrap();

                // Process inputs deleting spent UTXOs
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

                // Process outputs saving UTXOs
                let mut received: u64 = 0;
                for (i, output) in tx.output.iter().enumerate() {
                    if let Some((_, _)) =
                        database.get_path_from_script_pubkey(&output.script_pubkey)?
                    {
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
                    height: Some(height as u32),
                    received,
                    sent,
                    timestamp: 0,
                };
                debug!("Saving tx: {}", tx.clone().txid());
                updates.set_tx(&details).unwrap();

                // Mark that we've processed this txid
                processed_txids.push(ltx.info.txid);
            }

            // Exit loop if no transactions remain, otherwise increment ltx_skip
            if ltxs.len() < ltx_count {
                break;
            } else {
                ltx_skip += ltx_count;
            }
        }

        database.commit_batch(updates)?;
        info!("Saved transactions");

        Ok(())
    }

    async fn get_tx(&mut self, txid: &Txid) -> Result<Option<Transaction>, Error> {
        let response = self.0.as_mut().unwrap().get_transaction(txid, Some(true))?;
        let tx = response.transaction()?;
        Ok(Some(tx))
    }

    async fn broadcast(&mut self, tx: &Transaction) -> Result<(), Error> {
        self.0.as_mut().unwrap().send_raw_transaction(tx)?;
        Ok(())
    }

    async fn get_height(&mut self) -> Result<usize, Error> {
        let info = self.0.as_mut().unwrap().get_blockchain_info()?;
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

    #[tokio::test]
    async fn test_rpc_sync() {
        // Create a random wallet name
        let wallet_name = rand_str();

        // Create database
        let mut db_file = std::env::temp_dir();
        db_file.push(&wallet_name);
        db_file.push("database.sled");
        let database = sled::open(db_file.clone()).unwrap();
        let tree = database.open_tree("rpc").unwrap();

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
        let blockchain = BitcoinRpcBlockchain::new(Some(wallet_client));

        // Create watch-only wallet
        default_client
            .create_wallet(&wallet_name, Some(true))
            .unwrap();

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
        .await
        .unwrap();
        wallet.sync(None, None).await.unwrap();

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
        wallet.broadcast(tx.clone()).await.unwrap();
        default_client
            .generate_to_address(1, &default_addr)
            .unwrap();
        wallet.sync(None, None).await.unwrap();

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
    }
}
