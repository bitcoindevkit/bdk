// BIP143 adapted from `bip143_signature_hash` in rbitcoin
// `crates/rbitcoin-consensus/src/script/mod.rs` at 49210ea82130474a4df366e253d1346a16369fdb (MIT OR
// Apache-2.0, rbitcoin contributors).

use alloc::vec::Vec;
use core::cell::{Cell, OnceCell, RefCell};

use bitcoin::consensus::Encodable;
use bitcoin::hashes::{sha256d, Hash, HashEngine};
use bitcoin::sighash::{Annex, EcdsaSighashType, Prevouts, SighashCache, TapSighashType};
use bitcoin::{Amount, Script, TapLeafHash, Transaction, TxOut};

use crate::Fail;

/// Signature hashing for the inputs of one transaction, plus a record of which spent outputs the
/// hashes committed to.
pub struct Sighasher<'a> {
    tx: &'a Transaction,
    prevouts: &'a [Option<&'a TxOut>],
    cache: RefCell<SighashCache<&'a Transaction>>,
    bip143: OnceCell<Bip143Midstate>,
    all_prevouts: OnceCell<Result<Vec<&'a TxOut>, Vec<usize>>>,
    read_all_prevouts: Cell<bool>,
}

struct Bip143Midstate {
    prevouts: [u8; 32],
    sequences: [u8; 32],
    outputs: [u8; 32],
}

impl<'a> Sighasher<'a> {
    pub fn new(tx: &'a Transaction, prevouts: &'a [Option<&'a TxOut>]) -> Self {
        #[cfg(test)]
        work::record(|counts| counts.contexts += 1);
        Self {
            tx,
            prevouts,
            cache: RefCell::new(SighashCache::new(tx)),
            bip143: OnceCell::new(),
            all_prevouts: OnceCell::new(),
            read_all_prevouts: Cell::new(false),
        }
    }

    pub fn tx(&self) -> &'a Transaction {
        self.tx
    }

