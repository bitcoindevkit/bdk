use bdk_bitcoind_client::Auth;
use bdk_testenv::anyhow;
use bdk_testenv::TestEnv;

/// This trait is used for testing. It allows creating a new [`bdk_bitcoind_client::Client`]
/// connected to the instance of bitcoind running in the test environment. This way the `TestEnv`
/// and the `Emitter` aren't required to share the same client. In the future when we no longer
/// depend on `bitcoincore-rpc`, this can be updated to return the production client that is used by
/// BDK.
pub trait ClientExt {
    /// Creates a new [`bdk_bitcoin_client::Client`] connected to the current node instance.
    fn get_rpc_client(&self) -> anyhow::Result<bdk_bitcoind_client::Client>;
}

impl ClientExt for TestEnv {
    fn get_rpc_client(&self) -> anyhow::Result<bdk_bitcoind_client::Client> {
        Ok(bdk_bitcoind_client::Client::with_auth(
            &self.bitcoind.rpc_url(),
            Auth::CookieFile(self.bitcoind.params.cookie_file.clone()),
        )?)
    }
}
