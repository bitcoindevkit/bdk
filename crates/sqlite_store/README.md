# BDK SQLite Store

This is a simple append-only [SQLite] database backed implementation of
[`Persist`](`bdk_chain::Persist`).

The main structure is [`Store`](`crate::Store`), which can be used with [`bdk`]'s
`Wallet` to persist wallet data into a SQLite database file.

[`bdk`]: https://docs.rs/bdk/latest
[`bdk_chain`]: https://docs.rs/bdk_chain/latest
[SQLite]: https://www.sqlite.org/index.html
