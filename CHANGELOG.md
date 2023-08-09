# Changelog

All notable changes to this project can be found here and in each release's git tag and can be viewed with `git tag -ln100 "v*"`. See also [DEVELOPMENT_CYCLE.md](DEVELOPMENT_CYCLE.md) for more details.

Contributors do not need to change this file but do need to add changelog details in their PR descriptions. The person making the next release will collect changelog details from included PRs and edit this file prior to each release.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [v0.28.2]

### Summary

Reverts the 0.28.1 esplora-client version update from 0.5.0 back to 0.4.0.

## [v0.28.1]

### Summary

This patch release backports (from the BDK 1.0 dev branch) a fix for a bug in the policy condition calculation and adds a new taproot single key descriptor template (BIP-86). The policy condition calculation bug can cause issues when a policy subtree fails due to missing info even if it's not selected when creating a new transaction, errors on unused policy paths are now ignored.

### Fixed

- Backported #932 fix for policy condition calculation #1008

### Added

-  Backported #840 taproot descriptor template (BIP-86) #1033

## [v0.28.0]

### Summary

Disable default-features for rust-bitcoin and rust-miniscript dependencies, and for rust-esplora-client optional dependency. New default `std` feature must be enabled unless building for wasm.

### Changed

- Bump bip39 crate to v2.0.0 #875
- Set default-features = false for rust-bitcoin and rust-miniscript #882
- Update esplora client dependency to version 0.4 #884
- Added new `std` feature as part of default features #930

## [v0.27.1]

### Summary

Fixes [RUSTSEC-2022-0090], this issue is only applicable if you are using the optional sqlite database feature.

[RUSTSEC-2022-0090]: https://rustsec.org/advisories/RUSTSEC-2022-0090

### Changed

- Update optional sqlite dependency from 0.27.0 to 0.28.0. #867

## [v0.27.0]

### Summary

A maintenance release with a bump in project MSRV to 1.57.0, updated dependence and a few developer oriented improvements. Improvements include  better error formatting, don't default to async/await for wasm32 and adding derived PartialEq and Eq on SyncTime.

### Changed

- Improve display error formatting #814
- Don't default to use async/await on wasm32 #831
- Project MSRV changed from 1.56.1 to 1.57.0 #842
- Update rust-miniscript dependency to latest bug fix release 9.0 #844

### Added

- Derive PartialEq, Eq on SyncTime #837

## [v0.26.0]

### Summary

This release improves Fulcrum electrum server compatibility and fixes public descriptor template key origin paths. We also snuck in small enhancements to configure the electrum client to validate the domain using SSL and sort TransactionDetails by block height and timestamp.
  
### Fixed
  
- Make electrum blockchain client `save_tx` function order independent to work with Fulcrum servers. #808
- Fix wrong testnet key origin path in public descriptor templates. #818
- Make README.md code examples compile without errors. #820

### Changed

- Bump `hwi` dependency to `0.4.0`. #825
- Bump `esplora-client` dependency to `0.3` #830

### Added
  
- For electrum blockchain client, allow user to configure whether to validate the domain using SSL. #805
- Implement ordering for `TransactionDetails`. #812

## [v0.25.0]

### Summary

This release fixes slow sync time and big script_pubkeys table with SQLite, the wallet rescan height for the FullyNodedExport and setting the network for keys in the KeyMap when using descriptor templates. Also added are new blockchain and mnemonic examples.
  
### Fixed
  
- Slow sync time and big script_pubkeys table with SQLite.
- Wallet rescan height for the FullyNodedExport.
- Setting the network for keys in the KeyMap when using descriptor templates.
  
### Added
  
- Examples for connecting to Esplora, Electrum Server, Neutrino and Bitcoin Core.
- Example for using a mnemonic in a descriptors.

## [v0.24.0]

### Summary

