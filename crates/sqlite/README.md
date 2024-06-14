# BDK SQLite

This is a simple [SQLite] relational database client for persisting [`bdk_chain`] changesets.

The main structure is `Store` which persists `CombinedChangeSet` data into a SQLite database file.

[`bdk_chain`]:https://docs.rs/bdk_chain/latest/bdk_chain/
[SQLite]: https://www.sqlite.org/index.html
