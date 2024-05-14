# BDK File Store

This is a simple append-only flat file implementation of [`PersistBackend`](bdk_persist::PersistBackend).

The main structure is [`Store`] which works with any [`bdk_chain`] based changesets to persist data into a flat file.

[`bdk_chain`]:https://docs.rs/bdk_chain/latest/bdk_chain/
