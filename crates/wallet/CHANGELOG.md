# Changelog

All notable changes to this project can be found here and in each release's git tag and can be viewed with `git tag -ln100 "v*"`. See also [DEVELOPMENT_CYCLE.md](../../DEVELOPMENT_CYCLE.md) for more details.

Contributors do not need to change this file but do need to add changelog details in their PR descriptions. The person making the next release will collect changelog details from included PRs and edit this file prior to each release.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [wallet-1.2.0]

### Changed

- Accept any type that is convertible to a `ScriptBuf` in `TxBuilder::add_recipient` #1841
- Refactor/use iterators to preselect utxos #1798
- Bump bitcoin dependency to v0.32.4 #1853
- Pin bdk_chain version to latest release #1860
- chore: bump `miniscript` to `12.3.1` #1924

### Fixed

- Fix off-by-one error checking coinbase maturity in optional UTxOs #1830
- Fix PersistedWallet to be Send + Sync, even when used with a !Sync persister type such as rusqlite::Connection. #1874


## [wallet-1.1.0]

### Added

- `Wallet` can now be constructed using `Network::Testnet4` #1805
- test: create tx locktime cltv for a specific time #1682
- docs: add architectural decision records (ADR) #1592

### Changed

- Changed the default transaction version number to version 2 for TxBuilder. #1789

### Fixed

- Improve safety on finalize psbt #1790
- Fixed an issue preventing `build_fee_bump` from re-targeting the drain value for some wallets #1812

## [wallet-1.0.0]

### Summary

This is the final bdk_wallet 1.0.0 release. It contains small improvements to the wallet `transactions` function and 
`next_unused_address` API docs. Please thank all the [contributors] who made this first major release possible and
who's continued effort make the BDK project so awesome! 

[contributors]: https://github.com/bitcoindevkit/bdk/graphs/contributors

### Changed

- `Wallet::transactions` should only return relevant transactions. #1779
- Minor updates to fix new rustc 1.83.0 clippy warnings. #1776

### Documentation

- Reword the `next_unused_address` API docs. #1680

## [v1.0.0-beta.6]

### Summary

This is the final "beta" test release before a final `bdk_wallet` 1.0.0 version. Changes include small bug fixes and API improvements plus an improved algorithm for determining which transactions are in the current best "canonical" block chain. The new canonicalization algorithm processes the transaction graph in linear time versus the prior quadratic time algorithm.

### Changed

- Move transaction testing utilities from `crates/chain/tests/common` to `testenv` crate #1612
- Remove bdk_chain::ConfirmationTime. Use ChainPosition<ConfirmationBlockTime> in its place. #1643
- Fix building change signers in `load_with_params` #1662
- Remove bdk_chain method KeychainTxOutIndex::inner #1652
- Document bdk_file_store is a development/testing database #1661
- Fix incorrect links in docs to wallet examples #1668
- Add bdk_wallet "test-utils" feature flag that exposes common helpers for testing and development #1658
- Removed methods Wallet::insert_tx, Wallet::insert_checkpoint, Wallet::unbroadcast_transactions #1658
- Fix type constraint on list canonical tx #1724
- Fix testenv docs.rs docs  #1722
- Use `bitcoin::constants::COINBASE_MATURITY` #1727
- Rename bdk_core::spk_client's SyncResult to SyncResponse #1732
- Fix core checkpoint Drop stack overflow #1731
- Change Utxo::Foreign::sequence type to not be optional #1681
- Check time when persisting in `rusqlite` impl #1730
- Bump hashbrown dependency version to v0.14.5 #1721
- Add usage of debug_assert!() to LocalChain::apply_update #1734
- Allow Sqlite to persist anchor without tx #1736
- Change ChainPosition to represent transitive anchors and unconfirmed-without-last-seen values #1733
- Updated electrum-client dependency to 0.22.0 #1751
- Change TxBuilder to be Send safe and not implement the Clone trait #1737
- Update esplora-client dependency to 0.11.0 #1746
- Fix fetch_prev_txout to no longer queries coinbase transactions #1756
- Remove serde json dependency from chain crate #1752
- Introduce `O(n)` canonicalization algorithm #1670
- Add chain O(n) canonicalization algorithm see: /crates/chain/src/canonical_iter.rs #1670
- Add chain indexing fields in TxGraph; txs_by_anchor_height and txs_by_last_seen #1670
- Removed chain TxGraph methods: try_get_chain_position and get_chain_position #1670
- Change coin_selection and DescriptorExt::dust_value to use Amount type #1763

## [v1.0.0-beta.5]

### Summary

This release changes bdk_wallet transaction creation to enable RBF by default, it also updates the bdk_esplora client to retry server requests that fail due to rate limiting. The bdk_electrum crate now also offers a use-openssl feature.

