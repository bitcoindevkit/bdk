// Bitcoin Dev Kit
// Written in 2023 by The Bitcoin Dev Kit Developers
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

use crate::collections::{HashMap, HashSet};
use alloc::{boxed::Box, vec::Vec};
use bdk_chain::ConfirmationTime;
use bitcoin::util::psbt::Input as PsbtInput;
use bitcoin::{OutPoint, Script, TxOut};
use miniscript::{Descriptor, DescriptorPublicKey};

// TODO: replace with the miniscript one
// Clone is useful for tests, idk if it's going to be here irl
#[derive(Clone)]
pub struct Asset {}

pub struct Plan {}

// TODO: the generic for the plan_id might be overkill here
pub enum Utxo<P> {
    Planned {
        outpoint: OutPoint,
        txout: TxOut,
        confirmation_time: ConfirmationTime,
        plan_id: Option<P>,
    },
    PsbtInput {
        outpoint: OutPoint,
        satisfaction_weight: u32,
        psbt_input: PsbtInput,
    },
}

#[derive(Default)]
pub struct CoinGroup<P> {
    utxos: HashMap<OutPoint, Utxo<P>>,
    unspendable: HashSet<OutPoint>,
}

// Step 1
pub struct CoinControl<G, P> {
    // group to utxos
    utxos: HashMap<G, CoinGroup<P>>,
    plans: HashMap<P, Plan>,
    #[allow(clippy::type_complexity)]
    grouping_fn: Box<dyn Fn(&Utxo<P>) -> G>,
}

impl<P> Default for CoinControl<Script, P> {
    fn default() -> Self {
        CoinControl {
            grouping_fn: Box::new(|utxo| {
                match utxo {
                    Utxo::Planned { txout, .. } => txout.script_pubkey.clone(),
                    Utxo::PsbtInput {
                        psbt_input,
                        outpoint,
                        ..
                    } => {
                        match (&psbt_input.witness_utxo, &psbt_input.non_witness_utxo) {
                            (Some(txout), _) => txout.script_pubkey.clone(),
                            (_, Some(tx)) => {
                                tx.output[outpoint.vout as usize].script_pubkey.clone()
                            }
                            (None, None) => {
                                // The user didn't give us enough info for us to figure out
                                // the spk
                                // We should add this coin to a fake group composed of only
                                // this coin
                                // I can probably do this with some weird hack, creating a fake
                                // spk from the hash of the outpoint, but honestly I don't love it
                                // and I'm looking for better solutions
                                todo!();
                            }
                        }
                    }
                }
            }),
            utxos: HashMap::default(),
            plans: HashMap::default(),
        }
    }
}

impl<G: core::hash::Hash + core::cmp::Eq + PartialEq> CoinControl<G, usize> {
    pub fn add_planned_utxos(
        &mut self,
        utxos: impl IntoIterator<Item = (OutPoint, TxOut, ConfirmationTime)>,
        _desc: Descriptor<DescriptorPublicKey>,
        _assets: impl IntoIterator<Item = Asset>,
        unspendable: HashSet<OutPoint>,
    ) {
        let plan = Plan {};
        // TODO: if you can't obtain the plan, insert all into the unspendable list
        let plan_id = self.plans.len();
        self.plans.insert(plan_id, plan);
        for (outpoint, txout, confirmation_time) in utxos {
            let utxo = Utxo::Planned {
                outpoint,
                txout,
                confirmation_time,
                plan_id: Some(plan_id),
            };
            let group_id = (self.grouping_fn)(&utxo);
            let group = &mut self.utxos.entry(group_id).or_insert(CoinGroup::default());
            group.utxos.insert(outpoint, utxo);
            if unspendable.contains(&outpoint) {
                group.unspendable.insert(outpoint);
            }
        }
    }

    pub fn add_psbt_inputs(
        &mut self,
        utxos: impl IntoIterator<Item = (OutPoint, u32, PsbtInput)>,
        unspendable: HashSet<OutPoint>,
    ) {
        for (outpoint, satisfaction_weight, psbt_input) in utxos {
            let utxo = Utxo::PsbtInput {
                outpoint,
                satisfaction_weight,
                psbt_input,
            };
            let group_id = (self.grouping_fn)(&utxo);
            let group = &mut self.utxos.entry(group_id).or_insert(CoinGroup::default());
            group.utxos.insert(outpoint, utxo);
            if unspendable.contains(&outpoint) {
                group.unspendable.insert(outpoint);
            }
        }
    }

    pub fn as_candidates(&self, _include_partial_groups: bool) -> Vec<UtxoGroupCandidate<G>> {
        todo!()
    }
}

pub struct UtxoGroupCandidate<G> {
    pub group_id: G,
    pub cs_candidate: Candidate,
}

impl<G> AsRef<Candidate> for UtxoGroupCandidate<G> {
    fn as_ref(&self) -> &Candidate {
        &self.cs_candidate
    }
}

/// A `Candidate` represents an input candidate for `CoinSelector`. This can either be a
/// single UTXO, or a group of UTXOs that should be spent together.
#[derive(Debug, Clone, Copy)]
pub struct Candidate {
    /// Total value of the UTXO(s) that this [`Candidate`] represents.
    pub value: u64,
    /// Total weight of including this/these UTXO(s).
    /// `txin` fields: `prevout`, `nSequence`, `scriptSigLen`, `scriptSig`, `scriptWitnessLen`,
    /// `scriptWitness` should all be included.
    pub weight: u32,
    /// Total number of inputs; so we can calculate extra `varint` weight due to `vin` len changes.
    pub input_count: usize,
    /// Whether this [`Candidate`] contains at least one segwit spend.
    pub is_segwit: bool,
}

impl AsRef<Candidate> for Candidate {
    fn as_ref(&self) -> &Candidate {
        self
    }
}

// This is step 0 which will probably be reworked 10 more times, so I'm commenting it
/*
pub struct TxParams<K> {
    pub recipients: Vec<(Script, AmountOrDrain)>,
    pub fee_policy: FeePolicy,
    pub change_keychain: K,
    pub sighash: psbt::PsbtSighashType,
    pub locktime: LockTime,
    pub rbf: RbfValue,
    pub version: Version,
    pub current_height: Option<u32>,
}

pub enum AmountOrDrain {
    Amount(Amount),
    // TODO: maybe drain could have a percentage, so you can drain 60%
    // to A and 40% to B.
    // At the moment if you put multiple drains the amount will be split
    // equally between the drain recipients.
    Drain,
}

/// Transaction version
///
/// Has a default value of `1`
#[derive(Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Clone, Copy)]
pub struct Version(i32);

impl Default for Version {
    fn default() -> Self {
        Version(1)
    }
}

/// RBF nSequence value
///
/// Has a default value of `0xFFFFFFFD`
#[derive(Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Clone, Copy)]
pub enum RbfValue {
    Default,
    Value(Sequence),
}

impl RbfValue {
    pub(crate) fn get_value(&self) -> Sequence {
        match self {
            RbfValue::Default => Sequence::ENABLE_RBF_NO_LOCKTIME,
            RbfValue::Value(v) => *v,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum FeePolicy {
    FeeRate(FeeRate),
    FeeAmount(u64),
}
*/
