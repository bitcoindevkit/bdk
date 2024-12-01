# Introduce `O(n)` Canonicalization Algorithm

* Status: Proposed
* Authors: @LLFourn, @evanlinjin
* Date: 2024-12-01
* Targeted modules: `bdk_chain`
* Associated Tickets/PRs: Issue #1665, ~PR #1659~, PR #1670

## Context and Problem Statement

The [2024 Wizardsardine BDK code audit](https://gist.github.com/darosior/4aeb9512d7f1ac7666abc317d6f9453b) uncovered the severity of the performance issues in the original canonicalization logic. The problem is especially severe for wallet histories with many unconfirmed and conflicting transactions. This can be a dDos vector if BDK is used in server-side applications. The time complexity of the original canonicalization logic is $O(n^2)$.

The old canonicalization logic is based on `TxGraph::get_chain_position`. This is called on every transaction included in `TxGraph`, having to traverse backwards and forwards to check that all ancestors do not conflict with anything that is anchored in the best chain and that no conflict has a higher `last-seen` value. Also note that `last-seen` values are transitive, so to determine the *actual* `last-seen` value, we need to iterate through all descendants.

## Considered Options

#### Option 1: Introduce a `canonical_cache` as a parameter to all `get_chain_position`-based methods.

The `canonical_cache` will include both `canonical` and `not_canonical` sets of txids. This avoids revisiting what has already been visited.

**Pros:**
* Least API and code changes.

**Cons:**
* The API can be misused. Can get wildly wrong results if the `canonical_cache` parameter is used across updates to `TxGraph` or the `ChainOracle` impl.
* Visiting transactions in a certain order may decrease the number of traversals. I.e. if we call `get_chain_position` on transactions with anchors first, `get_chain_position` calls on non-anchored transactions later on won't need to do as much work. Visiting order is not enforced if we stick to a `get_chain_position`-based API.

#### Option 2: Traverse `TxGraph` spends forwards, starting from graph roots.

For this algorithm, we maintain two `txid` sets; `maybe_canonical` and `not_canonical`. Note that these sets are not mutually exclusive since we are traversing spends, and a transaction can have multiple inputs (spends). When we arrive at a transaction's input (spend), we may not have checked all of the transaction's other inputs to be sure that an ancestor does not conflict with a transaction that is anchored or has a higher last-seen value.

**Pros:**
* API cannot be misused (as it can in option 1).
* We can traverse transactions in a pseudo-chronological order.

**Cons:**
* Duplicate work may have to be done if we have transactions with multiple inputs. We may mark a subset of transactions as `maybe_canonical`, then end up having to mark a majority of those as `not_canonical` later on if a spend of a previously-visited transaction is determined to be a descendant of a `not_canonical` transaction.
* Does not handle transitively-anchored transactions properly. If a transaction is anchored in the best chain, all of it's ancestors are anchored in the best chain even though they do not have an explicit anchor attached. To find transitive anchors, we need to traverse backwards. However this algorithm only traverses forwards.

#### Option 3: Traverse `TxGraph` backwards, starting from transactions with the highest `last-seen` values.

The premise is that transactions with higher last-seen values are most likely to be canonical and not conflict with transactions anchored in the best chain (since they are seen most recently in the mempool).

The algorithm maintains 2 `txid` sets. One for `canonical` and `not_canonical`. These are mutually exclusive sets. A transaction that is included in either of these sets have already been visited and can be skipped. We iterate through all transactions, ordered by descending last-seen values.

For each transaction, we traverse it's ancestors, stopping when we hit a confirmed transaction or a transaction that conflicts with a confirmed transaction. If a conflict with a confirmed transaction is found, we can mark that transaction and all it's descendants as `not_canonical`. Otherwise, the entire subset will be `canonical`. If we hit a transaction that is anchored in the best chain, we can mark it and all of it's ancestors as `canonical`.

**Pros:**
* We can efficiently mark large subsets as canonical/not-canonical.

**Cons:**
* Like option 2, this does not handle transitively-anchored transactions properly.

#### Option 4: Traverse transactions with anchors first.

The algorithm's premise is as follows:

1. If transaction `A` is determined to be canonical, all of `A`'s ancestors must also be canonical.
2. If transaction `B` is determined to be NOT canonical, all of `B`'s descendants must also be NOT canonical.
3. If a transaction is anchored in the best chain, it is canonical.
4. If a transaction conflicts with a canonical transaction, it is NOT canonical.
5. A transaction with a higher last-seen has precedence.
6. Last-seen values are transitive. A transaction's real last-seen value is the max between it's last-seen value all of it's descendants.

Like Option 3's algorithm, we maintain two mutually-exclusive `txid` sets: `canoncial` and `not_canonical`.

Imagine a method `mark_canonical(A)` that is based on premise 1 and 2. This method will mark transaction `A` and all of it's ancestors as canonical. For each transaction that is marked canonical, we can iterate all of it's conflicts and mark those as `non_canonical`. If a transaction already exists in `canoncial` or `not_canonical`, we can break early, avoiding duplicate work.

This algorithm iterates transactions in 3 runs.

1. Iterate over all transactions with anchors in descending anchor-height order. For any transaction that has an anchor pointing to the best chain, we call `mark_canonical` on it. We iterate in descending-height order to reduce the number of anchors we need to check against the `ChainOracle` (premise 1). The purpose of this run is to populate `non_canonical` with all transactions that directly conflict with anchored transactions and populate `canonical` with all anchored transactions and ancestors of anchors transactions (transitive anchors).
2. Iterate over all transactions with last-seen values, in descending last-seen order. We can call `mark_canonical` on all of these that do not already exist in `canonical` or `not_canonical`.
3. Iterate over remaining transactions that contains anchors (but not in the best chain) and have no last-seen value. We treat these transactions in the same way as we do in run 2.

**Pros:**
* Transitive anchors are handled correctly.
* We can efficiently mark large subsets as canonical/non-canonical.

**Cons:** ?

## Decision Outcome

Option 4 is implemented in PR #1670.