### Changed

- ci: automated update to rustc 1.81.0 by @create-pr-actions in https://github.com/bitcoindevkit/bdk/pull/1613
- doc(wallet): Add docs to explain the lookahead by @ValuedMammal in https://github.com/bitcoindevkit/bdk/pull/1596
- chore: fix RUSTSEC-2024-0370 proc-macro-error is unmaintained by @oleonardolima in https://github.com/bitcoindevkit/bdk/pull/1603
- fix(bdk_esplora, bdk_electrum): build and test with `--no-default-features` by @oleonardolima in https://github.com/bitcoindevkit/bdk/pull/1615
- fix!(wallet): `ChangeSet` should not be non-exhaustive by @evanlinjin in https://github.com/bitcoindevkit/bdk/pull/1623
- feat(wallet)!: enable RBF by default on TxBuilder by @luisschwab in https://github.com/bitcoindevkit/bdk/pull/1616
- deps(esplora): bump esplora-client to 0.10.0 by @ValuedMammal in https://github.com/bitcoindevkit/bdk/pull/1626
- feat(chain,core)!: move `Merge` to `bdk_core` by @evanlinjin in https://github.com/bitcoindevkit/bdk/pull/1625
- Replace trait `AnchorFromBlockPosition` with new struct by @jirijakes in https://github.com/bitcoindevkit/bdk/pull/1594
- ci: fix build-test job with --no-default-features, add miniscript/no-std by @notmandatory in https://github.com/bitcoindevkit/bdk/pull/1636
- feat(bdk_electrum): add `use-openssl` as a feature by @oleonardolima in https://github.com/bitcoindevkit/bdk/pull/1620
- Bump bdk_wallet version to 1.0.0-beta.5 by @ValuedMammal in https://github.com/bitcoindevkit/bdk/pull/1639

## [v1.0.0-beta.4]

### Summary

BDK Wallet 1.0.0-beta.4 replaces 1.0.0-beta.3 and fixes a versioning mistake with two of our dependency crates. The bdk-wallet 1.0.0-beta.3 version and related dependency patch releases have been yanked from crates.io.

### Changed

See [v1.0.0-beta.3]

## [v1.0.0-beta.3] **YANKED**

RELEASE YANKED FROM CRATES.IO

### Summary

BDK Wallet 1.0.0-beta.3 is out! 🚀 Fixed transaction creation to not skip unused addresses, added function for sorting wallet transactions and option to change default BNB fallback coin selection. We moved the bdk_hwi crate functionality to the rust-hwi repo.

NOTE: The `bdk_wallet` BETA releases are meant for early user testing to find bugs, get feedback on APIs and identify any missing functionality. A final `bdk_wallet` 1.0.0 release will be available once known bugs are fixed, and tutorial docs and language bindings projects are updated.

### Changed
 
- chore: add `print_stdout`/`print_stderr` lints to workspace level by @oleonardolima in https://github.com/bitcoindevkit/bdk/pull/1425
- ci: add token for cron-update-rust.yml by @notmandatory in https://github.com/bitcoindevkit/bdk/pull/1580
- feat(core): add `TxUpdate::map_anchors` by @evanlinjin in https://github.com/bitcoindevkit/bdk/pull/1587
- ci: pin `tokio-util` dependency version to build with rust 1.63 by @LagginTimes in https://github.com/bitcoindevkit/bdk/pull/1590
- feat(wallet): add transactions_sort_by function by @notmandatory in https://github.com/bitcoindevkit/bdk/pull/1477
- docs: update CONTRIBUTING.md by @ValuedMammal in https://github.com/bitcoindevkit/bdk/pull/1584
- fix(wallet): only mark change address used if `create_tx` succeeds by @ValuedMammal in https://github.com/bitcoindevkit/bdk/pull/1579
- refactor(wallet): use `Amount` everywhere by @ValuedMammal in https://github.com/bitcoindevkit/bdk/pull/1595
- Change methods of `IndexedTxGraph`/`TxGraph`/`Wallet` that insert txs to be more generic by @evanlinjin in https://github.com/bitcoindevkit/bdk/pull/1586
- fix: typos by @storopoli in https://github.com/bitcoindevkit/bdk/pull/1599
- fix(wallet): do `check_wallet_descriptor` when creating and loading by @ValuedMammal in https://github.com/bitcoindevkit/bdk/pull/1597
- refactor(bdk_hwi): remove `bdk_hwi`, as `HWISigner`'s being moved to `rust-hwi` by @oleonardolima in https://github.com/bitcoindevkit/bdk/pull/1561
- Allow custom fallback algorithm for bnb by @evanlinjin in https://github.com/bitcoindevkit/bdk/pull/1581
- fix(core): calling `CheckPoint::insert` with existing block must succeed by @evanlinjin in https://github.com/bitcoindevkit/bdk/pull/1601
- fix(wallet): fix SingleRandomDraw to error if insufficient funds by @notmandatory in https://github.com/bitcoindevkit/bdk/pull/1605
- Bump bdk_wallet version to 1.0.0-beta.3 by @notmandatory in https://github.com/bitcoindevkit/bdk/pull/1608

