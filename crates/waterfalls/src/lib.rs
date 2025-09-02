use std::collections::{BTreeMap, HashSet};

use bdk_core::{
    bitcoin::{self, address::FromScriptError, Address, Network},
    spk_client::{FullScanRequest, FullScanResponse},
    BlockId, CheckPoint, ConfirmationBlockTime, TxUpdate,
};
use waterfalls_client::{api::WaterfallResponse, BlockingClient, Builder, TxStatus};

pub use waterfalls_client;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

pub struct Client {
    client: BlockingClient,
    network: Network,
}

// Open points:
// How to handle unbounded keychains?
// How to handle evicted txs?
// How to handle custom gap limit?

impl Client {
    pub fn new(network: Network, base_url: &str) -> Result<Self, Error> {
        Ok(Self {
            client: Builder::new(base_url).build_blocking(),
            network,
        })
    }

    pub fn broadcast(&self, tx: &bdk_core::bitcoin::Transaction) -> Result<(), Error> {
        self.client.broadcast(tx).map_err(|e| Box::new(e))?;
        Ok(())
    }

    pub fn full_scan<K: Ord + Clone, R: Into<FullScanRequest<K>>>(
        &self,
        request: R,
        _stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<FullScanResponse<K>, Error> {
        let mut request: FullScanRequest<K> = request.into();

        let keychain_addresses =
            get_addesses_from_request(&mut request, self.network).map_err(|e| Box::new(e))?;

        let mut chain_update = None;
        let mut tx_update = TxUpdate::default();
        let mut last_active_indices = BTreeMap::new();

        let mut waterfalls_responses = Vec::new();
        for (keychain, addresses) in keychain_addresses {
            // TODO: we can do a single call for multiple keychains
            let result = self
                .client
                .waterfalls_addresses(&addresses)
                .map_err(|e| Box::new(e))?;
            log::info!("result: {:?}", result);
            let index = get_last_active_index(&result);
            log::info!("last_active_index: {:?}", index);
            last_active_indices.insert(keychain, index);

            for tx_seens in result.txs_seen.get("addresses").unwrap().iter() {
                for tx_seen in tx_seens.iter() {
                    insert_anchor_or_seen_at_from_status(
                        &mut tx_update,
                        request.start_time(),
                        tx_seen.txid,
                        tx_status(tx_seen),
                    );
                }
            }

            if let Some(block_hash) = &result.tip {
                chain_update = Some(CheckPoint::new(BlockId {
                    height: 1000 as u32, // TODO: get height
                    hash: *block_hash,
                }));
            }

            waterfalls_responses.push(result);
        }
        let txids: HashSet<_> = waterfalls_responses
            .iter()
            .flat_map(|response| {
                response
                    .txs_seen
                    .get("addresses")
                    .unwrap()
                    .iter()
                    .flat_map(|txs| txs.iter().map(|tx| tx.txid))
            })
            .collect();

        log::info!("txids: {:?}", txids);

        // Fetch transactions in parallel using parallel_requests parameter
        let mut txids_iter = txids.into_iter();
        loop {
            let handles = txids_iter
                .by_ref()
                .take(parallel_requests.max(1)) // Ensure at least 1 request
                .map(|txid| {
                    let client = self.client.clone();
                    std::thread::spawn(move || {
                        client
                            .get_tx(&txid)
                            .map_err(|e| Box::new(e) as Error)
                            .map(|tx_opt| (txid, tx_opt))
                    })
                })
                .collect::<Vec<_>>();

            if handles.is_empty() {
                break;
            }

            for handle in handles {
                let (_txid, tx_opt) = handle.join().expect("thread must not panic")?;
                if let Some(tx) = tx_opt {
                    tx_update.txs.push(std::sync::Arc::new(tx));
                }
            }
        }

        Ok(FullScanResponse {
            chain_update,
            tx_update,
            last_active_indices,
        })
    }
}

fn get_addesses_from_request<K: Ord + Clone>(
    request: &mut FullScanRequest<K>,
    network: Network,
) -> Result<Vec<(K, Vec<Address>)>, FromScriptError> {
    let mut result = Vec::new();
    for (i, keychain) in request.keychains().iter().enumerate() {
        let mut addresses = Vec::new();
        let keychain_spks = request.iter_spks(keychain.clone());
        for (j, spk) in keychain_spks {
            addresses.push(Address::from_script(&spk, network)?);

            if addresses.len() == 50 {
                // TODO hanlde unbounded keychains
                break;
            }
        }
        log::info!("keychain_{}: addresses: {:?}", i, addresses);
        result.push((keychain.clone(), addresses));
    }
    Ok(result)
}

/// Find the index of the last non-empty element in the addresses array from waterfalls response
fn get_last_active_index(response: &WaterfallResponse) -> u32 {
    if let Some(addresses) = response.txs_seen.get("addresses") {
        // Find the last index that has non-empty transactions
        for (index, txs) in addresses.iter().enumerate().rev() {
            if !txs.is_empty() {
                return index as u32;
            }
        }
    }
    // If no addresses with transactions found, return 0
    0
}

fn tx_status(tx_seen: &waterfalls_client::api::TxSeen) -> TxStatus {
    TxStatus {
        block_height: (tx_seen.height != 0).then_some(tx_seen.height),
        block_hash: tx_seen.block_hash,
        block_time: tx_seen.block_timestamp.map(|t| t as u64),
        confirmed: tx_seen.height != 0,
    }
}

// copied from esplora/src/lib.rs
#[allow(dead_code)]
fn insert_anchor_or_seen_at_from_status(
    update: &mut TxUpdate<ConfirmationBlockTime>,
    start_time: u64,
    txid: bitcoin::Txid,
    status: TxStatus,
) {
    if let TxStatus {
        block_height: Some(height),
        block_hash: Some(hash),
        block_time: Some(time),
        ..
    } = status
    {
        let anchor = ConfirmationBlockTime {
            block_id: BlockId { height, hash },
            confirmation_time: time,
        };
        update.anchors.insert((anchor, txid));
    } else {
        update.seen_ats.insert((txid, start_time));
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use bdk_chain::{miniscript::Descriptor, SpkIterator};
    use bdk_core::{
        bitcoin::{key::Secp256k1, Address, Network},
        spk_client::FullScanRequest,
    };
    use bdk_testenv::anyhow;

    use crate::Client;

    #[test]
    pub fn test_full_scan() -> anyhow::Result<()> {
        let _ = env_logger::try_init();
        // Initialize a waterfalls TestEnv
        let rt = tokio::runtime::Runtime::new().unwrap();
        let test_env = rt.block_on(async {
            let exe = std::env::var("BITCOIND_EXEC").expect("BITCOIND_EXEC must be set");
            waterfalls::test_env::launch(exe, waterfalls::be::Family::Bitcoin).await
        });
        let url = test_env.base_url();

        let client = Client::new(Network::Regtest, &url).map_err(|e| anyhow::anyhow!("{}", e))?;

        // Now let's test the gap limit. First of all get a chain of 10 addresses.
        let addresses = [
            "bcrt1qj9f7r8r3p2y0sqf4r3r62qysmkuh0fzep473d2ar7rcz64wqvhssjgf0z4",
            "bcrt1qmm5t0ch7vh2hryx9ctq3mswexcugqe4atkpkl2tetm8merqkthas3w7q30",
            "bcrt1qut9p7ej7l7lhyvekj28xknn8gnugtym4d5qvnp5shrsr4nksmfqsmyn87g",
            "bcrt1qqz0xtn3m235p2k96f5wa2dqukg6shxn9n3txe8arlrhjh5p744hsd957ww",
            "bcrt1q9c0t62a8l6wfytmf2t9lfj35avadk3mm8g4p3l84tp6rl66m48sqrme7wu",
            "bcrt1qkmh8yrk2v47cklt8dytk8f3ammcwa4q7dzattedzfhqzvfwwgyzsg59zrh",
            "bcrt1qvgrsrzy07gjkkfr5luplt0azxtfwmwq5t62gum5jr7zwcvep2acs8hhnp2",
            "bcrt1qw57edarcg50ansq8mk3guyrk78rk0fwvrds5xvqeupteu848zayq549av8",
            "bcrt1qvtve5ekf6e5kzs68knvnt2phfw6a0yjqrlgat392m6zt9jsvyxhqfx67ef",
            "bcrt1qw03ddumfs9z0kcu76ln7jrjfdwam20qtffmkcral3qtza90sp9kqm787uk",
        ];
        let addresses: Vec<_> = addresses
            .into_iter()
            .map(|s| Address::from_str(s).unwrap().assume_checked())
            .collect();
        let spks: Vec<_> = addresses
            .iter()
            .enumerate()
            .map(|(i, addr)| (i as u32, addr.script_pubkey()))
            .collect();

        // Send coins to one of the addresses from the bitcoin node.
        // Convert the first address to waterfalls format and send coins to it
        let waterfalls_address = waterfalls::be::Address::Bitcoin(addresses[3].clone());
        let _txid = test_env.send_to(&waterfalls_address, 10000);
        rt.block_on(test_env.node_generate(1));

        let full_scan_update = {
            let request = FullScanRequest::builder().spks_for_keychain(0, spks.clone());
            client
                .full_scan(request, 0, 4)
                .map_err(|e| anyhow::anyhow!("{}", e))?
        };

        assert_eq!(full_scan_update.last_active_indices.get(&0).unwrap(), &3u32);

        rt.block_on(test_env.shutdown());

        Ok(())
    }

    #[test]
    #[ignore = "requires prod server and internet"]
    pub fn test_signet() {
        let _ = env_logger::try_init();
        let base_url = "https://waterfalls.liquidwebwallet.org/bitcoinsignet/api";
        let client = Client::new(Network::Signet, base_url).unwrap();

        let waterfalls_client = waterfalls_client::Builder::new(base_url).build_blocking();
        let external = "tr(tpubDDh1wUM29wsoJnHomNYrEwhGainWHUSzErfNrsZKiCjQWWUjFLwhtAqWvGUKc4oESXqcGKdbPDv7fBDsPHPYitNuGNrJ9BKrW1GPxUyiUUb/0/*)";
        let internal = "tr(tpubDDh1wUM29wsoJnHomNYrEwhGainWHUSzErfNrsZKiCjQWWUjFLwhtAqWvGUKc4oESXqcGKdbPDv7fBDsPHPYitNuGNrJ9BKrW1GPxUyiUUb/1/*)";

        let resp = waterfalls_client.waterfalls(&external).unwrap();
        log::info!("resp: {:?}", resp);
        let resp2 = waterfalls_client.waterfalls(&internal).unwrap();
        log::info!("resp2: {:?}", resp2);

        let secp = Secp256k1::new();
        let (external_descriptor, _) = Descriptor::parse_descriptor(&secp, external).unwrap();
        let (internal_descriptor, _) = Descriptor::parse_descriptor(&secp, internal).unwrap();
        let request = FullScanRequest::builder()
            .spks_for_keychain(0, SpkIterator::new_with_range(external_descriptor, 0..100))
            .spks_for_keychain(1, SpkIterator::new_with_range(internal_descriptor, 0..100));
        let full_scan_update = client.full_scan(request, 0, 4).unwrap();
    }
}
