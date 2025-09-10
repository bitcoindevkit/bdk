# Changelog

All notable changes to this project can be found here and in each release's git tag and can be viewed with `git tag -ln100 "v*"`. See also [DEVELOPMENT_CYCLE.md](../../DEVELOPMENT_CYCLE.md) for more details.

Contributors do not need to change this file but do need to add changelog details in their PR descriptions. The person making the next release will collect changelog details from included PRs and edit this file prior to each release.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [bitcoind_rpc-0.22.0]

### Fixed
- `FilterIter` now handles reorgs to ensure consistency of the header chain. #2000

### Changed
- `Event` is now a struct instead of enum #2000

### Added
- `FilterIter::new` constructor that takes as input a reference to the RPC client, checkpoint, and a list of SPKs. #2000
- `Error::ReorgDepthExceeded` variant. #2000
- `Error::TryFromInt` variant. #2000

### Removed
- `FilterIter::new_with_height` method #2000
- `FilterIter::new_with_checkpoint` method #2000
- `EventInner` type #2000
- `FilterIter::get_tip` method #2000
- `FilterIter::chain_update` method #2000
- `Error::NoScripts` variant #2000

## [bitcoind_rpc-0.21.0]

### Added

- Introduce usage of `cfg_attr(coverage_nightly)` in order to not consider tests under coverage. #1986

### Changed

- deps: bump bdk_core to 0.6.1

### Fixed

- Some mempool transactions not being emitted at all, it was fixed by simplifying the emitter, and replaced the avoid-re-emission-logic with a new one that emits all mempool transactions. #1988
- Use the `getrawmempool` without verbose, as a more performant method. `Emitter::mempool` method now requires the `std` feature. A non-std version of this is added: `Emitter::mempool_at` #1988

## [bitcoind_rpc-0.20.0]

### Changed

- bump bdk_core to 0.6.0

## [bitcoind_rpc-0.19.0]

### Changed

- feat(rpc)!: Update Emitter::mempool to support evicted_at #1857
- Change Emitter::mempool to return MempoolEvents which contain mempool-eviction data.
- Change Emitter::client to have more relaxed generic bounds. C: Deref, C::Target: RpcApi are the new bounds.
- deps: bump `bdk_core` to 0.5.0

## [bitcoind_rpc-0.18.0]

### Added

- Added `bip158` module as a means of updating `bdk_chain` structures #1614

## [bitcoind_rpc-0.17.1]

### Changed

- Minor updates to fix new rustc 1.83.0 clippy warnings #1776

[bitcoind_rpc-0.17.1]: https://github.com/bitcoindevkit/bdk/releases/tag/bitcoind_rpc-0.17.1
[bitcoind_rpc-0.18.0]: https://github.com/bitcoindevkit/bdk/releases/tag/bitcoind_rpc-0.18.0
[bitcoind_rpc-0.19.0]: https://github.com/bitcoindevkit/bdk/releases/tag/bitcoind_rpc-0.19.0
[bitcoind_rpc-0.20.0]: https://github.com/bitcoindevkit/bdk/releases/tag/bitcoind_rpc-0.20.0
[bitcoind_rpc-0.21.0]: https://github.com/bitcoindevkit/bdk/releases/tag/bitcoind_rpc-0.21.0
[bitcoind_rpc-0.22.0]: https://github.com/bitcoindevkit/bdk/releases/tag/bitcoind_rpc-0.22.0