## [v1.0.0-beta.2]

### Summary

The primary user facing changes are re-enabling single descriptor wallets and renaming LoadParams methods to be more explict. Wallet persistence was also simplified and blockchain clients no longer depend on bdk_chain.

### Changed

- ci: pin tokio to 1.38.1 to support MSRV 1.63 by @notmandatory in https://github.com/bitcoindevkit/bdk/pull/1524
- fix: typos by @storopoli in https://github.com/bitcoindevkit/bdk/pull/1529
- chore: fix clippy lints by @storopoli in https://github.com/bitcoindevkit/bdk/pull/1530
- bdk_wallet: Don't reimplement descriptor checksum, use the one from rust-miniscript by @darosior in https://github.com/bitcoindevkit/bdk/pull/1523
- Remove crate-renaming by @evanlinjin in https://github.com/bitcoindevkit/bdk/pull/1544
- example: Update `example_cli` and retire old nursery crates by @ValuedMammal in https://github.com/bitcoindevkit/bdk/pull/1495
- feat(testenv): Add method `new_with_config` by @ValuedMammal in https://github.com/bitcoindevkit/bdk/pull/1545
- test(electrum): Test sync in reorg and no-reorg situations by @LagginTimes in https://github.com/bitcoindevkit/bdk/pull/1535
- Add documentation for Electrum's full_scan/sync by @thunderbiscuit in https://github.com/bitcoindevkit/bdk/pull/1494
- Enable selecting use-rustls-ring feature on electrum client by @thunderbiscuit in https://github.com/bitcoindevkit/bdk/pull/1491
- [wallet] Enable support for single descriptor wallets by @ValuedMammal in https://github.com/bitcoindevkit/bdk/pull/1533
- Allow opting out of getting `LocalChain` updates with `FullScanRequest`/`SyncRequest` structures by @evanlinjin in https://github.com/bitcoindevkit/bdk/pull/1478
- Use explicit names for wallet builder methods that validate rather than set by @thunderbiscuit in https://github.com/bitcoindevkit/bdk/pull/1537
- fix(example_cli): add bitcoin and rand dependencies by @notmandatory in https://github.com/bitcoindevkit/bdk/pull/1549
- Simplify wallet persistence by @evanlinjin in https://github.com/bitcoindevkit/bdk/pull/1547
- Derive `Clone` on `AddressInfo` by @praveenperera in https://github.com/bitcoindevkit/bdk/pull/1560
- fix(wallet)!: make `LoadParams` implicitly satisfy `Send` by @evanlinjin in https://github.com/bitcoindevkit/bdk/pull/1562
- ci: add cron-update-rust.yml by @ValuedMammal in https://github.com/bitcoindevkit/bdk/pull/1564
- Introduce `tx_graph::Update` and simplify `TxGraph` update logic by @evanlinjin in https://github.com/bitcoindevkit/bdk/pull/1568
- Introduce `bdk_core` by @evanlinjin in https://github.com/bitcoindevkit/bdk/pull/1569
- Bump bdk version to 1.0.0-beta.2 by @notmandatory in https://github.com/bitcoindevkit/bdk/pull/1572
- chore: add missing bdk_core README.md and remove specific bdk_chain dev version by @notmandatory in https://github.com/bitcoindevkit/bdk/pull/1573

## [v1.0.0-beta.1]

### Summary

This release includes the first beta version of `bdk_wallet` with a stable 1.0.0 API. The changes in this version include reworked wallet persistence, changeset, and construction, optional user provided RNG, custom tx sorting, and use of merkle proofs in bdk_electrum.

### Changed

