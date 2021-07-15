# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Wallet

- Removed and replaced `set_single_recipient` with more general `drain_to` and replaced `maintain_single_recipient` with `allow_shrinking`.

### Blockchain

- Removed `stop_gap` from `Blockchain` trait and added it to only `ElectrumBlockchain` and `EsploraBlockchain` structs  

## [v0.9.0] - [v0.8.0]

### Wallet

- Added Bitcoin core RPC added as blockchain backend
- Added a `verify` feature that can be enable to verify the unconfirmed txs we download against the consensus rules

## [v0.8.0] - [v0.7.0]

### Wallet
- Added an option that must be explicitly enabled to allow signing using non-`SIGHASH_ALL` sighashes (#350)
#### Changed
`get_address` now returns an `AddressInfo` struct that includes the index and derefs to `Address`.

## [v0.7.0] - [v0.6.0]

### Policy
#### Changed
Removed `fill_satisfaction` method in favor of enum parameter in `extract_policy` method

#### Added
Timelocks are considered (optionally) in building the `satisfaction` field

### Wallet

- Changed `Wallet::{sign, finalize_psbt}` now take a `&mut psbt` rather than consuming it.
- Require and validate `non_witness_utxo` for SegWit signatures by default, can be adjusted with `SignOptions`
- Replace the opt-in builder option `force_non_witness_utxo` with the opposite `only_witness_utxo`. From now on we will provide the `non_witness_utxo`, unless explicitly asked not to.

## [v0.6.0] - [v0.5.1]

### Misc
#### Changed
- New minimum supported rust version is 1.46.0
- Changed `AnyBlockchainConfig` to use serde tagged representation.

### Descriptor
#### Added
- Added ability to analyze a `PSBT` to check which and how many signatures are already available

### Wallet
#### Changed
- `get_new_address()` refactored to `get_address(AddressIndex::New)` to support different `get_address()` index selection strategies

#### Added
- Added `get_address(AddressIndex::LastUnused)` which returns the last derived address if it has not been used or if used in a received transaction returns a new address
- Added `get_address(AddressIndex::Peek(u32))` which returns a derived address for a specified descriptor index but does not change the current index
- Added `get_address(AddressIndex::Reset(u32))` which returns a derived address for a specified descriptor index and resets current index to the given value
- Added `get_psbt_input` to create the corresponding psbt input for a local utxo.

#### Fixed
- Fixed `coin_select` calculation for UTXOs where `value < fee` that caused over-/underflow errors.

## [v0.5.1] - [v0.5.0]

### Misc
#### Changed
- Pin `hyper` to `=0.14.4` to make it compile on Rust 1.45

## [v0.5.0] - [v0.4.0]

### Misc
#### Changed
- Updated `electrum-client` to version `0.7`

### Wallet
#### Changed
- `FeeRate` constructors `from_sat_per_vb` and `default_min_relay_fee` are now `const` functions

## [v0.4.0] - [v0.3.0]

### Keys
#### Changed
- Renamed `DerivableKey::add_metadata()` to `DerivableKey::into_descriptor_key()`
- Renamed `ToDescriptorKey::to_descriptor_key()` to `IntoDescriptorKey::into_descriptor_key()`
#### Added
- Added an `ExtendedKey` type that is an enum of `bip32::ExtendedPubKey` and `bip32::ExtendedPrivKey`
- Added `DerivableKey::into_extended_key()` as the only method that needs to be implemented

### Misc
#### Removed
- Removed the `parse_descriptor` example, since it wasn't demonstrating any bdk-specific API anymore.
#### Changed
- Updated `bitcoin` to `0.26`, `miniscript` to `5.1` and `electrum-client` to `0.6`
#### Added
- Added support for the `signet` network (issue #62)
- Added a function to get the version of BDK at runtime

### Wallet
#### Changed
- Removed the explicit `id` argument from `Wallet::add_signer()` since that's now part of `Signer` itself
- Renamed `ToWalletDescriptor::to_wallet_descriptor()` to `IntoWalletDescriptor::into_wallet_descriptor()`

### Policy
#### Changed
- Removed unneeded `Result<(), PolicyError>` return type for `Satisfaction::finalize()`
- Removed the `TooManyItemsSelected` policy error (see commit message for more details)

## [v0.3.0] - [v0.2.0]

### Descriptor
#### Changed
- Added an alias `DescriptorError` for `descriptor::error::Error`
- Changed the error returned by `descriptor!()` and `fragment!()` to `DescriptorError`
- Changed the error type in `ToWalletDescriptor` to `DescriptorError`
- Improved checks on descriptors built using the macros

### Blockchain
#### Changed
- Remove `BlockchainMarker`, `OfflineClient` and `OfflineWallet` in favor of just using the unit
  type to mark for a missing client.
- Upgrade `tokio` to `1.0`.

### Transaction Creation Overhaul

The `TxBuilder` is now created from the `build_tx` or `build_fee_bump` functions on wallet and the
final transaction is created by calling `finish` on the builder.

- Removed `TxBuilder::utxos` in favor of `TxBuilder::add_utxos`
- Added `Wallet::build_tx` to replace `Wallet::create_tx`
- Added `Wallet::build_fee_bump` to replace `Wallet::bump_fee`
- Added `Wallet::get_utxo`
- Added `Wallet::get_descriptor_for_keychain`

### `add_foreign_utxo`

- Renamed `UTXO` to `LocalUtxo`
- Added `WeightedUtxo` to replace floating `(UTXO, usize)`.
- Added `Utxo` enum to incorporate both local utxos and foreign utxos
- Added `TxBuilder::add_foreign_utxo` which allows adding a utxo external to the wallet.

### CLI
#### Changed
- Remove `cli.rs` module, `cli-utils` feature and `repl.rs` example; moved to new [`bdk-cli`](https://github.com/bitcoindevkit/bdk-cli) repository

## [v0.2.0] - [0.1.0-beta.1]

### Project
#### Added
- Add CONTRIBUTING.md
- Add a Discord badge to the README
- Add code coverage github actions workflow
- Add scheduled audit check in CI
- Add CHANGELOG.md

#### Changed
- Rename the library to `bdk`
- Rename `ScriptType` to `KeychainKind`
- Prettify README examples on github
- Change CI to github actions
- Bump rust-bitcoin to 0.25, fix Cargo dependencies
- Enable clippy for stable and tests by default
- Switch to "mainline" rust-miniscript
- Generate a different cache key for every CI job
- Fix to at least bitcoin ^0.25.2

#### Fixed
- Fix or ignore clippy warnings for all optional features except compact_filters
- Pin cc version because last breaks rocksdb build

### Blockchain
#### Added
- Add a trait to create `Blockchain`s from a configuration
- Add an `AnyBlockchain` enum to allow switching at runtime
- Document `AnyBlockchain` and `ConfigurableBlockchain`
- Use our Instant struct to be compatible with wasm
- Make esplora call in parallel
- Allow to set concurrency in Esplora config and optionally pass it in repl

#### Fixed
- Fix receiving a coinbase using Electrum/Esplora
- Use proper type for EsploraHeader, make conversion to BlockHeader infallible
- Eagerly unwrap height option, save one collect

#### Changed
- Simplify the architecture of blockchain traits
- Improve sync
- Remove unused varaint HeaderParseFail

### CLI
#### Added
- Conditionally remove cli args according to enabled feature

#### Changed
- Add max_addresses param in sync
- Split the internal and external policy paths

### Database
#### Added
- Add `AnyDatabase` and `ConfigurableDatabase` traits

### Descriptor
#### Added
- Add a macro to write descriptors from code
- Add descriptor templates, add `DerivableKey`
- Add ToWalletDescriptor trait tests
- Add support for `sortedmulti` in `descriptor!`
- Add ExtractPolicy trait tests
- Add get_checksum tests, cleanup tests
- Add descriptor macro tests

#### Changes
- Improve the descriptor macro, add traits for key and descriptor types

#### Fixes
- Fix the recovery of a descriptor given a PSBT

### Keys
#### Added
- Add BIP39 support
- Take `ScriptContext` into account when converting keys
- Add a way to restrict the networks in which keys are valid
- Add a trait for keys that can be generated
- Fix entropy generation
- Less convoluted entropy generation
- Re-export tiny-bip39
- Implement `GeneratableKey` trait for `bitcoin::PrivateKey`
- Implement `ToDescriptorKey` trait for `GeneratedKey`
- Add a shortcut to generate keys with the default options

#### Fixed
- Fix all-keys and cli-utils tests

### Wallet
#### Added
- Allow to define static fees for transactions Fixes #137
- Merging two match expressions for fee calculation
- Incorporate RBF rules into utxo selection function
- Add Branch and Bound coin selection
- Add tests for BranchAndBoundCoinSelection::coin_select
- Add tests for BranchAndBoundCoinSelection::bnb
- Add tests for BranchAndBoundCoinSelection::single_random_draw
- Add test that shwpkh populates witness_utxo
- Add witness and redeem scripts to PSBT outputs
- Add an option to include `PSBT_GLOBAL_XPUB`s in PSBTs
- Eagerly finalize inputs

#### Changed
- Use collect to avoid iter unwrapping Options
- Make coin_select take may/must use utxo lists
- Improve `CoinSelectionAlgorithm`
- Refactor `Wallet::bump_fee()`
- Default to SIGHASH_ALL if not specified
- Replace ChangeSpendPolicy::filter_utxos with a predicate
- Make 'unspendable' into a HashSet
- Stop implicitly enforcing manaul selection by .add_utxo
- Rename DumbCS to LargestFirstCoinSelection
- Rename must_use_utxos to required_utxos
- Rename may_use_utxos to optional_uxtos
- Rename get_must_may_use_utxos to preselect_utxos
- Remove redundant Box around address validators
- Remove redundant Box around signers
- Make Signer and AddressValidator Send and Sync
- Split `send_all` into `set_single_recipient` and `drain_wallet`
- Use TXIN_DEFAULT_WEIGHT constant in coin selection
- Replace `must_use` with `required` in coin selection
- Take both spending policies into account in create_tx
- Check last derivation in cache to avoid recomputation
- Use the branch-and-bound cs by default
- Make coin_select return UTXOs instead of TxIns
- Build output lookup inside complete transaction
- Don't wrap SignersContainer arguments in Arc
- More consistent references with 'signers' variables

#### Fixed
- Fix signing for `ShWpkh` inputs
- Fix the recovery of a descriptor given a PSBT

### Examples
#### Added
- Support esplora blockchain source in repl

#### Changed
- Revert back the REPL example to use Electrum
- Remove the `magic` alias for `repl`
- Require esplora feature for repl example

#### Security
- Use dirs-next instead of dirs since the latter is unmantained

## [0.1.0-beta.1] - 2020-09-08

### Blockchain
#### Added
- Lightweight Electrum client with SSL/SOCKS5 support
- Add a generalized "Blockchain" interface
- Add Error::OfflineClient
- Add the Esplora backend
- Use async I/O in the various blockchain impls
- Compact Filters blockchain implementation
- Add support for Tor
- Impl OnlineBlockchain for types wrapped in Arc

### Database
#### Added
- Add a generalized database trait and a Sled-based implementation
- Add an in-memory database

### Descriptor
#### Added
- Wrap Miniscript descriptors to support xpubs
- Policy and contribution
- Transform a descriptor into its "public" version
- Use `miniscript::DescriptorPublicKey`

### Macros
#### Added
- Add a feature to enable the async interface on non-wasm32 platforms

### Wallet
#### Added
- Wallet logic
- Add `assume_height_reached` in PSBTSatisfier
- Add an option to change the assumed current height
- Specify the policy branch with a map
- Add a few commands to handle psbts
- Add hd_keypaths to outputs
- Add a `TxBuilder` struct to simplify `create_tx()`'s interface
- Abstract coin selection in a separate trait
- Refill the address pool whenever necessary
- Implement the wallet import/export format from FullyNoded
- Add a type convert fee units, add `Wallet::estimate_fee()`
- TxOrdering, shuffle/bip69 support
- Add RBF and custom versions in TxBuilder
- Allow limiting the use of internal utxos in TxBuilder
- Add `force_non_witness_utxo()` to TxBuilder
- RBF and add a few tests
- Add AddressValidators
- Add explicit ordering for the signers
- Support signing the whole tx instead of individual inputs
- Create a PSBT signer from an ExtendedDescriptor

### Examples
#### Added
- Add REPL broadcast command
- Add a miniscript compiler CLI
- Expose list_transactions() in the REPL
- Use `MemoryDatabase` in the compiler example
- Make the REPL return JSON

[unreleased]: https://github.com/bitcoindevkit/bdk/compare/v0.9.0...HEAD
[0.1.0-beta.1]: https://github.com/bitcoindevkit/bdk/compare/96c87ea5...0.1.0-beta.1
[v0.2.0]: https://github.com/bitcoindevkit/bdk/compare/0.1.0-beta.1...v0.2.0
[v0.3.0]: https://github.com/bitcoindevkit/bdk/compare/v0.2.0...v0.3.0
[v0.4.0]: https://github.com/bitcoindevkit/bdk/compare/v0.3.0...v0.4.0
[v0.5.0]: https://github.com/bitcoindevkit/bdk/compare/v0.4.0...v0.5.0
[v0.5.1]: https://github.com/bitcoindevkit/bdk/compare/v0.5.0...v0.5.1
[v0.6.0]: https://github.com/bitcoindevkit/bdk/compare/v0.5.1...v0.6.0
[v0.7.0]: https://github.com/bitcoindevkit/bdk/compare/v0.6.0...v0.7.0
[v0.8.0]: https://github.com/bitcoindevkit/bdk/compare/v0.7.0...v0.8.0
[v0.9.0]: https://github.com/bitcoindevkit/bdk/compare/v0.8.0...v0.9.0
