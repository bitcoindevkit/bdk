# BDK File Store

> ⚠ `bdk_file_store` is a development/testing database. It does not natively support backwards compatible BDK version upgrades so should not be used in production.

This is a simple append-only flat file database for persisting [`bdk_chain`] changesets.

The main structure is [`Store`] which works with any [`bdk_chain`] based changesets to persist data into a flat file.

[`bdk_chain`]:https://docs.rs/bdk_chain/latest/bdk_chain/
