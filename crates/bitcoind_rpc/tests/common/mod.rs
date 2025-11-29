use bdk_testenv::anyhow;
use bdk_testenv::TestEnv;

/// This trait is used for testing. It allows creating a new [`bitcoincore_rpc::Client`] connected
/// to the instance of bitcoind running in the test environment. This way the `TestEnv` and the
/// `Emitter` aren't required to share the same client. In the future when we no longer depend on
/// `bitcoincore-rpc`, this can be updated to return the production client that is used by BDK.
pub trait ClientExt {
    /// Creates a new [`bitcoincore_rpc::Client`] connected to the current node instance.
    fn get_rpc_client(&self) -> anyhow::Result<bitcoincore_rpc::Client>;
}

impl ClientExt for TestEnv {
    fn get_rpc_client(&self) -> anyhow::Result<bitcoincore_rpc::Client> {
        Ok(bitcoincore_rpc::Client::new(
            &self.bitcoind.rpc_url(),
            bitcoincore_rpc::Auth::CookieFile(self.bitcoind.params.cookie_file.clone()),
        )?)
    }
}
