# BDK Script Verify

Pure-Rust verification of a transaction input's scriptSig, scriptPubKey and witness against the
output it spends, under Bitcoin's consensus script rules (P2SH, BIP66, BIP65, BIP112, BIP141,
BIP143, BIP147, BIP341 and BIP342).

It checks scripts only. It does not check amounts, locktimes against the chain, double spends or
any other transaction or block rule.

Use `verify_transaction` to check every input of a transaction. It shares signature-hash caches
and lazily prepared Taproot prevouts within that call, while returning a separate result and
prevout-dependency report for each input. To control input order or verify only selected inputs,
create a `VerificationCache` and pass it to `verify_input` for each check. The cache borrows immutable
transaction and prevout data and cannot be reused with another witness copy or changed prevouts.
The whole-transaction API discards its cache when it returns. This avoids repeated transaction-wide
hashing for segwit and Taproot.
Legacy signature hashes and scripts with input-specific work retain their own costs.

## Provenance

These parts are adapted from [rbitcoin](https://github.com/reardencode/rbitcoin) at commit
`49210ea82130474a4df366e253d1346a16369fdb`:

| Here | From rbitcoin |
|------|---------------|
| `src/interpreter.rs` (the interpreter) | `crates/rbitcoin-consensus/src/script/interpreter.rs` |
| `src/crypto.rs` (signature encoding checks, ECDSA verify) | the `crypto` module of `crates/rbitcoin-consensus/src/script/mod.rs` |
| `src/sighash.rs` (raw-hashtype BIP143) | `bip143_signature_hash` in `crates/rbitcoin-consensus/src/script/mod.rs` |
| `src/interpreter.rs` `verify_taproot_commitment` | `verify_control_commitment` in `crates/rbitcoin-consensus/src/script/p2tr.rs` |
| `src/scriptnum.rs` | `crates/rbitcoin-primitives/src/scriptnum.rs` |

rbitcoin is licensed under MIT OR Apache-2.0, "Copyright (c) 2026 rbitcoin contributors". Those
files carry no per-file headers. Each adapted file says so at the top, and
`verify_taproot_commitment` in its doc comment. The adaptation removes the
block-validation job, precompute and thread-pool plumbing and the policy (standardness) flags. It
replaces rbitcoin's typed fast paths with Bitcoin Core's `VerifyScript` flow run through the one
interpreter.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](../../LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.