- fix(wallet)!: Simplify `SignOptions` and improve finalization logic by @ValuedMammal in https://github.com/bitcoindevkit/bdk/pull/1476
- feat(wallet): Allow user provided RNG, make rand an optional dependency by @rustaceanrob in https://github.com/bitcoindevkit/bdk/pull/1395
- refactor(wallet): Use Psbt::sighash_ecdsa for computing sighashes by @ValuedMammal in https://github.com/bitcoindevkit/bdk/pull/1424
- refactor(wallet)!: Use `Weight` type instead of `usize` by @oleonardolima in https://github.com/bitcoindevkit/bdk/pull/1468
- refactor(wallet): Remove usage of `blockdata::` from bitcoin paths by @tcharding in https://github.com/bitcoindevkit/bdk/pull/1490
- refactor(chain): calculate DescriptorId as the sha256 hash of spk at index 0 by @notmandatory in https://github.com/bitcoindevkit/bdk/pull/1486
- refactor(chain)!: Change tx_last_seen to `Option<u64>` by @ValuedMammal in https://github.com/bitcoindevkit/bdk/pull/1416
- refactor(wallet)!: Add support for custom sorting and deprecate BIP69 by @nymius in https://github.com/bitcoindevkit/bdk/pull/1487
- refactor(chain)!: Create module `indexer` by @ValuedMammal in https://github.com/bitcoindevkit/bdk/pull/1493
- chore(chain)!: Rename `Append` to `Merge` by @LagginTimes in https://github.com/bitcoindevkit/bdk/pull/1502
- refactor(wallet)!: Simplify public_descriptor(), remove redundant function by @gnapoli23 in https://github.com/bitcoindevkit/bdk/pull/1503
- ci: pin cc dependency version to build with rust 1.63 by @LagginTimes in https://github.com/bitcoindevkit/bdk/pull/1505
- feat(electrum)!: Update `bdk_electrum` to use merkle proofs by @LagginTimes in https://github.com/bitcoindevkit/bdk/pull/1489
- refactor(wallet)!: rework persistence, changeset, and construction  by @evanlinjin in https://github.com/bitcoindevkit/bdk/pull/1514
- refactor(chain)!: Update KeychainTxOutIndex methods to use owned K and ScriptBuf by @rustaceanrob in https://github.com/bitcoindevkit/bdk/pull/1506
- Bump bdk version to 1.0.0-beta.1 by @notmandatory in https://github.com/bitcoindevkit/bdk/pull/1522

## [v1.0.0-alpha.13]

### Summary

This release includes major changes required to finalize the bdk_wallet 1.0.0 APIs, including: upgrading to rust-bitcoin 0.32 and rust-miniscript 0.12.0, constructing a Wallet now requires two descriptors (external and internal), the db field was removed from Wallet, staged changes can be persisted via a blocking or async data store.

### Fixed

*  Fix KeychainTxOutIndex range-based methods. #1463

## [v1.0.0-alpha.12]

**NOTE: The wallet API library up to tag `1.0.0-alpha.11` uses the crate name `bdk`. The `1.0.0-alpha.12` tag and later use `bdk_wallet` as the crate name.**

### Summary

This bi-weekly release fixes an electrum syncing bug when calculating fees and adds the bdk_sqlite crate for storing wallet data in a SQLite tables. The Wallet::allow_shrinking function was also remove because it was too easy to misuse. Also the `bdk` crate was renamed to `bdk_wallet`. See the changelog for all the details.

### Fixed

- Fixed `calculate_fee` result when syncing with electrum.  #1443

### Changed

- Removed `TxBuilder::allow_shrinking()` function. #1386
- Renamed `bdk` crate to `bdk_wallet`. #1326
- Update Wallet to use `CombinedChangeSet` for persistence. #1128

### Added

- Add `CombinedChangeSet` in bdk_persist crate. #1128
- Add `bdk_sqlite` crate implementing SQLite based wallet data storage. #1128
- Update `bdk_wallet` export feature to support taproot descriptors. #1393

## [v1.0.0-alpha.11]

### Summary

This incremental bi-weekly release includes three big improvements. New electrum full_scan and sync APIs were added for more efficiently querying blockchain data. And the keychain::Changeset now includes public key descriptors and keychain::Balance uses bitcoin::Amount instead of u32 sats amounts. See the changelog for all the details.

### Changed

- Include the descriptor in keychain::Changeset #1203
- Update bdk_electrum crate to use sync/full-scan structs #1403
- Update keychain::Balance to use bitcoin::Amount #1411
- Change `bdk_testenv` to re-export internally used crates. #1414
- Updated documentation for `full_scan` and `sync` in `bdk_esplora`. #1427

## [v1.0.0-alpha.10]

### Summary

This incremental bi-weekly release improves the address API, simplifies the Esplora API, and introduced new structures for spk-based scanning that will enable easier syncing with electrum/esplora. It also introduces a new bdk-persist crate, removes the generic T from the Wallet, and makes KeychainTxOutIndex more range based.

### Fixed
- Enable blocking-https-rustls feature on esplora client https://github.com/bitcoindevkit/bdk/pull/1408

### Changed

- KeychainTxOutIndex methods modified to take ranges of keychains instead. https://github.com/bitcoindevkit/bdk/pull/1324
- Remove the generic from wallet https://github.com/bitcoindevkit/bdk/pull/1387
  - Changed PersistenceBackend errors to depend on the anyhow crate
  - Remove the generic T from Wallet
