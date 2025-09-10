# Changelog

All notable changes to this project can be found here and in each release's git tag and can be viewed with `git tag -ln100 "v*"`. See also [DEVELOPMENT_CYCLE.md](../../DEVELOPMENT_CYCLE.md) for more details.

Contributors do not need to change this file but do need to add changelog details in their PR descriptions. The person making the next release will collect changelog details from included PRs and edit this file prior to each release.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [electrum-0.23.2]

### Added

- Add a new `populate_anchor_cache` method. #2005

### Changed

- Update the `populate_tx_cache` method documentation. #2005

### Fixed

- Update the `batch_fetch_anchors` to no longer uses a potentially stale hash as the anchor. #2011

## [electrum-0.23.1]

### Added
- Introduce usage of `cfg_attr(coverage_nightly)` in order to not consider tests under coverage. #1986

### Changed

- Add transaction anchor cache to prevent redundant network calls. #1957
- Batch Merkle Proof, script history, and header requests. #1957
- Fix `clippy` warnings on electrum-client batch call #1990
- deps: bump bdk_core to 0.6.1
- deps: bump electrum-client to 0.24.0

### Fixed
- Removed all `unwrap()`s and `expect()`s from `bdk_electrum_client.rs` #1981

## [electrum-0.23.0]

### Changed

- bump bdk_core to 0.6.0

## [electrum-0.22.0]

### Fixed

- Fix `bdk_electrum` handling of negative spk-history height. #1837

### Changed

- Make full-scan/sync flow easier to reason about. #1838
- Change `bdk_electrum` to understand `SpkWithExpectedTxids`. #1839
- deps: bump `electrum-client` to 0.23.1
- deps: bump `bdk_core` to 0.5.0

## [electrum-0.21.0]

### Changed

- Bump crate MSRV to 1.75.0 #1803
- deps: bump `electrum-client` to 0.23.0
- add test for checking that fee calculation is correct #1685

## [electrum-0.20.1]

### Changed

- Minor updates to fix new rustc 1.83.0 clippy warnings #1776

[electrum-0.20.1]: https://github.com/bitcoindevkit/bdk/releases/tag/electrum-0.20.1
[electrum-0.21.0]: https://github.com/bitcoindevkit/bdk/releases/tag/electrum-0.21.0
[electrum-0.22.0]: https://github.com/bitcoindevkit/bdk/releases/tag/electrum-0.22.0
[electrum-0.23.0]: https://github.com/bitcoindevkit/bdk/releases/tag/electrum-0.23.0
[electrum-0.23.1]: https://github.com/bitcoindevkit/bdk/releases/tag/electrum-0.23.1
[electrum-0.23.2]: https://github.com/bitcoindevkit/bdk/releases/tag/electrum-0.23.2
