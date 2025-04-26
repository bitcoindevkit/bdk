# Changelog

All notable changes to this project can be found here and in each release's git tag and can be viewed with `git tag -ln100 "v*"`. See also [DEVELOPMENT_CYCLE.md](../../DEVELOPMENT_CYCLE.md) for more details.

Contributors do not need to change this file but do need to add changelog details in their PR descriptions. The person making the next release will collect changelog details from included PRs and edit this file prior to each release.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [file_store-0.20.0]

### Changed

- deps: bump `bdk_core` to 0.5.0

## [file_store-0.19.0]

### Added:

- `StoreError` enum, which includes `Io`, `Bincode` and `InvalidMagicBytes` #1684.
- docs: add "not intended for production" note in `README`.

### Changed:

- `Store::create_new` to `Store::create`, with new return type: `Result<Self, StoreError>`
- `Store::open` to `Store::load`, with new return type: `Result<(Self, Option<C>), StoreErrorWithDump<C>>`
- `Store::open_or_create` to `Store::load_or_create`, with new return type: `Result<(Option<C>, Self), StoreErrorWithDump<C>>`
- `Store::aggregate_changesets` to `Store::dump`, with new return type: `Result<Option<C>, StoreErrorWithDump<C>>`
- `FileError` to `StoreError`
- `AggregateChangesetsError` to `StoreErrorWithDump`, which now can include all the variants of `StoreError` in the error field.

#### Removed:

- `IterError` deleted.

## [file_store-0.18.1]

### Changed

- Minor updates to fix new rustc 1.83.0 clippy warnings #1776

[file_store-0.18.1]: https://github.com/bitcoindevkit/bdk/releases/tag/file_store-0.18.1
[file_store-0.19.0]: https://github.com/bitcoindevkit/bdk/releases/tag/file_store-0.19.0
[file_store-0.20.0]: https://github.com/bitcoindevkit/bdk/releases/tag/file_store-0.20.0