- Improve address API https://github.com/bitcoindevkit/bdk/pull/1402
  - Added Wallet methods:
    - peek_address
    - reveal_next_address
    - next_unused_address
    - reveal_addresses_to
    - list_unused_addresses
    - mark_used
    - unmark_used
  - Removed Wallet methods:
    - get_address
    - get_internal_address
    - try_get_address
    - try_get_internal_address
- Simplified EsploraExt API https://github.com/bitcoindevkit/bdk/pull/1380
  - Changed EsploraExt API so that sync only requires one round of fetching data. The local_chain_update method is removed and the local_tip parameter is added to the full_scan and sync methods.
  - Removed TxGraph::missing_heights and tx_graph::ChangeSet::missing_heights_from methods.
  - Introduced CheckPoint::insert which allows convenient checkpoint-insertion. This is intended for use by chain-sources when crafting an update.
  - Refactored merge_chains to also return the resultant CheckPoint tip.
  - Optimized the update LocalChain logic - use the update CheckPoint as the new CheckPoint tip when possible.
- Introduce `bdk-persist` crate https://github.com/bitcoindevkit/bdk/pull/1412
- Introduce universal sync/full-scan structures for spk-based syncing #1413
  - Add universal structures for initiating/receiving sync/full-scan requests/results for spk-based syncing.
  - Updated bdk_esplora chain-source to make use of new universal sync/full-scan structures.

## [v1.0.0-alpha.9]

### Summary

This regular bi-weekly alpha release updates dependencies rust-bitcoin to v0.31.0 and rust-miniscript to v11.0.0 plus replaces the deprecated rust-miniscript function max_satisfaction_weight with max_weight_to_satisfy. It also adds chain module improvements needed to simplify syncing with electrum and esplora blockchain clients.

### Fixed

* Replace the deprecated max_satisfaction_weight from rust-miniscript to max_weight_to_satisfy. #1345

### Changed

* Update dependencies: rust-bitcoin to v0.31.0 and rust-miniscript to v11.0.0. #1177
* Changed TxGraph to store transactions as Arc<Transaction>. This allows chain-sources to cheaply keep a copy of already-fetched transactions.  #1373
* Add get and range methods to CheckPoint #1369
  * Added get and range methods to CheckPoint (and in turn, LocalChain). This simulates an API where we have implemented a skip list of checkpoints (to implement in the future). This is a better API because we can query for any height or height range with just a checkpoint tip instead of relying on a separate checkpoint index (which needs to live in LocalChain).
  * Changed LocalChain to have a faster Eq implementation. We now maintain an xor value of all checkpoint block hashes. We compare this xor value to determine whether two chains are equal.
  * Added PartialEq implementation for CheckPoint and local_chain::Update.
* Methods into_tx_graph and into_confirmation_time_tx_graph for RelevantTxids are changed to no longer accept a seen_at parameter. #1385
  * Added method update_last_seen_unconfirmed for TxGraph.
* Added proptest for CheckPoint::range. #1397

## [v1.0.0-alpha.8]

### Summary

This incremental bi-weekly release migrates API to use the rust-bitcoin FeeRate type, fixes PSBT finalization to remove extra taproot fields, and fixes blockchain scanning stop_gap definition and documentation. We recommend all 1.0.0-alpha users upgrade to this release.

### Fixed

- Remove extra taproot fields when finalizing PSBT #1310
- Define and document stop_gap #1351

### Changed

- Migrate to bitcoin::FeeRate #1216
- chore: extract TestEnv into separate crate #1171

## [v1.0.0-alpha.7]

### Summary

This incremental bi-weekly release includes an API change to relax the generic requirements on the wallet transaction builder and a small fix when manually looking ahead to unrevealed scripts. Unless you need one of these changes there's no need to upgrade to this release.

### Fixed

- Fix `KeychainTxOutIndex::lookahead_to_target` to look ahead to correct index.  #1349

### Changed

- Relax the generic requirements on `TxBuilder`. #1312

## [v1.0.0-alpha.6]

### Summary

This small bi-weekly release fixes TxBuilder to support setting explicit nSequence for foreign inputs and a bug in tx_graph::ChangeSet::is_empty where is returns true even when it wasn't empty 🙈.  We also added a new option for Esplora APIs to include floating TxOuts in pudates for fee calculations.

### Fixed

- TxBuilder now supports setting explicit nSequence for foreign inputs. #1316
- Fix bug in tx_graph::ChangeSet::is_empty where is returns true even when it wasn't empty. #1335

### Added

- New Option for Esplora APIs to include floating TxOuts in updates for fee calculation. #1308

## [v1.0.0-alpha.5]

### Summary

This release introduces a block-by-block API to bdk::Wallet and adds a RPC wallet example, improves performance of bdk_file_store::EntryIter, and simplifies Esplora::update_local_chain with additional tests. See release notes for all the details.

