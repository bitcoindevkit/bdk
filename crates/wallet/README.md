<div align="center">
  <h1>BDK</h1>

  <img src="https://raw.githubusercontent.com/bitcoindevkit/bdk/master/static/bdk.png" width="220" />

  <p>
    <strong>A modern, lightweight, descriptor-based wallet library written in Rust!</strong>
  </p>

  <p>
    <a href="https://crates.io/crates/bdk_wallet"><img alt="Crate Info" src="https://img.shields.io/crates/v/bdk_wallet.svg"/></a>
    <a href="https://github.com/bitcoindevkit/bdk/blob/master/LICENSE"><img alt="MIT or Apache-2.0 Licensed" src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg"/></a>
    <a href="https://github.com/bitcoindevkit/bdk/actions?query=workflow%3ACI"><img alt="CI Status" src="https://github.com/bitcoindevkit/bdk/workflows/CI/badge.svg"></a>
    <a href="https://coveralls.io/github/bitcoindevkit/bdk?branch=master"><img src="https://coveralls.io/repos/github/bitcoindevkit/bdk/badge.svg?branch=master"/></a>
    <a href="https://docs.rs/bdk_wallet"><img alt="API Docs" src="https://img.shields.io/badge/docs.rs-bdk_wallet-green"/></a>
    <a href="https://blog.rust-lang.org/2022/08/11/Rust-1.63.0.html"><img alt="Rustc Version 1.63.0+" src="https://img.shields.io/badge/rustc-1.63.0%2B-lightgrey.svg"/></a>
    <a href="https://discord.gg/d7NkDKm"><img alt="Chat on Discord" src="https://img.shields.io/discord/753336465005608961?logo=discord"></a>
  </p>

  <h4>
    <a href="https://bitcoindevkit.org">Project Homepage</a>
    <span> | </span>
    <a href="https://docs.rs/bdk_wallet">Documentation</a>
  </h4>
</div>

# BDK Wallet

The `bdk_wallet` crate provides the [`Wallet`] type which is a simple, high-level
interface built from the low-level components of [`bdk_chain`]. `Wallet` is a good starting point
for many simple applications as well as a good demonstration of how to use the other mechanisms to
construct a wallet. It has two keychains (external and internal) which are defined by
[miniscript descriptors][`rust-miniscript`] and uses them to generate addresses. When you give it
chain data it also uses the descriptors to find transaction outputs owned by them. From there, you
can create and sign transactions.

For details about the API of `Wallet` see the [module-level documentation][`Wallet`].

## Blockchain data

In order to get blockchain data for `Wallet` to consume, you should configure a client from
an available chain source. Typically you make a request to the chain source and get a response
that the `Wallet` can use to update its view of the chain.

**Blockchain Data Sources**

* [`bdk_esplora`]: Grabs blockchain data from Esplora for updating BDK structures.
* [`bdk_electrum`]: Grabs blockchain data from Electrum for updating BDK structures.
* [`bdk_bitcoind_rpc`]: Grabs blockchain data from Bitcoin Core for updating BDK structures.

**Examples**

