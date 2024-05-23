# BDK SQLite

This is a simple [SQLite] relational database schema backed implementation of [`PersistBackend`](bdk_persist::PersistBackend).

The main structure is `Store` which persists [`bdk_persist`] `CombinedChangeSet` data into a SQLite database file.

[`bdk_persist`]:https://docs.rs/bdk_persist/latest/bdk_persist/
[SQLite]: https://www.sqlite.org/index.html
