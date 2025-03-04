# Remove `persist` module from `bdk_chain`

* Status: accepted
* Authors: _
* Date: 2024-06-14
* Targeted modules: `bdk_chain`
* Associated tickets: PRs #1454, #1473, refined by [ADR-0002](./0002_persisted.md)

## Context and Problem

How to enable the `Wallet` to work with an async persist backend?

A long-standing problem of the pre-1.0 BDK was that the "wallet is not async". In particular this made the timing of database reads and writes suboptimal considering the main benefit of asynchronous programming is to do async IO. Users who were accustomed to utilizing an asynchronous framework at the persistence layer thus had a difficult time working around this limitation of the wallet.

A series of persistence-related PRs took place all aimed at reducing pain points that included removing generics from the `Wallet` structure (#1387) to alleviate ffi headaches, moving the persist module to its own crate to isolate a dependency problem (#1412), taking the database out of the wallet completely using a new "take-staged" approach (#1454), and finally the decision to scrap the persist module defining the `PersistBackend` trait which previously all backends were required to implement (#1473).

## Motivation

Allow persistence to work with any database including the ability to utilize asynchronous IO. For example, one desired use case is to use a high-performance database such as PostgreSQL in a server environment. 

## Considered options

#### Option 1: Introduce a (not so) new trait `PersistBackendAsync`

The first approach was to create another trait `PersistBackendAsync` that was essentially equivalent to the existing `PersistBackend`, only that its methods were declared `async fn`. To accomplish this, we added the `#[async_trait]` macro which seemed acceptable although comes with the obvious downside of duplicating code. The perceived benefit of this approach was that it offered a suitable replacement for use in an async context while providing a familiar interface.

#### Option 2: Return futures from persistence backend functions

Another idea that was offered was to return something implementing `Future` from methods like `commit`. The idea was that it would minimize added dependencies and increase flexibility by allowing the caller to `await` the result. In the end it seems less of an effort was put toward executing this idea.

## Decision

The ultimate decision was to seek maximum flexibility by decoupling persistence from both wallet and chain, making these crates totally agnostic to the persistence layer. There is no more `PersistBackend` trait - instead the wallet and its internal structures are allowed to assume that all persisted state can be recovered intact.

The critical piece that allows this to work is the `CombinedChangeSet` structure residing in `bdk_chain`. This means that anything capable of (de)serializing the combined changeset has all the information necessary to save and recover a previous state.

## Outcomes

#### Implications for UX

The result of our decision could be more of a UX burden on users in that it requires more hands-on involvement with the `CombinedChangeSet` structure, whereas before these details were kept internal and we merely exposed an ergonomic API via `stage` and `commit`, etc.

On the other hand, users no longer need to implement a persistence trait that BDK understands, and the added flexibility should enable more users to integrate BDK into their systems with fewer limitations.

#### Core library development

Library development is for the most part similar to before in the sense that we stage changes as they become relevant and let the user decide when to commit them. It is believed that the increased modularity and separation of concerns should lead to a significant quality of life improvement.

It remains the job of the developers to adequately document and showcase a correct use of the API. It is important that we clearly define the structure of the changeset that we now expect users to be aware of and to be proactive and transparent when communicating upgrades to the data model.