This release contains important dependency updates for `rust-bitcoin` to `0.29` and `rust-miniscript` to `8.0`, plus related crates that also depend on the latest version of `rust-bitcoin`. The release also includes a breaking change to the BDK signer which now produces low-R signatures by default, saving one byte. A bug was found in the `get_checksum` and `get_checksum_bytes` functions, which are now deprecated in favor of fixed versions called `calc_checksum` and `calc_checksum_bytes`. And finally a new `hardware-signer` features was added that re-exports the `hwi` crate, along with a new `hardware_signers.rs` example file.
   
### Changed

- Updated dependency versions for `rust-bitcoin` to `0.29` and `rust-miniscript` to `8.0`, plus all related crates. @afilini #770
- BDK Signer now produces low-R signatures by default, saving one byte. If you want to preserve the original behavior, set allow_grinding in the SignOptions to false. @vladimirfomene #779
- Deprecated `get_checksum`and `get_checksum_bytes` due to bug where they calculates the checksum of a descriptor that already has a checksum.  Use `calc_checksum` and `calc_checksum_bytes` instead. @evanlinjin #765
- Remove deprecated "address validators". @afilini #770
  
### Added

- New `calc_checksum` and `calc_checksum_bytes`, replace deprecated `get_checksum` and `get_checksum_bytes`. @evanlinjin #765
- Re-export the hwi crate when the feature hardware-signer is on.  @danielabrozzoni #758
- New examples/hardware_signer.rs. @danielabrozzoni #758
- Make psbt module public to expose PsbtUtils trait to downstream projects. @notmandatory #782

## [v0.23.0]

### Summary

This release brings new utilities functions on PSBTs like `fee_amount()` and `fee_rate()` and migrates BDK to use our new external esplora client library.
As always many bug fixes, docs and tests improvement are also included.

### Changed

- Update electrum-client to 0.11.0 by @afilini in https://github.com/bitcoindevkit/bdk/pull/737
- Change configs for source-base code coverage by @wszdexdrf in https://github.com/bitcoindevkit/bdk/pull/708
- Improve docs regarding PSBT finalization by @tnull in https://github.com/bitcoindevkit/bdk/pull/753
- Update compiler example to a Policy example by @rajarshimaitra in https://github.com/bitcoindevkit/bdk/pull/730
- Fix the release process by @afilini in https://github.com/bitcoindevkit/bdk/pull/754
- Remove redundant duplicated keys check by @afilini in https://github.com/bitcoindevkit/bdk/pull/761
- Remove genesis_block lazy initialization by @shobitb in https://github.com/bitcoindevkit/bdk/pull/756
- Fix `Wallet::descriptor_checksum` to actually return the checksum by @evanlinjin in https://github.com/bitcoindevkit/bdk/pull/763
- Use the esplora client crate by @afilini in https://github.com/bitcoindevkit/bdk/pull/764

### Added

- Run code coverage on every PR by @danielabrozzoni in https://github.com/bitcoindevkit/bdk/pull/747
- Add psbt_signer.rs example by @notmandatory in https://github.com/bitcoindevkit/bdk/pull/744
- Add fee_amount() and fee_rate() functions to PsbtUtils trait by @notmandatory in https://github.com/bitcoindevkit/bdk/pull/728
- Add tests to improve coverage by @vladimirfomene in https://github.com/bitcoindevkit/bdk/pull/745
- Enable signing taproot transactions with only `non_witness_utxos` by @afilini in https://github.com/bitcoindevkit/bdk/pull/757
- Add datatype for is_spent sqlite column by @vladimirfomene in https://github.com/bitcoindevkit/bdk/pull/713
- Add vscode filter to gitignore by @evanlinjin in https://github.com/bitcoindevkit/bdk/pull/762

## [v0.22.0]

### Summary

This release brings support for hardware signers on desktop through the HWI library.
It also includes fixes and improvements which are part of our ongoing effort of integrating
BDK and LDK together.

### Changed

