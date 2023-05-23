# BDK Chain

This crate is a collection of core structures for [Bitcoin Dev Kit] (alpha release).

The goal of this crate is to give wallets the mechanisms needed to:

1. Figure out what data they need to fetch.
2. Process the data in a way that never leads to inconsistent states.
3. Fully index that data and expose it to be consumed without friction.

Our design goals for these mechanisms are:

1. Data source agnostic -- nothing in `bdk_chain` cares about where you get data from or whether
   you do it synchronously or asynchronously. If you know a fact about the blockchain, you can just
   tell `bdk_chain`'s APIs about it, and that information will be integrated, if it can be done
   consistently.
2. Error-free APIs.
3. Data persistence agnostic -- `bdk_chain` does not care where you cache on-chain data, what you
   cache or how you fetch it.

[Bitcoin Dev Kit]: https://bitcoindevkit.org/
