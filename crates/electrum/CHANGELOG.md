# Changelog

All notable changes to this project can be found here and in each release's git tag and can be viewed with `git tag -ln100 "v*"`. See also [DEVELOPMENT_CYCLE.md](../../DEVELOPMENT_CYCLE.md) for more details.

Contributors do not need to change this file but do need to add changelog details in their PR descriptions. The person making the next release will collect changelog details from included PRs and edit this file prior to each release.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
