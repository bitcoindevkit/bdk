use std::collections::BTreeMap;

use bdk_core::{
    bitcoin::{address::FromScriptError, Address, Network},
    spk_client::{FullScanRequest, FullScanResponse},
    TxUpdate,
};
use waterfalls_client::{api::WaterfallResponse, BlockingClient, Builder};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

pub struct Client {
    client: BlockingClient,
    network: Network,
}

impl Client {
    pub fn new(network: Network, base_url: &str) -> Result<Self, Error> {
        Ok(Self {
            client: Builder::new(base_url).build_blocking(),
            network,
        })
    }

    pub fn full_scan<K: Ord + Clone, R: Into<FullScanRequest<K>>>(
        &self,
        request: R,
        _stop_gap: usize,
        _parallel_requests: usize,
    ) -> Result<FullScanResponse<K>, Error> {
        let mut request: FullScanRequest<K> = request.into();

        let keychain_addresses =
            get_addesses_from_request(&mut request, self.network).map_err(|e| Box::new(e))?;

        let chain_update = None; // TODO handle chain update
        let tx_update = TxUpdate::default();
        let mut last_active_indices = BTreeMap::new();

        for (keychain, addresses) in keychain_addresses {
            // TODO: we can do a single call for multiple keychains
            let result = self
                .client
                .waterfalls_addresses(&addresses)
                .map_err(|e| Box::new(e))?;
            let index = get_last_active_index(&result);
            last_active_indices.insert(keychain, index);
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
        for (_, spk) in keychain_spks {
            addresses.push(Address::from_script(&spk, network)?);
        }
        println!("keychain_{}: addresses: {:?}", i, addresses);
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

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use bdk_core::{
        bitcoin::{Address, Network},
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
                .full_scan(request, 0, 0)
                .map_err(|e| anyhow::anyhow!("{}", e))?
        };

        assert_eq!(full_scan_update.last_active_indices.get(&0).unwrap(), &3u32);

        rt.block_on(test_env.shutdown());

        Ok(())
    }
}
