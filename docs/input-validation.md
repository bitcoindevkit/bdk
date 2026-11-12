# ADR: input script validation in `bdk_chain`

## Problem

A chain source can inject an invalid unconfirmed transaction that spends the wallet's on-chain
outputs. `bdk_chain` believes it: the coins look spent, the balance moves, and the wallet replaces
against (RBF) or bumps (CPFP) a transaction that can never confirm.

## Decision

- **Only a provably Invalid unconfirmed candidate is excluded, during canonicalization.**
  `CanonicalTask::mark_canonical` runs `TxGraph::validate_tx` on a candidate that would be canonical
  only by being seen (`last_seen`, or a block outside the best chain), and on its ancestors that are
  not canonical yet. For each txid it selects a Valid whole copy if available, otherwise Unknown.
  Only if every retained copy is Invalid is the candidate rejected through the existing undo path,
  along with everything spending from it. The canonical view carries the exact selected copy.
  Valid and Unknown are both eligible, so nothing has to be proven valid.
- **Anchored and assumed transactions are never checked.** An anchor in the best chain, or
  `assume_canonical`, outranks validation for the transaction and the ancestry it pulls in. An
  anchor's trust is a matter of proof of work.
- **One rule set:** today's consensus script rules (`bdk_script_verify::Flags::CONSENSUS`), the
  ones every unconfirmed transaction must meet. There's no policy, sigop counting or block rules.
- **Floating txouts count as prevouts.** Each spent output comes from the graph's full transaction
  if it has one, and otherwise from the floating txout. An input is Unknown only when the graph has
  neither. Each verdict records `relies_on_floating`, and canonicalization ignores it.
- **No validation state persists between canonicalizations.** The graph keeps no verdicts and there
  is no switch. Each task memoizes its selected copies (or all-Invalid results), including across
  rejected walks, and judges the graph as it is. `validate_tx` answers for any transaction, in the
  graph or not.
- **The verifier is a new pure-Rust, `no_std` crate, `bdk_script_verify`**
  (`verify_input(&mut cache, vin, flags)` and `verify_transaction(tx, prevouts, flags)`), adapted
  from rbitcoin's interpreter. Missing
  prevouts are reported as missing, never as a script failure. Its tests run Core's script
  vectors, plus differential tests against Bitcoin Core's libbitcoinkernel. They use the
  `bitcoinkernel` 0.3.2 crate, which bundles Core master at `70d9ec7f` (2026-07-14). No published
  crate bundles a tagged release, and that snapshot's script interpreter differs from v32.1 only
  by refactors.
- **Every received copy of a txid is kept, by wtxid** (SQLite schema v4, `bdk_tx_copies`). The
  ordinary graph queries expose one copy, chosen without reading scripts: the minimum of
  `(empty witnesses, witness size, wtxid)`. Canonicalization uses that order within each validity
  class, preferring Valid over Unknown and rejecting only all-Invalid sets. It stops checking
  copies at the first Valid result. Witnesses from different copies are never spliced.

## Consequences

- An injected spend of the wallet's outputs can't carry a valid signature, so it comes out Invalid
  and is excluded, against a full parent or a floating txout. The valid conflict wins.
- Canonicalization may check multiple witness copies of an unconfirmed transaction. Selection is
  memoized for the task's lifetime; no cache is kept across canonicalizations.
- Each copy is checked through `bdk_script_verify::verify_transaction`, which shares signature-hash
  caches and prepared Taproot prevouts across its inputs. Per-input dependency reports stay
  independent. The caches borrow immutable transaction and prevout data and are discarded after
  the call, so they cannot outlive a copy or graph update.
- `ChangeSet`'s type is unchanged. Its `txs` may hold several copies of one txid.
- Unsigned or placeholder-script drafts inserted with a `last_seen` drop out once their parents
  are present, unless they are in `assume_canonical`. On master they stay canonical.

## Known limitations

- **A floating txout is the chain source's word.** A lying amount makes a genuine segwit or
  taproot spend Invalid. That can drop a transaction the wallet built with a foreign input, such as
  a payjoin, which master keeps. Inserting full parents would make those verdicts proof.
- Legacy signature hashing and input-specific script work can still scale quadratically. Segwit
  and Taproot reuse transaction-wide hashes, but returning a list of missing prevouts for every
  input can itself require quadratic output space.

## Signature-hash cache validation

Tests compare shared and independent caches, including repeated and reversed input order,
missing prevouts, failed signatures, and mixed legacy, segwit and Taproot inputs. Test-only
counters establish that shared BIP143 midstates and complete Taproot prevouts are prepared once
per transaction, rather than once per input. Core transaction vectors and differential tests
also compare the full results and dependency reports of both cache lifetimes.

A local release-build comparison on Apple Silicon with Rust 1.96 measured `TxGraph::validate_tx`
before this change (`d35b3277`) and with the shared cache, using isolated source and target
directories. Both ran the same valid signed fixtures, excluding signing and graph construction.
Times below are microseconds per transaction, averaged over 100 calls for 1/16 inputs and 10
calls for 128/1024 inputs. These are indicative timings, not test thresholds.

| Inputs | Segwit before | Segwit shared | Taproot before | Taproot shared |
|-------:|--------------:|--------------:|---------------:|---------------:|
| 1 | 27.1 | 27.2 | 26.5 | 26.4 |
| 16 | 452.1 | 422.9 | 467.1 | 420.0 |
| 128 | 4,735.3 | 3,472.7 | 5,996.8 | 3,438.1 |
| 1024 | 104,270.8 | 28,529.0 | 393,902.2 | 27,908.1 |

These timings include signature verification and graph prevout lookup. The improvement removes
repeated transaction-wide work; it does not make arbitrary scripts or legacy signature hashing
linear. The ignored `signature_hash_scaling` test provides an ongoing comparison of fresh
per-input caches against the graph's shared-cache path:

```sh
cargo test --release -p bdk_chain --all-features --test test_input_validation \
  signature_hash_scaling -- --exact --ignored --nocapture
```