- FeeRate function name as_sat_vb to as_sat_per_vb. #678
- Verify signatures after signing. #718
- Dependency electrum-client to 0.11.0. #737

### Added
  
- Functions to create FeeRate from sats/kvbytes and sats/kwu. #678
- Custom hardware wallet signer HwiSigner in wallet::hardwaresigner module. #682
- Function allow_dust on TxBuilder. #689
- Implementation of Deref<Target=UrlClient> for EsploraBlockchain. #722
- Implementation of Deref<Target=Client> for ElectrumBlockchain #705
- Implementation of Deref<Target=Client> for RpcBlockchain. #731

## [v0.21.0]

- Add `descriptor::checksum::get_checksum_bytes` method.
- Add `Excess` enum to handle remaining amount after coin selection.
- Move change creation from `Wallet::create_tx` to `CoinSelectionAlgorithm::coin_select`.
- Change the interface of `SqliteDatabase::new` to accept any type that implement AsRef<Path>
- Add the ability to specify which leaves to sign in a taproot transaction through `TapLeavesOptions` in `SignOptions`
- Add the ability to specify whether a taproot transaction should be signed using the internal key or not, using `sign_with_tap_internal_key` in `SignOptions`
- Consolidate params `fee_amount` and `amount_needed` in `target_amount` in `CoinSelectionAlgorithm::coin_select` signature.
- Change the meaning of the `fee_amount` field inside `CoinSelectionResult`: from now on the `fee_amount` will represent only the fees asociated with the utxos in the `selected` field of `CoinSelectionResult`.
- New `RpcBlockchain` implementation with various fixes.
- Return balance in separate categories, namely `confirmed`, `trusted_pending`, `untrusted_pending` & `immature`.

## [v0.20.0]

- New MSRV set to `1.56.1`
- Fee sniping discouraging through nLockTime - if the user specifies a `current_height`, we use that as a nlocktime, otherwise we use the last sync height (or 0 if we never synced)
- Fix hang when `ElectrumBlockchainConfig::stop_gap` is zero.
- Set coin type in BIP44, BIP49, and BIP84 templates
- Get block hash given a block height - A `get_block_hash` method is now defined on the `GetBlockHash` trait and implemented on every blockchain backend. This method expects a block height and returns the corresponding block hash.
- Add `remove_partial_sigs` and `try_finalize` to `SignOptions`
- Deprecate `AddressValidator`
- Fix Electrum wallet sync potentially causing address index decrement - compare proposed index and current index before applying batch operations during sync.

## [v0.19.0]

- added `OldestFirstCoinSelection` impl to `CoinSelectionAlgorithm`
- New MSRV set to `1.56`
- Unpinned tokio to `1`
- Add traits to reuse `Blockchain`s across multiple wallets (`BlockchainFactory` and `StatelessBlockchain`).
- Upgrade to rust-bitcoin `0.28`
- If using the `sqlite-db` feature all cached wallet data is deleted due to a possible UTXO inconsistency, a wallet.sync will recreate it
- Update `PkOrF` in the policy module to become an enum
- Add experimental support for Taproot, including:
  - Support for `tr()` descriptors with complex tapscript trees
  - Creation of Taproot PSBTs (BIP-371)
  - Signing Taproot PSBTs (key spend and script spend)
  - Support for `tr()` descriptors in the `descriptor!()` macro
- Add support for Bitcoin Core 23.0 when using the `rpc` blockchain

## [v0.18.0]

- Add `sqlite-bundled` feature for deployments that need a bundled version of sqlite, i.e. for mobile platforms.
- Added `Wallet::get_signers()`, `Wallet::descriptor_checksum()` and `Wallet::get_address_validators()`, exposed the `AsDerived` trait.
- Deprecate `database::Database::flush()`, the function is only needed for the sled database on mobile, instead for mobile use the sqlite database.
- Add `keychain: KeychainKind` to `wallet::AddressInfo`.
- Improve key generation traits
- Rename `WalletExport` to `FullyNodedExport`, deprecate the former.
- Bump `miniscript` dependency version to `^6.1`.

