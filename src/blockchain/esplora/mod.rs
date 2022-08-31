//! Esplora
//!
//! This module defines a [`EsploraBlockchain`] struct that can query an Esplora
//! backend populate the wallet's [database](crate::database::Database) by:
//!
//! ## Example
//!
//! ```no_run
//! # use bdk::blockchain::esplora::EsploraBlockchain;
//! let blockchain = EsploraBlockchain::new("https://blockstream.info/testnet/api", 20);
//! # Ok::<(), bdk::Error>(())
//! ```
//!
//! Esplora blockchain can use either `ureq` or `reqwest` for the HTTP client
//! depending on your needs (blocking or async respectively).
//!
//! Please note, to configure the Esplora HTTP client correctly use one of:
//! Blocking:  --features='use-esplora-blocking'
//! Async:     --features='async-interface,use-esplora-async' --no-default-features

pub use esplora_client::Error as EsploraError;

#[cfg(feature = "use-esplora-async")]
mod r#async;

#[cfg(feature = "use-esplora-async")]
pub use self::r#async::*;

#[cfg(feature = "use-esplora-blocking")]
mod blocking;

#[cfg(feature = "use-esplora-blocking")]
pub use self::blocking::*;

/// Configuration for an [`EsploraBlockchain`]
#[derive(Debug, serde::Deserialize, serde::Serialize, Clone, PartialEq)]
pub struct EsploraBlockchainConfig {
    /// Base URL of the esplora service
    ///
    /// eg. `https://blockstream.info/api/`
    pub base_url: String,
    /// Optional URL of the proxy to use to make requests to the Esplora server
    ///
    /// The string should be formatted as: `<protocol>://<user>:<password>@host:<port>`.
    ///
    /// Note that the format of this value and the supported protocols change slightly between the
    /// sync version of esplora (using `ureq`) and the async version (using `reqwest`). For more
    /// details check with the documentation of the two crates. Both of them are compiled with
    /// the `socks` feature enabled.
    ///
    /// The proxy is ignored when targeting `wasm32`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// Number of parallel requests sent to the esplora service (default: 4)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u8>,
    /// Stop searching addresses for transactions after finding an unused gap of this length.
    pub stop_gap: usize,
    /// Socket timeout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

impl EsploraBlockchainConfig {
    /// create a config with default values given the base url and stop gap
    pub fn new(base_url: String, stop_gap: usize) -> Self {
        Self {
            base_url,
            proxy: None,
            timeout: None,
            stop_gap,
            concurrency: None,
        }
    }
}

impl From<esplora_client::BlockTime> for crate::BlockTime {
    fn from(esplora_client::BlockTime { timestamp, height }: esplora_client::BlockTime) -> Self {
        Self { timestamp, height }
    }
}

const DEFAULT_CONCURRENT_REQUESTS: u8 = 4;

#[cfg(feature = "test-esplora")]
#[cfg(test)]
mod test {
    use super::*;
    use crate::make_blockchain_tests;
    use crate::testutils::blockchain_tests::TestClient;

    fn init_blockchain(test_client: &TestClient) -> EsploraBlockchain {
        EsploraBlockchain::new(
            &format!(
                "http://{}",
                test_client.electrsd.esplora_url.as_ref().unwrap()
            ),
            20,
        )
    }

    make_blockchain_tests![
        init_blockchain,
        tests(
            test_sync_simple,
            test_sync_stop_gap_20,
            test_sync_before_and_after_receive,
            test_sync_multiple_outputs_same_tx,
            test_sync_receive_multi,
            test_sync_address_reuse,
            test_sync_receive_rbf_replaced,
            test_sync_after_send,
            test_sync_address_index_should_not_decrement,
            test_sync_address_index_should_increment,
            test_sync_double_receive,
            test_sync_many_sends_to_a_single_address,
            test_update_confirmation_time_after_generate,
            test_sync_outgoing_from_scratch,
            test_sync_long_change_chain,
            test_sync_bump_fee_basic,
            test_sync_bump_fee_remove_change,
            test_sync_bump_fee_add_input_simple,
            test_sync_bump_fee_add_input_no_change,
            test_add_data,
            test_sync_receive_coinbase,
            test_send_to_bech32m_addr,
            test_tx_chain,
            test_double_spend,
            test_send_receive_pkh,
            test_taproot_key_spend,
            test_taproot_script_spend,
            test_sign_taproot_core_keyspend_psbt,
            test_sign_taproot_core_scriptspend2_psbt,
            test_sign_taproot_core_scriptspend3_psbt,
            test_get_block_hash,
        )
    ];

    #[test]
    fn test_esplora_with_variable_configs() {
        use super::*;

        use crate::testutils::{
            blockchain_tests::TestClient,
            configurable_blockchain_tests::ConfigurableBlockchainTester,
        };

        struct EsploraTester;

        impl ConfigurableBlockchainTester<EsploraBlockchain> for EsploraTester {
            const BLOCKCHAIN_NAME: &'static str = "Esplora";

            fn config_with_stop_gap(
                &self,
                test_client: &mut TestClient,
                stop_gap: usize,
            ) -> Option<EsploraBlockchainConfig> {
                Some(EsploraBlockchainConfig {
                    base_url: format!(
                        "http://{}",
                        test_client.electrsd.esplora_url.as_ref().unwrap()
                    ),
                    proxy: None,
                    concurrency: None,
                    stop_gap: stop_gap,
                    timeout: None,
                })
            }
        }

        EsploraTester.run();
    }
}