    pub fn prevout(&self, input_index: usize) -> Option<&'a TxOut> {
        self.prevouts[input_index]
    }

    pub fn read_all_prevouts(&self) -> bool {
        self.read_all_prevouts.get()
    }

    pub fn reset_dependencies(&self) {
        self.read_all_prevouts.set(false);
    }

    /// `script_code` must already have its `OP_CODESEPARATOR`s and FindAndDelete matches removed.
    pub fn legacy(&self, input_index: usize, script_code: &[u8], raw_ty: u32) -> [u8; 32] {
        self.cache
            .borrow()
            .legacy_signature_hash(input_index, Script::from_bytes(script_code), raw_ty)
            .expect("input index is in range")
            .to_byte_array()
    }

    /// BIP143 with the raw `nHashType`.
    ///
    /// rust-bitcoin's segwit v0 sighash takes an [`EcdsaSighashType`], which serializes a
    /// normalized type. Consensus commits to the raw byte, and mainnet has non-standard ones (e.g.
    /// `0x65`).
    pub fn segwit_v0(
        &self,
        input_index: usize,
        script_code: &Script,
        amount: Amount,
        raw_ty: u32,
    ) -> [u8; 32] {
        use EcdsaSighashType::*;
        let tx = self.tx;
        let mapped = EcdsaSighashType::from_consensus(raw_ty);
        let anyone_can_pay = matches!(
            mapped,
            AllPlusAnyoneCanPay | NonePlusAnyoneCanPay | SinglePlusAnyoneCanPay
        );
        let base = match mapped {
            None | NonePlusAnyoneCanPay => None,
            Single | SinglePlusAnyoneCanPay => Single,
            _ => All,
        };
        let zero = [0u8; 32];
        let mid = self.bip143.get_or_init(|| {
            #[cfg(test)]
            work::record(|counts| counts.bip143 += 1);
            Bip143Midstate::new(tx)
        });

        let hash_prevouts = if anyone_can_pay { zero } else { mid.prevouts };
        let hash_sequence = if !anyone_can_pay && base == All {
            mid.sequences
        } else {
            zero
        };
        let hash_outputs = if base == All {
            mid.outputs
        } else if base == Single && input_index < tx.output.len() {
            let mut eng = sha256d::Hash::engine();
            tx.output[input_index]
                .consensus_encode(&mut eng)
                .expect("hash engines do not error");
            sha256d::Hash::from_engine(eng).to_byte_array()
        } else {
            zero
        };

        let txin = &tx.input[input_index];
        let mut eng = sha256d::Hash::engine();
        tx.version
            .consensus_encode(&mut eng)
            .expect("hash engines do not error");
        eng.input(&hash_prevouts);
        eng.input(&hash_sequence);
        txin.previous_output
            .consensus_encode(&mut eng)
            .expect("hash engines do not error");
        script_code
            .consensus_encode(&mut eng)
            .expect("hash engines do not error");
        amount
            .consensus_encode(&mut eng)
            .expect("hash engines do not error");
        txin.sequence
            .consensus_encode(&mut eng)
            .expect("hash engines do not error");
        eng.input(&hash_outputs);
        tx.lock_time
            .consensus_encode(&mut eng)
            .expect("hash engines do not error");
        eng.input(&raw_ty.to_le_bytes());
        sha256d::Hash::from_engine(eng).to_byte_array()
    }

    /// BIP341 signature hash. `leaf` is `None` on the key path.
    ///
    /// Unless the type is ANYONECANPAY the hash commits to every input's spent output, so a missing
    /// one leaves the result undetermined rather than invalid.
    pub fn taproot(
        &self,
        input_index: usize,
        annex: Option<&[u8]>,
        leaf: Option<(TapLeafHash, u32)>,
        ty: TapSighashType,
    ) -> Result<[u8; 32], Fail> {
        use TapSighashType::*;
        let annex = annex.map(|a| Annex::new(a).expect("caller checked the 0x50 tag"));
        let anyone_can_pay = matches!(
            ty,
            AllPlusAnyoneCanPay | NonePlusAnyoneCanPay | SinglePlusAnyoneCanPay
        );
        let mut cache = self.cache.borrow_mut();
        let hash = if anyone_can_pay {
            let own =
                self.prevouts[input_index].expect("own prevout is checked before scripts run");
            cache.taproot_signature_hash(
                input_index,
                &Prevouts::One(input_index, own),
                annex,
                leaf,
                ty,
            )
        } else {
            self.read_all_prevouts.set(true);
            let all = self.all_prevouts.get_or_init(|| {
                #[cfg(test)]
                work::record(|counts| counts.taproot_prevouts += 1);
                let missing = self
                    .prevouts
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.is_none())
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>();
                if missing.is_empty() {
                    Ok(self.prevouts.iter().flatten().copied().collect())
                } else {
                    Err(missing)
                }
            });
            let all = all
                .as_ref()
                .map_err(|missing| Fail::Missing(missing.clone()))?;
            cache.taproot_signature_hash(input_index, &Prevouts::All(all), annex, leaf, ty)
        };
        // The only reachable error is SIGHASH_SINGLE without a matching output, which Core reports
        // as a bad hashtype.
        hash.map(|h| h.to_byte_array())
            .map_err(|_| Fail::Script("SCHNORR_SIG_HASHTYPE"))
    }
}

impl Bip143Midstate {
    fn new(tx: &Transaction) -> Self {
        let mut prevouts = sha256d::Hash::engine();
        let mut sequences = sha256d::Hash::engine();
        for txin in &tx.input {
            txin.previous_output
                .consensus_encode(&mut prevouts)
                .expect("hash engines do not error");
            txin.sequence
                .consensus_encode(&mut sequences)
                .expect("hash engines do not error");
        }
        let mut outputs = sha256d::Hash::engine();
        for txout in &tx.output {
            txout
                .consensus_encode(&mut outputs)
                .expect("hash engines do not error");
        }
        Self {
            prevouts: sha256d::Hash::from_engine(prevouts).to_byte_array(),
            sequences: sha256d::Hash::from_engine(sequences).to_byte_array(),
            outputs: sha256d::Hash::from_engine(outputs).to_byte_array(),
        }
    }
}

#[cfg(test)]
pub(crate) mod work {
    use core::cell::Cell;

    #[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
    pub struct Counts {
        pub contexts: usize,
        pub bip143: usize,
        pub taproot_prevouts: usize,
    }

    std::thread_local! {
        static COUNTS: Cell<Counts> = Cell::new(Counts::default());
    }

    pub fn record(update: impl FnOnce(&mut Counts)) {
        COUNTS.with(|cell| {
            let mut counts = cell.get();
            update(&mut counts);
            cell.set(counts);
        });
    }

    pub fn take() -> Counts {
        COUNTS.with(|cell| cell.replace(Counts::default()))
    }
}
