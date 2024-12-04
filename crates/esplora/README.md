# BDK Esplora

BDK Esplora extends [`esplora-client`] (with extension traits: [`EsploraExt`] and
[`EsploraAsyncExt`]) to update [`bdk_chain`] structures from an Esplora server.

The extension traits are primarily intended to satisfy [`SyncRequest`]s with [`sync`] and
[`FullScanRequest`]s with [`full_scan`].

## Usage

For blocking-only:
```toml
bdk_esplora = { version = "0.19", features = ["blocking"] }
```

For async-only:
```toml
bdk_esplora = { version = "0.19", features = ["async"] }
```

For async-only (with https):

You can additionally specify to use either rustls or native-tls, e.g. `async-https-native`, and this applies to both async and blocking features.
```toml
bdk_esplora = { version = "0.19", features = ["async-https"] }
```

For async-only (with tokio):
```toml
bdk_esplora = { version = "0.19", features = ["async", "tokio"] }
```

To use the extension traits:
```rust
// for blocking
#[cfg(feature = "blocking")]
use bdk_esplora::EsploraExt;

// for async
#[cfg(feature = "async")]
use bdk_esplora::EsploraAsyncExt;
```

For full examples, refer to [`example_wallet_esplora_blocking`](https://github.com/bitcoindevkit/bdk/tree/master/example-crates/example_wallet_esplora_blocking) and [`example_wallet_esplora_async`](https://github.com/bitcoindevkit/bdk/tree/master/example-crates/example_wallet_esplora_async).

[`esplora-client`]: https://docs.rs/esplora-client/
[`bdk_chain`]: https://docs.rs/bdk-chain/
[`EsploraExt`]: crate::EsploraExt
[`EsploraAsyncExt`]: crate::EsploraAsyncExt
[`SyncRequest`]: bdk_core::spk_client::SyncRequest
[`FullScanRequest`]: bdk_core::spk_client::FullScanRequest
[`sync`]: crate::EsploraExt::sync
[`full_scan`]: crate::EsploraExt::full_scan
