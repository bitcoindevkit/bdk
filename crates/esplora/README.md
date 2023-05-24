# BDK Esplora

BDK Esplora extends [`esplora_client`](crate::esplora_client) to update [`bdk_chain`] structures
from an Esplora server.

## Usage

There are two versions of the extension trait (blocking and async).

For blocking-only:
```toml
bdk_esplora = { version = "0.1", features = ["blocking"] }
```

For async-only:
```toml
bdk_esplora = { version = "0.1", features = ["async"] }
```

For async-only (with https):
```toml
bdk_esplora = { version = "0.1", features = ["async-https"] }
```

To use the extension traits:
```rust
// for blocking
use bdk_esplora::EsploraExt;
// for async
// use bdk_esplora::EsploraAsyncExt;
```

For full examples, refer to [`example-crates/wallet_esplora`](https://github.com/bitcoindevkit/bdk/tree/master/example-crates/wallet_esplora) (blocking) and [`example-crates/wallet_esplora_async`](https://github.com/bitcoindevkit/bdk/tree/master/example-crates/wallet_esplora_async).
