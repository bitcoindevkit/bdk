# BDK Esplora

BDK Esplora extends [`esplora-client`] (with extension traits: [`EsploraExt`] and
[`EsploraAsyncExt`]) to update [`bdk_chain`] structures from an Esplora server.

The extension traits are primarily intended to satisfy [`SyncRequest`]s with [`sync`] and
[`FullScanRequest`]s with [`full_scan`].

## Dependencies

`esplora` depends on [Blockstream's version of `electrs`](https://github.com/Blockstream/electrs)
being installed and available in `PATH` or in the `ELECTRS_EXEC` ENV variable.

## Usage

For blocking-only:
```toml
bdk_esplora = { version = "0.3", features = ["blocking"] }
```

For async-only:
```toml
bdk_esplora = { version = "0.3", features = ["async"] }
```

For async-only (with https):
```toml
bdk_esplora = { version = "0.3", features = ["async-https"] }
```

To use the extension traits:
```rust
// for blocking
use bdk_esplora::EsploraExt;
// for async
use bdk_esplora::EsploraAsyncExt;
```

For full examples, refer to [`example-crates/wallet_esplora_blocking`](https://github.com/bitcoindevkit/bdk/tree/master/example-crates/wallet_esplora_blocking) and [`example-crates/wallet_esplora_async`](https://github.com/bitcoindevkit/bdk/tree/master/example-crates/wallet_esplora_async).

[`esplora-client`]: https://docs.rs/esplora-client/
[`bdk_chain`]: https://docs.rs/bdk-chain/
[`EsploraExt`]: crate::EsploraExt
[`EsploraAsyncExt`]: crate::EsploraAsyncExt
[`SyncRequest`]: bdk_core::spk_client::SyncRequest
[`FullScanRequest`]: bdk_core::spk_client::FullScanRequest
[`sync`]: crate::EsploraExt::sync
[`full_scan`]: crate::EsploraExt::full_scan
