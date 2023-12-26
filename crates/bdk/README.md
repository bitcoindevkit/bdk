<div align="center">
  <h1>BDK</h1>

  <img src="https://raw.githubusercontent.com/bitcoindevkit/bdk/master/static/bdk.png" width="220" />

  <p>
    <strong>A modern, lightweight, descriptor-based wallet library written in Rust!</strong>
  </p>

  <p>
    <a href="https://crates.io/crates/bdk"><img alt="Crate Info" src="https://img.shields.io/crates/v/bdk.svg"/></a>
    <a href="https://github.com/bitcoindevkit/bdk/blob/master/LICENSE"><img alt="MIT or Apache-2.0 Licensed" src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg"/></a>
    <a href="https://github.com/bitcoindevkit/bdk/actions?query=workflow%3ACI"><img alt="CI Status" src="https://github.com/bitcoindevkit/bdk/workflows/CI/badge.svg"></a>
    <a href="https://coveralls.io/github/bitcoindevkit/bdk?branch=master"><img src="https://coveralls.io/repos/github/bitcoindevkit/bdk/badge.svg?branch=master"/></a>
    <a href="https://docs.rs/bdk"><img alt="API Docs" src="https://img.shields.io/badge/docs.rs-bdk-green"/></a>
    <a href="https://blog.rust-lang.org/2022/08/11/Rust-1.63.0.html"><img alt="Rustc Version 1.63.0+" src="https://img.shields.io/badge/rustc-1.63.0%2B-lightgrey.svg"/></a>
    <a href="https://discord.gg/d7NkDKm"><img alt="Chat on Discord" src="https://img.shields.io/discord/753336465005608961?logo=discord"></a>
  </p>

  <h4>
    <a href="https://bitcoindevkit.org">Project Homepage</a>
    <span> | </span>
    <a href="https://docs.rs/bdk">Documentation</a>
  </h4>
</div>

## `bdk`

The `bdk` crate provides the [`Wallet`](`crate::Wallet`) type which is a simple, high-level
interface built from the low-level components of [`bdk_chain`]. `Wallet` is a good starting point
for many simple applications as well as a good demonstration of how to use the other mechanisms to
construct a wallet. It has two keychains (external and internal) which are defined by
[miniscript descriptors][`rust-miniscript`] and uses them to generate addresses. When you give it
chain data it also uses the descriptors to find transaction outputs owned by them. From there, you
can create and sign transactions.

