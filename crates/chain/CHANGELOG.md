# Changelog

All notable changes to this project can be found here and in each release's git tag and can be viewed with `git tag -ln100 "v*"`. See also [DEVELOPMENT_CYCLE.md](../../DEVELOPMENT_CYCLE.md) for more details.

Contributors do not need to change this file but do need to add changelog details in their PR descriptions. The person making the next release will collect changelog details from included PRs and edit this file prior to each release.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
