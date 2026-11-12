//! Pure-Rust verification of a transaction input's scripts and witness under Bitcoin's consensus
//! script rules.
//!
//! [`verify_input`] runs one input's scriptSig, the scriptPubKey it spends and its witness, as
//! Bitcoin Core's `VerifyScript` does. Spent outputs that are not known are passed as `None`. The
//! result then tells apart a script that failed from one whose outcome depends on data that is
//! missing.
//! [`verify_transaction`] checks all inputs while reusing transaction-wide signature hashes.
//!
//! Only scripts are checked: not amounts, locktimes against the chain, double spends or any other
//! transaction or block rule.
#![no_std]
#![warn(missing_docs)]

extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

mod crypto;
mod interpreter;
mod scriptnum;
mod sighash;

#[cfg(test)]
mod tests;

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use bitcoin::{Transaction, TxOut};

use sighash::Sighasher;

/// Which script rules apply.
///
/// Each flag is one of Bitcoin Core's consensus `SCRIPT_VERIFY_*` flags. Policy (standardness)
/// flags are not supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Flags {
    /// BIP16 pay-to-script-hash.
    pub p2sh: bool,
    /// BIP66 strict DER signatures.
    pub dersig: bool,
    /// BIP65 `OP_CHECKLOCKTIMEVERIFY`.
    pub checklocktimeverify: bool,
    /// BIP112 `OP_CHECKSEQUENCEVERIFY`.
    pub checksequenceverify: bool,
    /// BIP141/BIP143 segregated witness. Requires `p2sh`.
    pub witness: bool,
    /// BIP147 empty `OP_CHECKMULTISIG` dummy.
    pub nulldummy: bool,
    /// BIP341/BIP342 taproot. Requires `witness`.
    pub taproot: bool,
}

impl Flags {
    /// Every consensus rule active at the tip of the Bitcoin main chain.
    pub const CONSENSUS: Self = Self {
        p2sh: true,
        dersig: true,
        checklocktimeverify: true,
        checksequenceverify: true,
        witness: true,
        nulldummy: true,
        taproot: true,
    };
}

impl Default for Flags {
    fn default() -> Self {
        Self::CONSENSUS
    }
}

/// The outcome of [`verify_input`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verification {
    /// `Ok` if the input's scripts pass.
    pub result: Result<(), Error>,
    /// Which entries of the `prevouts` argument the result depends on.
    ///
    /// The same transaction, flags and values of these entries always give the same result.
    pub depends_on: DependsOn,
}

/// The spent outputs a [`Verification`] depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DependsOn {
    /// Only the output spent by the verified input.
    OwnPrevout,
    /// The outputs spent by every input of the transaction, as a BIP341 signature hash commits to
    /// them all.
    AllPrevouts,
}

/// Why an input did not verify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The outcome depends on spent outputs that were passed as `None`. Holds their input indices.
    ///
    /// This is not a failure: the input may verify once they are known.
    MissingPrevouts(Vec<usize>),
    /// The scripts conclusively failed.
    Script(ScriptError),
}

/// A conclusive script failure, named after Bitcoin Core's `SCRIPT_ERR_*` code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScriptError(&'static str);

impl ScriptError {
    /// Bitcoin Core's name for the failure, without its `SCRIPT_ERR_` prefix.
    pub fn code(&self) -> &'static str {
        self.0
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::MissingPrevouts(inputs) => write!(f, "missing prevouts of inputs {inputs:?}"),
            Error::Script(e) => write!(f, "script failed: {e}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

#[derive(Debug)]
enum Fail {
    Script(&'static str),
    Missing(Vec<usize>),
}

impl From<Fail> for Error {
    fn from(fail: Fail) -> Self {
        match fail {
            Fail::Script(code) => Error::Script(ScriptError(code)),
            Fail::Missing(inputs) => Error::MissingPrevouts(inputs),
        }
    }
}

/// Reusable signature-hash data for one transaction and its spent outputs.
///
/// Borrows immutable data so cached hashes cannot be reused with a different transaction or
/// prevouts. Pass the same cache to [`verify_input`] for each input, in any order. Input verdicts
/// and dependency reports are not cached.
pub struct VerificationCache<'a> {
    sighasher: Sighasher<'a>,
}

impl<'a> VerificationCache<'a> {
    /// Creates an empty cache. `prevouts[i]` is the output spent by input `i`, or `None`.
    /// Hashes and complete Taproot prevout data are prepared lazily when needed.
    ///
    /// # Panics
    ///
    /// If `prevouts.len() != tx.input.len()`.
    pub fn new(tx: &'a Transaction, prevouts: &'a [Option<&'a TxOut>]) -> Self {
        assert_eq!(prevouts.len(), tx.input.len(), "one prevout per input");
        Self {
            sighasher: Sighasher::new(tx, prevouts),
        }
    }
}

/// Verifies the scripts of `input_index`, reusing the supplied cache.
///
/// Only the prevout entries named by [`Verification::depends_on`] affect this input's result,
/// independently of earlier calls. Coinbase inputs always pass.
///
/// ```
/// # use bdk_script_verify::{verify_input, Flags, VerificationCache};
/// # use bitcoin::{Transaction, TxOut};
/// # fn check(tx: &Transaction, prevouts: &[Option<&TxOut>]) {
/// let mut cache = VerificationCache::new(tx, prevouts);
/// for vin in 0..tx.input.len() {
///     let verification = verify_input(&mut cache, vin, Flags::CONSENSUS);
///     assert!(verification.result.is_ok());
/// }
/// # }
/// ```
///
/// # Panics
///
/// If `input_index` is out of range for the cache's transaction.
pub fn verify_input(
    cache: &mut VerificationCache<'_>,
    input_index: usize,
    flags: Flags,
) -> Verification {
    let sighasher = &cache.sighasher;
    assert!(
        input_index < sighasher.tx().input.len(),
        "input index out of range"
    );
    sighasher.reset_dependencies();
    let tx = sighasher.tx();
    let result = if tx.is_coinbase() {
        Ok(())
    } else {
        match sighasher.prevout(input_index) {
            None => Err(Fail::Missing(vec![input_index])),
            Some(prevout) => interpreter::verify_script(sighasher, input_index, prevout, flags),
        }
    };
    Verification {
        result: result.map_err(Error::from),
        depends_on: if sighasher.read_all_prevouts() {
            DependsOn::AllPrevouts
        } else {
            DependsOn::OwnPrevout
        },
    }
}

/// Verifies every input with one cache, returning results in input order.
///
/// Equivalent to creating a [`VerificationCache`] and calling [`verify_input`] for each input.
/// Missing prevouts are represented by `None`; all inputs are checked even if an earlier one fails.
///
/// # Panics
///
/// If `prevouts.len() != tx.input.len()`.
pub fn verify_transaction(
    tx: &Transaction,
    prevouts: &[Option<&TxOut>],
    flags: Flags,
) -> Vec<Verification> {
    let mut cache = VerificationCache::new(tx, prevouts);
    (0..tx.input.len())
        .map(|vin| verify_input(&mut cache, vin, flags))
        .collect()
}
