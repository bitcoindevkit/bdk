# BDK File Store

This is a simple append-only flat file implementation of
[`PersistBackend`](bdk_persist::PersistBackend).

The main structure is [`Store`](crate::Store), which can be used with [`bdk`]'s
`Wallet` to persist wallet data into a flat file.

[`bdk`]: https://docs.rs/bdk/latest
[`bdk_persist`]: https://docs.rs/bdk_persist/latest
