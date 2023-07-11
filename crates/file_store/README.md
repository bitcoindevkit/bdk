# BDK File Store

This is a simple append-only flat file implementation of
[`Persist`](`bdk_chain::Persist`).

The main structure is [`Store`](`crate::Store`), which can be used with [`bdk`]'s
`Wallet` to persist wallet data into a flat file.

[`bdk`]: https://docs.rs/bdk/latest
[`bdk_chain`]: https://docs.rs/bdk_chain/latest