### Fixed

- `InsertTxError` now implements `std::error::Error`. #1172
- Simplified `EsploraExt::update_local_chain` logic. #1267

### Changed

- `EntryIter` performance is improved by reducing syscalls. #1270
- Changed to implement `ElectrumExt` for all that implements `ElectrumApi`. #1306

### Added

- `Wallet` methods to apply full blocks (`apply_block` and `apply_block_connected_to`) and a method to apply a batch of unconfirmed transactions (`apply_unconfirmed_txs`). #1172
- `CheckPoint::from_block_ids` convenience method. #1172
- `LocalChain` methods to apply a block header (`apply_header` and `apply_header_connected_to`). #1172
- Test to show that `LocalChain` can apply updates that are shorter than original. This will happen during reorgs if we sync wallet with `bdk_bitcoind_rpc::Emitter`. #1172

## [v1.0.0-alpha.4]

### Summary

This release improves the `KeychainTxOutIndex` API and contains a few bug fixes and performance improvements.

### Fixed

- Avoid using `BTreeMap::append` due to performance issues (refer to https://github.com/rust-lang/rust/issues/34666#issuecomment-675658420). #1274

### Changed

- The old `hardwaresigner` module has been moved out of `bdk` and inside a new `bdk_hwi` crate. #1161
- Wallet's peek-address logic is optimized by making use of `<SpkIterator as Iterator>::nth`. #1269
- `KeychainTxOutIndex` API is refactored to better differentiate between methods that return unbounded vs stored spks. #1269
- `KeychainTxOutIndex` no longer directly exposes `SpkTxOutIndex` methods via `DeRef`. This was problematic because `SpkTxOutIndex` also contains lookahead spks which we want to hide. #1269

### Added

- LocalChain::disconnect_from method to evict a chain of blocks starting from a given BlockId. #1276
- `SpkIterator::descriptor` method which gets a reference to the internal descriptor. #1269

## [v1.0.0-alpha.3]

### Summary

This release changes LocalChain to have a hard-wired genesis block, adds context specific Wallet TxBuilder errors, and bumps the projects MSRV to 1.63. It also includes other API and docs improvements and bug fixes, see the changelog for all the details.

### Fixed

- Further improve unconfirmed tx conflict resolution. #1109
- Stuck Electrum chain sync issue. #1145
- Bug related to taproot signing with internal keys. We would previously sign with the first private key we had, without checking if it was the correct internal key or not. #1200
- Coinbase transactions cannot exist in the mempool and be unconfirmed. TxGraph::try_get_chain_position should always return None for coinbase transactions not anchored in best chain. #1202
- Esplora incorrect gap limit check in blocking client. #1225
- Loading a wallet from persistence now restores keychain indices. #1246

### Changed

- Rename ConfirmationTimeAnchor to ConfirmationTimeHeightAnchor. #1206
- New LocalChain now have a hardwired genesis block: #1178
  - Changed ChainOracle::get_chain_tip method to return a BlockId instead of an Option of a BlockId.
  - Refactored LocalChain so that the genesis BlockId is hardwired. This way, the ChainOracle::get_chain_tip implementation can always return a tip.
  - Add is_empty method to PersistBackend. This returns true when there is no data in the persistence.
  - Changed Wallet::new to initialize a fresh wallet only.
  - Added Wallet::load to restore an instance of a wallet.
  - Replaced Store::new with separate methods to create/open the database file.
- Updated the bdk module to use new context specific error types: #1028
  - wallet: MiniscriptPsbtError, CreateTxError, BuildFeeBumpError error enums.
  - coin_selection: module Error enum.
- Renamed fallible Wallet address functions to try_get_address() and try_get_internal_address(). #1028
- Rename LocalUtxo to LocalOutput. #1190
- MSRV is now 1.63.0 for bdk, chain, and bitcoind_rpc crates. #1183
- Use a universal lookahead value for KeychainTxOutIndex and have a reasonable default. #1229
- Return NonEmptyDatabase error when constructing a wallet with Wallet::new if the file already contains data (in which case, the caller should use load or new_or_load). #1256
- In electrum_ext rename functions scan_without_keychain to sync and scan to full_scan. #1235
- In esplora_ext rename functions scan_txs to sync and scan_txs_with_keychains to full_scan. #1235
- Increase rust-bip39 dependency version to 2.0 #1259

### Removed

- Removed catch-all top level bdk::Error enum. #1028
- Removed impl_error macro. #1028

### Added

- Add infallible Wallet get_address and get_internal_address functions. #1028
- Add Wallet::list_output method. #1190
- New async-https-rustls feature flag for the bdk_esplora crate, allowing to compile rust-esplora-client using rustls-tls instead of the default native-tls. #1179

## [v1.0.0-alpha.2]

### Summary

Notable changes include a new bitcoind RPC based blockchain client module for quick syncing to bitcoind,  a new linked-list LocalChain, and an upgrade to rust-bitcoin 0.30.

### Fixed

- wallet_esplora: missing_heights uses the graph update #1152
- bump electrum version to 0.18 #1132
- Correct the coin type in the derivation path for wallet examples #1089

### Added

- Add bitcoind_rpc chain-source module. #1041
- Add example_bitcoind_rpc example module. #1041
- Add AnchorFromBlockPosition trait which are for anchors that can be constructed from a given block, height and position in block. #1041
- Add helper methods to IndexedTxGraph and TxGraph for batch operations and applying blocks directly. #1041
- Add helper methods to CheckPoint for easier construction from a block Header. #1041
- Add cli-example for esplora. #1040
- Introduced tx_template module. #1064
- Introduced TxGraph::TxAncestors iterator. #1064
- Added walk_ancestors to TxGraph. #1064
- Implement Anchor for BlockId. #1069

### Changed

- Move WalletUpdate to the wallet module. #1084
- Remove ForEachTxout trait completely. #1084
- Refactor ElectrumExt to not use WalletUpdate. #1084
- Rename indexed_tx_graph::IndexedAdditions to indexed_tx_graph::ChangeSet. #1065
- Rename indexed_tx_graph::IndexedAdditions::graph_additions to indexed_tx_graph::ChangeSet::graph. #1065
- Rename indexed_tx_graph::IndexedAdditions::index_additions to indexed_tx_graph::ChangeSet::indexer. #1065
- Rename tx_graph::Additions to tx_graph::ChangeSet. #1065
- Rename keychain::DerivationAdditions to keychain::ChangeSet. #1065
- Rename CanonicalTx::node to CanonicalTx::tx_node. #1065
- Rename CanonicalTx::observed_as to CanonicalTx::chain_position. #1065
- Rename LocalChangeSet to WalletChangeSet. #1065
- Rename LocalChangeSet::chain_changeset to WalletChangeSet::chain. #1065
- Rename LocalChangeSet::indexed_additions to WalletChangeSet::indexed_tx_graph. #1065
- Rename LocalUpdate to WalletUpdate. #1065
- Make TxGraph::determine_changeset pub(crate). #1065
- Add TxGraph::initial_changeset. #1065
- Add IndexedTxGraph::initial_changeset. #1065
- Remove TxGraph::insert_txout_preview. #1065
- Remove TxGraph::insert_tx_preview. #1065
- Remove insert_anchor_preview. #1065
- Remove insert_seen_at_preview. #1065
- Refactored TxGraph::walk_conflicts to use TxGraph::TxAncestors. #1064
- Update to rust-bitcoin 0.30.0 and miniscript 10.0.0. #1023
- Use apply_update instead of determine_changeset + apply_changeset around the code. #1092
- Rename TxGraph::direct_conflicts_of_tx to TxGraph::direct_conflicts. #1164
- Rename methods of esplora ext. #1070

## [v1.0.0-alpha.1]

### Summary

The BDK 1.0.0-alpha release should be used for **experimentation only**, APIs are still unstable and the code is not fully tested. This alpha.1 release introduces the new `ChainOracle` struct for more efficient chain syncing. A new `std` default feature was added for `bdk`, `bdk_chain` and `bdk_esplora` crates; when disabled these crates can be used in `no-std` projects. BDK 1.0.0-alpha.x docs are now published to docs.rs.

### Fixed

- Fixed a bug in the policy condition calculation. #932
- Pin base64 to 0.21.0 #990
- Fix docsrs publishing for bdk crate. #1011

### Changed

- Refactor `bdk_chain` to use new `ChainOracle` structure. #926 #963 #965 #975 #976
- Better no std support. #894
  - Set `default-features = false` for `rust-bitcoin` and `rust-miniscript`.
  - Introduce `std` default feature for `bdk`, `bdk_chain` and `bdk_esplora`.

### Added

- Add a simple conversion tool for going to kilo weight units. #953
- Add Custom spk iterator. #927
- Add taproot descriptor template (BIP-86). #840

## [v0.30.0]

### Summary

This release bumps the MSRV to 1.63.0 and updates the `rusqlite` dependency version to 0.31 to be aligned with the upcoming 1.0.0 release. These changes will prepare projects for migrating wallet data to the 1.0.0 version of BDK, see #1648.

### Changed

- ci (maintenance): Pin byteorder, webpki to keep the MSRV by @danielabrozzoni in https://github.com/bitcoindevkit/bdk/pull/1160
- Bump MSRV 1.63.0 AND add deprecate TxBuilder::allow_shrinking() by @notmandatory in https://github.com/bitcoindevkit/bdk/pull/1366
- (maintenance) doc(bdk): Clarify the absolute_fee docs by @danielabrozzoni in https://github.com/bitcoindevkit/bdk/pull/1159
- release/0.29, deps: Update `rusqlite` to 0.31 by @ValuedMammal in https://github.com/bitcoindevkit/bdk/pull/1651
- test: update electrsd to fix RUSTSEC-2024-0384 warning by @notmandatory in https://github.com/bitcoindevkit/bdk/pull/1744

## [v0.29.0]

### Summary

This maintenance release updates our `rust-bitcoin` dependency to 0.30.x and fixes a wallet balance bug when a wallet has more than one coinbase transaction.

### Changed

- Update rust-bitcoin to 0.30 #1071

### Fixed

- Fix a bug when syncing coinbase utxos on electrum #1090

## [v0.28.2]

### Summary

Reverts the 0.28.1 esplora-client version update from 0.5.0 back to 0.4.0.

### Changed

- Reverts the 0.28.1 esplora-client version update from 0.5.0 back to 0.4.0.

## [v0.28.1]

### Summary

This patch release backports (from the BDK 1.0 dev branch) a fix for a bug in the policy condition calculation and adds a new taproot single key descriptor template (BIP-86). The policy condition calculation bug can cause issues when a policy subtree fails due to missing info even if it's not selected when creating a new transaction, errors on unused policy paths are now ignored.

### Fixed

- Backported #932 fix for policy condition calculation #1008

### Added

-  Backported #840 taproot descriptor template (BIP-86) #1033

## [v0.28.0]

### Summary

This is a maintenance release and includes changes from yanked version 0.27.2 including to disable default-features for rust-bitcoin and rust-miniscript dependencies, and for rust-esplora-client optional dependency. New default std feature must now be enabled unless building for wasm.

### Changed

- Bump bip39 crate to v2.0.0  #875
- Set default-features = false for rust-bitcoin and rust-miniscript #882
- Update esplora client dependency to version 0.4 #884
- Added new std feature as part of default features #930

## [v0.27.2] **YANKED**

RELEASE YANKED FROM CRATES.IO

See: [#897](https://github.com/bitcoindevkit/bdk/pull/897)

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
- Change the meaning of the `fee_amount` field inside `CoinSelectionResult`: from now on the `fee_amount` will represent only the fees associated with the utxos in the `selected` field of `CoinSelectionResult`.
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
- Remove unused variant `HeaderParseFail`

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
- Stop implicitly enforcing manual selection by .add_utxo
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
- Check last derivation in cache to avoid recomputing
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
- Use dirs-next instead of dirs since the latter is unmaintained

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
[v0.27.2]: https://github.com/bitcoindevkit/bdk/compare/v0.27.1...v0.27.2
[v0.28.0]: https://github.com/bitcoindevkit/bdk/compare/v0.27.2...v0.28.0
[v0.28.1]: https://github.com/bitcoindevkit/bdk/compare/v0.28.0...v0.28.1
[v0.28.2]: https://github.com/bitcoindevkit/bdk/compare/v0.28.1...v0.28.2
[v0.29.0]: https://github.com/bitcoindevkit/bdk/compare/v0.28.2...v0.29.0
[v0.30.0]: https://github.com/bitcoindevkit/bdk/compare/v0.29.0...v0.30.0
[v1.0.0-alpha.1]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-alpha.1
[v1.0.0-alpha.2]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-alpha.2
[v1.0.0-alpha.3]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-alpha.3
[v1.0.0-alpha.4]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-alpha.4
[v1.0.0-alpha.5]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-alpha.5
[v1.0.0-alpha.6]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-alpha.6
[v1.0.0-alpha.7]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-alpha.7
[v1.0.0-alpha.8]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-alpha.8
[v1.0.0-alpha.9]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-alpha.9
[v1.0.0-alpha.10]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-alpha.10
[v1.0.0-alpha.11]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-alpha.11
[v1.0.0-alpha.12]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-alpha.12
[v1.0.0-alpha.13]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-alpha.13
[v1.0.0-beta.1]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-beta.1
[v1.0.0-beta.2]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-beta.2
[v1.0.0-beta.3]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-beta.3
[v1.0.0-beta.4]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-beta.4
[v1.0.0-beta.5]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-beta.5
[v1.0.0-beta.6]: https://github.com/bitcoindevkit/bdk/releases/tag/v1.0.0-beta.6
[wallet-1.0.0]: https://github.com/bitcoindevkit/bdk/releases/tag/wallet-1.0.0
[wallet-1.1.0]: https://github.com/bitcoindevkit/bdk/releases/tag/wallet-1.1.0
[wallet-1.2.0]: https://github.com/bitcoindevkit/bdk/releases/tag/wallet-1.2.0
