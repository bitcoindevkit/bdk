# Changelog

All notable changes to this project can be found here and in each release's git tag and can be viewed with `git tag -ln100 "v*"`. See also [DEVELOPMENT_CYCLE.md](../../DEVELOPMENT_CYCLE.md) for more details.

Contributors do not need to change this file but do need to add changelog details in their PR descriptions. The person making the next release will collect changelog details from included PRs and edit this file prior to each release.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [esplora-0.22.0]

### Changed

- bump bdk_core to 0.6.0

## [esplora-0.21.0]

### Changed

- Make full-scan/sync flow easier to reason about. #1838
- Change `bdk_esplora` to understand `SpkWithExpectedTxids`. #1839
- deps: bump `esplora-client` to 0.12.0
- deps: bump `bdk_core` to 0.5.0
- deps: remove optional dependency on `miniscript`

## [esplora-0.20.1]

### Changed

- Minor updates to fix new rustc 1.83.0 clippy warnings #1776

[esplora-0.20.1]: https://github.com/bitcoindevkit/bdk/releases/tag/esplora-0.20.1
[esplora-0.21.0]: https://github.com/bitcoindevkit/bdk/releases/tag/esplora-0.21.0
[esplora-0.22.0]: https://github.com/bitcoindevkit/bdk/releases/tag/esplora-0.22.0
