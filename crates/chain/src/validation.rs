//! Script validation of transaction inputs.
//!
//! Validation exists to keep a provably invalid unconfirmed transaction out of the canonical set:
//! one a chain source injects to spend a wallet's on-chain outputs, so that they look spent or the
//! wallet replaces against a transaction that can never confirm. Canonicalization prefers a Valid
//! whole copy over Unknown and rejects a txid only when every retained copy is
//! [`TxValidity::Invalid`]. Valid and Unknown are both eligible, so nothing has to be proven valid.
//!
//! [`TxGraph::validate_tx`] checks each input of a transaction, in the graph or not, under today's
//! consensus script rules, the ones an unconfirmed transaction must meet. Each spent output comes
//! from the graph's full transaction when it has one, and otherwise from its floating txout. A
//! floating txout is the chain source's word, not proof, so each verdict records whether it relied
//! on one.
//!
//! Canonicalization runs the same check on each transaction that would be canonical only by being
//! seen. A txid with only Invalid copies, or one spending from it, is never picked that way. A
//! transaction confirmed in the best chain, or assumed canonical, is picked whatever its scripts,
//! since an anchor's trust is a matter of proof of work. Nothing is stored between
//! canonicalizations.
//!
//! See `docs/input-validation.md` in the repository for the model.
//!
//! [`TxGraph::validate_tx`]: crate::TxGraph::validate_tx

use alloc::vec::Vec;
use bitcoin::OutPoint;

pub use bdk_script_verify::ScriptError;

/// The script validity of a whole transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TxValidity {
    /// Every input's scripts passed.
    Valid,
    /// No input failed, but some could not be checked because the graph has neither a full
    /// transaction nor a floating txout for outputs they depend on.
    Unknown,
    /// At least one input's scripts failed.
    Invalid,
}

/// The script validity of one input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputValidity {
    /// The input's scripts passed.
    Valid {
        /// Whether the check read a floating txout rather than a full transaction's output.
        relies_on_floating: bool,
    },
    /// The input's scripts failed against the spent outputs the graph has. It is conclusive only
    /// as far as those outputs are: a floating txout is the chain source's word.
    Invalid {
        /// Why the scripts failed.
        error: ScriptError,
        /// Whether the check read a floating txout rather than a full transaction's output.
        relies_on_floating: bool,
    },
    /// The input cannot be checked until the graph has these spent outputs, as a full transaction
    /// or a floating txout.
    Unknown {
        /// The spent outputs the graph has no data for.
        ///
        /// This is what the check stopped on, not necessarily everything it needs: a taproot
        /// input missing its own prevout lists only that, and may then need the others.
        missing: Vec<OutPoint>,
    },
}

/// The result of [`TxGraph::validate_tx`](crate::TxGraph::validate_tx).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxValidation {
    /// The validity of each input, in input order.
    pub inputs: Vec<InputValidity>,
}

impl TxValidation {
    /// The validity of the whole transaction: [`TxValidity::Invalid`] if any input is invalid,
    /// otherwise [`TxValidity::Unknown`] if any input is unknown.
    pub fn validity(&self) -> TxValidity {
        let mut validity = TxValidity::Valid;
        for input in &self.inputs {
            match input {
                InputValidity::Invalid { .. } => return TxValidity::Invalid,
                InputValidity::Unknown { .. } => validity = TxValidity::Unknown,
                InputValidity::Valid { .. } => {}
            }
        }
        validity
    }

    /// Whether any input's Valid or Invalid verdict relied on a floating txout. An Unknown input
    /// has no verdict, so it relies on nothing.
    pub fn relies_on_floating(&self) -> bool {
        self.inputs.iter().any(|input| match input {
            InputValidity::Valid { relies_on_floating }
            | InputValidity::Invalid {
                relies_on_floating, ..
            } => *relies_on_floating,
            InputValidity::Unknown { .. } => false,
        })
    }
}
