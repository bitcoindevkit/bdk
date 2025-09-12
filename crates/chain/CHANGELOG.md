# Changelog

All notable changes to this project can be found here and in each release's git tag and can be viewed with `git tag -ln100 "v*"`. See also [DEVELOPMENT_CYCLE.md](../../DEVELOPMENT_CYCLE.md) for more details.

Contributors do not need to change this file but do need to add changelog details in their PR descriptions. The person making the next release will collect changelog details from included PRs and edit this file prior to each release.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [chain-0.23.2]

### Added

- Added a doctest illustrating how to filter confirmed balance results by simulating a minimum confirmation threshold via `chain_tip` height. #2007

### Fixed

- Behavior of `IndexedTxGraph` methods (`apply_block_relevant`, `batch_insert_relevant` and `batch_insert_relevant_unconfirmed`) to also consider conflicts of spk-relevant transactions as relevant. #2008

## [chain-0.23.1]

### Added

- Add new tests covering excluded bounds and the `SpkTxOutIndex::outputs_in_range` #1897
- Add new `reindex_tx_graph` benchmark, and unit test to cover `spk_cache` behavior #1968
- Add new `TxGraph::get_last_evicted` method #1977

### Changed

- Improve API docs, fixed parameter names to match the function or struct definition, and correct some spelling mistakes. #1968
- `KeychainTxOutIndex::apply_changeset` restores spk_cache before last revealed #1993
- deps: bump bdk_core to 0.6.1

### Fixed
- During canonicalization, exclude coinbase transactions when considering txs that are anchored in stale blocks #1976

## [chain-0.23.0]

### Added

- feat!(chain): implement `first_seen` tracking #1950
- fix(chain): persist `first_seen` #1966
- Persist spks derived from KeychainTxOutIndex #1963

### Changed

- fix(docs): `merge_chains` outdated documentation #1806
- chore: create and apply rustfmt.toml #1946
- chore: Update rust-version to 1.86.0 #1955
- bump bdk_core to 0.6.0

## [chain-0.22.0]

### Added

- Introduce evicted-at/last-evicted timestamps #1839
- Add method for constructing TxGraph from a ChangeSet #1930
- docs: Architectural Decision Records #1592
- Introduce canonicalization parameters #1808
- Add conversion impls for CanonicalTx to Txid/Arc<Transaction>.
- Add ChainPosition::is_unconfirmed method.

### Changed

- Make full-scan/sync flow easier to reason about. #1838
- Change `TxGraph` to track `last_evicted` timestamps. This is when a transaction is last marked as missing from the mempool.
- The canonicalization algorithm now disregards transactions with a `last_evicted` timestamp greater than or equal to it's `last_seen` timestamp, except when a canonical descendant exists due to rules of transitivity. #1839
- deps: bump miniscript to 12.3.1 #1924

### Fixed

- Fix canonicalization mess-up when transactions that conflict with itself are inserted. #1917

### Removed

- Remove `apply_update_at` as we no longer need to apply with a timestamp after-the-fact.

## [chain-0.21.1]

### Changed

- Minor updates to fix new rustc 1.83.0 clippy warnings #1776

[chain-0.21.1]: https://github.com/bitcoindevkit/bdk/releases/tag/chain-0.21.1
[chain-0.22.0]: https://github.com/bitcoindevkit/bdk/releases/tag/chain-0.22.0
[chain-0.23.0]: https://github.com/bitcoindevkit/bdk/releases/tag/chain-0.23.0
[chain-0.23.1]: https://github.com/bitcoindevkit/bdk/releases/tag/chain-0.23.1
[chain-0.23.2]: https://github.com/bitcoindevkit/bdk/releases/tag/chain-0.23.2