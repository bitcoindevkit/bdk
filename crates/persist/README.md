# BDK Persist

This crate is home to the [`PersistBackend`](crate::PersistBackend) trait which defines the behavior of a database to perform the task of persisting changes made to BDK data structures. The [`Persist`](crate::Persist) type provides a convenient wrapper around a `PersistBackend` that allows staging changes before committing them.