# Changelog

All notable changes to this project can be found here and in each release's git tag and can be viewed with `git tag -ln100 "v*"`. See also [DEVELOPMENT_CYCLE.md](../../DEVELOPMENT_CYCLE.md) for more details.

Contributors do not need to change this file but do need to add changelog details in their PR descriptions. The person making the next release will collect changelog details from included PRs and edit this file prior to each release.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

## [core-0.5.0]

### Added

- Add `FullScanRequest::builder_at` and `SyncRequest::builder_at` methods which are the non-std version of the `..Request::builder` methods.
- Add `TxUpdate::evicted_ats` which tracks transactions that have been replaced and are no longer present in mempool.
- Add `SpkWithExpectedTxids` in `spk_client` which keeps track of expected `Txid`s for each `spk`.
- Add `SyncRequestBuilder::expected_txids_of_spk` method which adds an association between `txid`s and `spk`s.
- test: add tests for `Merge` trait #1738

### Changed

- Make full-scan/sync flow easier to reason about. #1838
- Change `FullScanRequest::builder` and `SyncRequest::builder` methods to depend on `feature = "std"`.
This is because requests now have a `start_time`, instead of specifying a `seen_at` when applying the update.
- Change `TxUpdate` to be `non-exhaustive`.
- Change `TxUpdate::seen_ats` field to be a `HashSet` of `(Txid, u64)`. This allows a single update to have multiple `seen_at`s per tx.
- Introduce `evicted-at`/`last-evicted` timestamps #1839

## [core-0.4.1]

### Changed

- Minor updates to fix new rustc 1.83.0 clippy warnings #1776

[core-0.4.1]: https://github.com/bitcoindevkit/bdk/releases/tag/core-0.4.1
[core-0.5.0]: https://github.com/bitcoindevkit/bdk/releases/tag/core-0.5.0