* [`example-crates/wallet_esplora_async`](https://github.com/bitcoindevkit/bdk/tree/master/example-crates/wallet_esplora_async)
* [`example-crates/wallet_esplora_blocking`](https://github.com/bitcoindevkit/bdk/tree/master/example-crates/wallet_esplora_blocking)
* [`example-crates/wallet_electrum`](https://github.com/bitcoindevkit/bdk/tree/master/example-crates/wallet_electrum)
* [`example-crates/wallet_rpc`](https://github.com/bitcoindevkit/bdk/tree/master/example-crates/wallet_rpc)

## Persistence

To persist `Wallet` state data use a data store crate that reads and writes [`bdk_chain::WalletChangeSet`].

**Implementations**

* [`bdk_file_store`]: Stores wallet changes in a simple flat file.

**Example**

<!-- compile_fail because outpoint and txout are fake variables -->
```rust,no_run
use bdk_wallet::{bitcoin::Network, CreateParams, LoadParams, KeychainKind, ChangeSet};

// Open or create a new file store for wallet data.
let mut db =
    bdk_file_store::Store::<ChangeSet>::open_or_create_new(b"magic_bytes", "/tmp/my_wallet.db")
        .expect("create store");

// Create a wallet with initial wallet data read from the file store.
let network = Network::Testnet;
let descriptor = "wpkh(tprv8ZgxMBicQKsPdcAqYBpzAFwU5yxBUo88ggoBqu1qPcHUfSbKK1sKMLmC7EAk438btHQrSdu3jGGQa6PA71nvH5nkDexhLteJqkM4dQmWF9g/84'/1'/0'/0/*)";
let change_descriptor = "wpkh(tprv8ZgxMBicQKsPdcAqYBpzAFwU5yxBUo88ggoBqu1qPcHUfSbKK1sKMLmC7EAk438btHQrSdu3jGGQa6PA71nvH5nkDexhLteJqkM4dQmWF9g/84'/1'/0'/1/*)";
let load_params = LoadParams::with_descriptors(descriptor, change_descriptor, network)
    .expect("must parse descriptors");
let create_params = CreateParams::new(descriptor, change_descriptor, network)
    .expect("must parse descriptors");
let mut wallet = match load_params.load_wallet(&mut db).expect("wallet") {
    Some(wallet) => wallet,
    None => create_params.create_wallet(&mut db).expect("wallet"),
};

// Get a new address to receive bitcoin.
let receive_address = wallet.reveal_next_address(KeychainKind::External);
// Persist staged wallet data changes to the file store.
wallet.persist(&mut db).expect("persist");
println!("Your new receive address is: {}", receive_address.address);
```

<!-- ### Sync the balance of a descriptor -->

<!-- ```rust,no_run -->
<!-- use bdk_wallet::Wallet; -->
<!-- use bdk_wallet::blockchain::ElectrumBlockchain; -->
<!-- use bdk_wallet::SyncOptions; -->
<!-- use bdk_wallet::electrum_client::Client; -->
<!-- use bdk_wallet::bitcoin::Network; -->

<!-- fn main() -> Result<(), bdk_wallet::Error> { -->
<!--     let blockchain = ElectrumBlockchain::from(Client::new("ssl://electrum.blockstream.info:60002")?); -->
<!--     let wallet = Wallet::new( -->
<!--         "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/0/*)", -->
<!--         Some("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/1/*)"), -->
<!--         Network::Testnet, -->
<!--     )?; -->

<!--     wallet.sync(&blockchain, SyncOptions::default())?; -->

<!--     println!("Descriptor balance: {} SAT", wallet.balance()?); -->

<!--     Ok(()) -->
<!-- } -->
<!-- ``` -->
<!-- ### Generate a few addresses -->

<!-- ```rust -->
<!-- use bdk_wallet::Wallet; -->
<!-- use bdk_wallet::wallet::AddressIndex::New; -->
<!-- use bdk_wallet::bitcoin::Network; -->

<!-- fn main() -> Result<(), bdk_wallet::Error> { -->
<!--     let wallet = Wallet::new( -->
<!--         "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/0/*)", -->
<!--         Some("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/1/*)"), -->
<!--         Network::Testnet, -->
<!--     )?; -->

<!--     println!("Address #0: {}", wallet.get_address(New)); -->
<!--     println!("Address #1: {}", wallet.get_address(New)); -->
<!--     println!("Address #2: {}", wallet.get_address(New)); -->

<!--     Ok(()) -->
<!-- } -->
<!-- ``` -->

<!-- ### Create a transaction -->

<!-- ```rust,no_run -->
<!-- use bdk_wallet::{FeeRate, Wallet, SyncOptions}; -->
<!-- use bdk_wallet::blockchain::ElectrumBlockchain; -->

<!-- use bdk_wallet::electrum_client::Client; -->
<!-- use bdk_wallet::wallet::AddressIndex::New; -->

<!-- use bitcoin::base64; -->
<!-- use bdk_wallet::bitcoin::consensus::serialize; -->
<!-- use bdk_wallet::bitcoin::Network; -->

<!-- fn main() -> Result<(), bdk_wallet::Error> { -->
<!--     let blockchain = ElectrumBlockchain::from(Client::new("ssl://electrum.blockstream.info:60002")?); -->
<!--     let wallet = Wallet::new( -->
<!--         "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/0/*)", -->
<!--         Some("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/1/*)"), -->
<!--         Network::Testnet, -->
<!--     )?; -->

<!--     wallet.sync(&blockchain, SyncOptions::default())?; -->

<!--     let send_to = wallet.get_address(New); -->
<!--     let (psbt, details) = { -->
<!--         let mut builder = wallet.build_tx(); -->
<!--         builder -->
<!--             .add_recipient(send_to.script_pubkey(), 50_000) -->
<!--             .enable_rbf() -->
<!--             .do_not_spend_change() -->
<!--             .fee_rate(FeeRate::from_sat_per_vb(5.0)); -->
<!--         builder.finish()? -->
<!--     }; -->

<!--     println!("Transaction details: {:#?}", details); -->
<!--     println!("Unsigned PSBT: {}", base64::encode(&serialize(&psbt))); -->

<!--     Ok(()) -->
<!-- } -->
<!-- ``` -->

<!-- ### Sign a transaction -->

<!-- ```rust,no_run -->
<!-- use bdk_wallet::{Wallet, SignOptions}; -->

<!-- use bitcoin::base64; -->
<!-- use bdk_wallet::bitcoin::consensus::deserialize; -->
<!-- use bdk_wallet::bitcoin::Network; -->

<!-- fn main() -> Result<(), bdk_wallet::Error> { -->
<!--     let wallet = Wallet::new( -->
<!--         "wpkh([c258d2e4/84h/1h/0h]tprv8griRPhA7342zfRyB6CqeKF8CJDXYu5pgnj1cjL1u2ngKcJha5jjTRimG82ABzJQ4MQe71CV54xfn25BbhCNfEGGJZnxvCDQCd6JkbvxW6h/0/*)", -->
<!--         Some("wpkh([c258d2e4/84h/1h/0h]tprv8griRPhA7342zfRyB6CqeKF8CJDXYu5pgnj1cjL1u2ngKcJha5jjTRimG82ABzJQ4MQe71CV54xfn25BbhCNfEGGJZnxvCDQCd6JkbvxW6h/1/*)"), -->
<!--         Network::Testnet, -->
<!--     )?; -->

<!--     let psbt = "..."; -->
<!--     let mut psbt = deserialize(&base64::decode(psbt).unwrap())?; -->

<!--     let _finalized = wallet.sign(&mut psbt, SignOptions::default())?; -->

<!--     Ok(()) -->
<!-- } -->
<!-- ``` -->

## Testing

### Unit testing

```bash
cargo test
```

# License

Licensed under either of

* Apache License, Version 2.0, ([LICENSE-APACHE](../../LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
* MIT license ([LICENSE-MIT](../../LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

# Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

[`Wallet`]: https://docs.rs/bdk_wallet/latest/bdk_wallet/wallet/struct.Wallet.html
[`bdk_chain`]: https://docs.rs/bdk_chain/latest
[`bdk_file_store`]: https://docs.rs/bdk_file_store/latest
[`bdk_electrum`]: https://docs.rs/bdk_electrum/latest
[`bdk_esplora`]: https://docs.rs/bdk_esplora/latest
[`bdk_bitcoind_rpc`]: https://docs.rs/bdk_bitcoind_rpc/latest
[`rust-miniscript`]: https://docs.rs/miniscript/latest/miniscript/index.html