## [v0.17.0]

- Removed default verification from `wallet::sync`. sync-time verification is added in `script_sync` and is activated by `verify` feature flag.
- `verify` flag removed from `TransactionDetails`.
- Add `get_internal_address` to allow you to get internal addresses just as you get external addresses.
- added `ensure_addresses_cached` to `Wallet` to let offline wallets load and cache addresses in their database
- Add `is_spent` field to `LocalUtxo`; when we notice that a utxo has been spent we set `is_spent` field to true instead of deleting it from the db.

### Sync API change

To decouple the `Wallet` from the `Blockchain` we've made major changes:

- Removed `Blockchain` from Wallet.
- Removed `Wallet::broadcast` (just use `Blockchain::broadcast`)
- Deprecated `Wallet::new_offline` (all wallets are offline now)
- Changed `Wallet::sync` to take a `Blockchain`.
- Stop making a request for the block height when calling `Wallet:new`.
- Added `SyncOptions` to capture extra (future) arguments to `Wallet::sync`.
- Removed `max_addresses` sync parameter which determined how many addresses to cache before syncing since this can just be done with `ensure_addresses_cached`.
- remove `flush` method from the `Database` trait.

## [v0.16.1]

- Pin tokio dependency version to ~1.14 to prevent errors due to their new MSRV 1.49.0

## [v0.16.0]

- Disable `reqwest` default features.
- Added `reqwest-default-tls` feature: Use this to restore the TLS defaults of reqwest if you don't want to add a dependency to it in your own manifest.
- Use dust_value from rust-bitcoin
- Fixed generating WIF in the correct network format.

## [v0.15.0]

- Overhauled sync logic for electrum and esplora.
- Unify ureq and reqwest esplora backends to have the same configuration parameters. This means reqwest now has a timeout parameter and ureq has a concurrency parameter.
- Fixed esplora fee estimation.

## [v0.14.0]

- BIP39 implementation dependency, in `keys::bip39` changed from tiny-bip39 to rust-bip39.
- Add new method on the `TxBuilder` to embed data in the transaction via `OP_RETURN`. To allow that a fix to check the dust only on spendable output has been introduced.
- Update the `Database` trait to store the last sync timestamp and block height
- Rename `ConfirmationTime` to `BlockTime`

## [v0.13.0]

- Exposed `get_tx()` method from `Database` to `Wallet`.

## [v0.12.0]

- Activate `miniscript/use-serde` feature to allow consumers of the library to access it via the re-exported `miniscript` crate.
- Add support for proxies in `EsploraBlockchain`
- Added `SqliteDatabase` that implements `Database` backed by a sqlite database using `rusqlite` crate.

## [v0.11.0]

- Added `flush` method to the `Database` trait to explicitly flush to disk latest changes on the db.

## [v0.10.0]

- Added `RpcBlockchain` in the `AnyBlockchain` struct to allow using Rpc backend where `AnyBlockchain` is used (eg `bdk-cli`)
- Removed hard dependency on `tokio`.

### Wallet

- Removed and replaced `set_single_recipient` with more general `drain_to` and replaced `maintain_single_recipient` with `allow_shrinking`.

### Blockchain

- Removed `stop_gap` from `Blockchain` trait and added it to only `ElectrumBlockchain` and `EsploraBlockchain` structs.
- Added a `ureq` backend for use when not using feature `async-interface` or target WASM. `ureq` is a blocking HTTP client.

## [v0.9.0]

### Wallet

- Added Bitcoin core RPC added as blockchain backend
- Added a `verify` feature that can be enable to verify the unconfirmed txs we download against the consensus rules

## [v0.8.0]

