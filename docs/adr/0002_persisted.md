# Introduce `PersistedWallet`

* Status: accepted
* Authors: _
* Date: 2024-08-19
* Targeted modules: `bdk_wallet`
* Associated tickets: #1514, #1547

## Context and Problem

BDK v1.0.0-beta.1 introduced new persistence traits `PersistWith<Db>` and `PersistAsyncWith<Db>`. The design was slightly cumbersome, the latter proving difficult to implement by wallet users.

## Decision Drivers

We would like persistence operations to be both safe and ergonomic. It would cumbersome and error prone if users were expected to ask the wallet for a changeset at the correct time and persist it on their own. Considering that the wallet no longer drives its own database, an ideal interface is to have a method on `Wallet` such as [`persist`][1] that the user will call and everything will "just work."

## Chosen option

#### Introduce a new type `PersistedWallet` that wraps a BDK `Wallet` and provides a convenient interface for executing persistence operations aided by a `WalletPersister`.

`PersistedWallet` internally calls the methods of the `WalletPersister` trait. We do this to ensure consistency of the create and load steps particularly when it comes to handling staged changes and to reduce the surface area for footguns. Currently BDK provides implementations of `WalletPersister` for a [SQLite backend using `rusqlite`][2] as well as a [flat-file store][3].

The two-trait design was kept in order to accommodate both blocking and async use cases. For `AsyncWalletPersister` the definition is modified to return a [`FutureResult`][4] which is a rust-idiomatic way of creating something that can be polled by an async runtime. For example in the case of `persist` the implementer writes an `async fn` that does the persistence operation and then calls `Box::pin` on that.
```rust
impl AsyncWalletPersister for MyCustomDb {
    ...

    fn persist<'a>(persister: &'a mut Self, changeset: &'a ChangeSet) -> FutureResult<'a, (), Self::Error>
    where
        Self: 'a,
    {
        let persist_fn = async move |changeset| {
            // perform write operation...
            Ok(())
        };
    
        Box::pin(persist_fn)
    }
}
```

**Pros:**

* Relatively safe, ergonomic, and generalized to accommodate different storage implementations.

**Cons:**

* Requires manual intervention, i.e., how does a user know when and how often to call `persist`, whether it can be deferred until later, and what to do in case of persistence failure? As a first approximation we consider any operation that mutates the internal state of the wallet to be worthy of persisting. Others have suggested implementing more [fine-grained notifications][5] that are meant to trigger persistence.

<!-- ## Links -->
[1]: https://github.com/bitcoindevkit/bdk/blob/8760653339d3a4c66dfa9a54a7b9d943a065f924/crates/wallet/src/wallet/persisted.rs#L52
[2]: https://github.com/bitcoindevkit/bdk/blob/88423f3a327648c6e44edd7deb15c9c92274118a/crates/wallet/src/wallet/persisted.rs#L257-L287
[3]: https://github.com/bitcoindevkit/bdk/blob/8760653339d3a4c66dfa9a54a7b9d943a065f924/crates/wallet/src/wallet/persisted.rs#L314
[4]: https://github.com/bitcoindevkit/bdk/blob/8760653339d3a4c66dfa9a54a7b9d943a065f924/crates/wallet/src/wallet/persisted.rs#L55
[5]: https://github.com/bitcoindevkit/bdk/issues/1542#issuecomment-2276066581
