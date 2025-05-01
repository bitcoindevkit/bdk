# Changelog

All notable changes to this project can be found here and in each release's git tag and can be viewed with `git tag -ln100 "v*"`. See also [DEVELOPMENT_CYCLE.md](../../DEVELOPMENT_CYCLE.md) for more details.

Contributors do not need to change this file but do need to add changelog details in their PR descriptions. The person making the next release will collect changelog details from included PRs and edit this file prior to each release.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [bitcoind_rpc-0.19.0]

## Changed

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