### Wallet
- Added an option that must be explicitly enabled to allow signing using non-`SIGHASH_ALL` sighashes (#350)
#### Changed
`get_address` now returns an `AddressInfo` struct that includes the index and derefs to `Address`.

## [v0.7.0]

### Policy
#### Changed
Removed `fill_satisfaction` method in favor of enum parameter in `extract_policy` method

#### Added
Timelocks are considered (optionally) in building the `satisfaction` field

### Wallet

- Changed `Wallet::{sign, finalize_psbt}` now take a `&mut psbt` rather than consuming it.
- Require and validate `non_witness_utxo` for SegWit signatures by default, can be adjusted with `SignOptions`
- Replace the opt-in builder option `force_non_witness_utxo` with the opposite `only_witness_utxo`. From now on we will provide the `non_witness_utxo`, unless explicitly asked not to.

## [v0.6.0]

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

## [v0.5.1]

### Misc
#### Changed
- Pin `hyper` to `=0.14.4` to make it compile on Rust 1.45

## [v0.5.0]

### Misc
#### Changed
- Updated `electrum-client` to version `0.7`

### Wallet
#### Changed
- `FeeRate` constructors `from_sat_per_vb` and `default_min_relay_fee` are now `const` functions

## [v0.4.0]

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

## [v0.3.0]

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

## [v0.2.0]

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
[v0.10.0]: https://github.com/bitcoindevkit/bdk/compare/v0.9.0...v0.10.0
[v0.11.0]: https://github.com/bitcoindevkit/bdk/compare/v0.10.0...v0.11.0
[v0.12.0]: https://github.com/bitcoindevkit/bdk/compare/v0.11.0...v0.12.0
[v0.13.0]: https://github.com/bitcoindevkit/bdk/compare/v0.12.0...v0.13.0
[v0.14.0]: https://github.com/bitcoindevkit/bdk/compare/v0.13.0...v0.14.0
[v0.15.0]: https://github.com/bitcoindevkit/bdk/compare/v0.14.0...v0.15.0
[v0.16.0]: https://github.com/bitcoindevkit/bdk/compare/v0.15.0...v0.16.0
[v0.16.1]: https://github.com/bitcoindevkit/bdk/compare/v0.16.0...v0.16.1
[v0.17.0]: https://github.com/bitcoindevkit/bdk/compare/v0.16.1...v0.17.0
[v0.18.0]: https://github.com/bitcoindevkit/bdk/compare/v0.17.0...v0.18.0
[v0.19.0]: https://github.com/bitcoindevkit/bdk/compare/v0.18.0...v0.19.0
[v0.20.0]: https://github.com/bitcoindevkit/bdk/compare/v0.19.0...v0.20.0
[v0.21.0]: https://github.com/bitcoindevkit/bdk/compare/v0.20.0...v0.21.0
[v0.22.0]: https://github.com/bitcoindevkit/bdk/compare/v0.21.0...v0.22.0
[v0.23.0]: https://github.com/bitcoindevkit/bdk/compare/v0.22.0...v0.23.0
[v0.24.0]: https://github.com/bitcoindevkit/bdk/compare/v0.23.0...v0.24.0
[v0.25.0]: https://github.com/bitcoindevkit/bdk/compare/v0.24.0...v0.25.0
[v0.26.0]: https://github.com/bitcoindevkit/bdk/compare/v0.25.0...v0.26.0
[v0.27.0]: https://github.com/bitcoindevkit/bdk/compare/v0.26.0...v0.27.0
[v0.27.1]: https://github.com/bitcoindevkit/bdk/compare/v0.27.0...v0.27.1
[v0.28.0]: https://github.com/bitcoindevkit/bdk/compare/v0.27.1...v0.28.0
[v0.28.1]: https://github.com/bitcoindevkit/bdk/compare/v0.28.0...v0.28.1
[v0.28.2]: https://github.com/bitcoindevkit/bdk/compare/v0.28.1...v0.28.2
[Unreleased]: https://github.com/bitcoindevkit/bdk/compare/v0.28.2...HEAD
