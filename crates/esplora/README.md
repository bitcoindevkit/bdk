# BDK Esplora

BDK Esplora extends [`esplora-client`] (with extension traits: [`EsploraExt`] and
[`EsploraAsyncExt`]) to update [`bdk_chain`] structures from an Esplora server.

The extension traits are primarily intended to satisfy [`ScanRequest`]s with [`scan`]. This
unifies the work previously split across [`sync`] for explicit scripts/txids/outpoints and
[`full_scan`] for keychain discovery. [`scan`] is intended to replace [`sync`] and [`full_scan`].

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

For full examples, refer to [`example_wallet_esplora_blocking`](https://github.com/bitcoindevkit/bdk/tree/master/examples/example_wallet_esplora_blocking) and [`example_wallet_esplora_async`](https://github.com/bitcoindevkit/bdk/tree/master/examples/example_wallet_esplora_async).

[`esplora-client`]: https://docs.rs/esplora-client/
[`bdk_chain`]: https://docs.rs/bdk-chain/
[`EsploraExt`]: crate::EsploraExt
[`EsploraAsyncExt`]: crate::EsploraAsyncExt
[`ScanRequest`]: bdk_core::spk_client::ScanRequest
[`SyncRequest`]: bdk_core::spk_client::SyncRequest
[`FullScanRequest`]: bdk_core::spk_client::FullScanRequest
[`scan`]: crate::EsploraExt::scan
[`sync`]: crate::EsploraExt::sync
[`full_scan`]: crate::EsploraExt::full_scan