For more information, see the [`Wallet`'s documentation](https://docs.rs/bdk/latest/bdk/wallet/struct.Wallet.html).

### Blockchain data

In order to get blockchain data for `Wallet` to consume, you have to put it into particular form.
Right now this is [`KeychainScan`] which is defined in [`bdk_chain`].

This can be created manually or from blockchain-scanning crates.

**Blockchain Data Sources**

* [`bdk_esplora`]: Grabs blockchain data from Esplora for updating BDK structures.
* [`bdk_electrum`]: Grabs blockchain data from Electrum for updating BDK structures.

**Examples**

* [`example-crates/wallet_esplora`](https://github.com/bitcoindevkit/bdk/tree/master/example-crates/wallet_esplora)
* [`example-crates/wallet_electrum`](https://github.com/bitcoindevkit/bdk/tree/master/example-crates/wallet_electrum)

### Persistence

To persist the `Wallet` on disk, `Wallet` needs to be constructed with a
[`Persist`](https://docs.rs/bdk_chain/latest/bdk_chain/keychain/struct.KeychainPersist.html) implementation.

**Implementations**

* [`bdk_file_store`]: a simple flat-file implementation of `Persist`.

**Example**

```rust
use bdk::{bitcoin::Network, wallet::{AddressIndex, Wallet}};

fn main() {
    // a type that implements `Persist`
    let db = ();

    let descriptor = "wpkh(tprv8ZgxMBicQKsPdy6LMhUtFHAgpocR8GC6QmwMSFpZs7h6Eziw3SpThFfczTDh5rW2krkqffa11UpX3XkeTTB2FvzZKWXqPY54Y6Rq4AQ5R8L/84'/0'/0'/0/*)";
    let mut wallet = Wallet::builder(descriptor)
        .with_network(Network::Testnet)
        .init(db)
        .expect("should create");

    // get a new address (this increments revealed derivation index)
    println!("revealed address: {}", wallet.get_address(AddressIndex::New));
    println!("staged changes: {:?}", wallet.staged());
    // persist changes
    wallet.commit().expect("must save");
}
```

<!-- ### Sync the balance of a descriptor -->

<!-- ```rust,no_run -->
<!-- use bdk::Wallet; -->
<!-- use bdk::blockchain::ElectrumBlockchain; -->
<!-- use bdk::SyncOptions; -->
<!-- use bdk::electrum_client::Client; -->
<!-- use bdk::bitcoin::Network; -->

<!-- fn main() -> Result<(), bdk::Error> { -->
<!--     let blockchain = ElectrumBlockchain::from(Client::new("ssl://electrum.blockstream.info:60002")?); -->
<!--     let wallet = Wallet::new( -->
<!--         "wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/0/*)", -->
<!--         Some("wpkh([c258d2e4/84h/1h/0h]tpubDDYkZojQFQjht8Tm4jsS3iuEmKjTiEGjG6KnuFNKKJb5A6ZUCUZKdvLdSDWofKi4ToRCwb9poe1XdqfUnP4jaJjCB2Zwv11ZLgSbnZSNecE/1/*)"), -->
<!--         Network::Testnet, -->
<!--     )?; -->

<!--     wallet.sync(&blockchain, SyncOptions::default())?; -->

<!--     println!("Descriptor balance: {} SAT", wallet.get_balance()?); -->

<!--     Ok(()) -->
<!-- } -->
<!-- ``` -->
<!-- ### Generate a few addresses -->

<!-- ```rust -->
<!-- use bdk::Wallet; -->
<!-- use bdk::wallet::AddressIndex::New; -->
<!-- use bdk::bitcoin::Network; -->

<!-- fn main() -> Result<(), bdk::Error> { -->
<!--     let wallet = Wallet::new_no_persist( -->
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
<!-- use bdk::{FeeRate, Wallet, SyncOptions}; -->
<!-- use bdk::blockchain::ElectrumBlockchain; -->

<!-- use bdk::electrum_client::Client; -->
<!-- use bdk::wallet::AddressIndex::New; -->

<!-- use bitcoin::base64; -->
<!-- use bdk::bitcoin::consensus::serialize; -->
<!-- use bdk::bitcoin::Network; -->

<!-- fn main() -> Result<(), bdk::Error> { -->
<!--     let blockchain = ElectrumBlockchain::from(Client::new("ssl://electrum.blockstream.info:60002")?); -->
<!--     let wallet = Wallet::new_no_persist( -->
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
<!-- use bdk::{Wallet, SignOptions}; -->

<!-- use bitcoin::base64; -->
<!-- use bdk::bitcoin::consensus::deserialize; -->
<!-- use bdk::bitcoin::Network; -->

<!-- fn main() -> Result<(), bdk::Error> { -->
<!--     let wallet = Wallet::new_no_persist( -->
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

## License

Licensed under either of

* Apache License, Version 2.0, ([LICENSE-APACHE](../../LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
* MIT license ([LICENSE-MIT](../../LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

[`bdk_chain`]: https://docs.rs/bdk_chain/latest
[`bdk_file_store`]: https://docs.rs/bdk_file_store/latest
[`bdk_electrum`]: https://docs.rs/bdk_electrum/latest
[`bdk_esplora`]: https://docs.rs/bdk_esplora/latest
[`KeychainScan`]: https://docs.rs/bdk_chain/latest/bdk_chain/keychain/struct.KeychainScan.html
[`rust-miniscript`]: https://docs.rs/miniscript/latest/miniscript/index.html
