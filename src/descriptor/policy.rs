// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

//! Descriptor policy
//!
//! This module implements the logic to extract and represent the spending policies of a descriptor
//! in a more human-readable format.
//!
//! This is an **EXPERIMENTAL** feature, API and other major changes are expected.
//!
//! ## Example
//!
//! ```
//! # use std::sync::Arc;
//! # use bdk::descriptor::*;
//! # use bdk::wallet::signer::*;
//! # use bdk::bitcoin::secp256k1::Secp256k1;
//! use bdk::descriptor::policy::BuildSatisfaction;
//! let secp = Secp256k1::new();
//! let desc = "wsh(and_v(v:pk(cV3oCth6zxZ1UVsHLnGothsWNsaoxRhC6aeNi5VbSdFpwUkgkEci),or_d(pk(cVMTy7uebJgvFaSBwcgvwk8qn8xSLc97dKow4MBetjrrahZoimm2),older(12960))))";
//!
//! let (extended_desc, key_map) = ExtendedDescriptor::parse_descriptor(&secp, desc)?;
//! println!("{:?}", extended_desc);
//!
//! let signers = Arc::new(SignersContainer::build(key_map, &extended_desc, &secp));
//! let policy = extended_desc.extract_policy(&signers, BuildSatisfaction::None, &secp)?;
//! println!("policy: {}", serde_json::to_string(&policy)?);
//! # Ok::<(), bdk::Error>(())
//! ```

use std::cmp::max;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fmt;

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use bitcoin::hashes::{hash160, ripemd160, sha256};
use bitcoin::util::bip32::Fingerprint;
use bitcoin::{LockTime, PublicKey, Sequence, XOnlyPublicKey};

use miniscript::descriptor::{
    DescriptorPublicKey, ShInner, SinglePub, SinglePubKey, SortedMultiVec, WshInner,
};
use miniscript::hash256;
use miniscript::{
    Descriptor, Miniscript, Satisfier, ScriptContext, SigType, Terminal, ToPublicKey,
};

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use crate::descriptor::ExtractPolicy;
use crate::keys::ExtScriptContext;
use crate::wallet::signer::{SignerId, SignersContainer};
use crate::wallet::utils::{After, Older, SecpCtx};

use super::checksum::calc_checksum;
use super::error::Error;
use super::XKeyUtils;
use bitcoin::util::psbt::{Input as PsbtInput, PartiallySignedTransaction as Psbt};
use miniscript::psbt::PsbtInputSatisfier;

/// A unique identifier for a key
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PkOrF {
    /// A legacy public key
    Pubkey(PublicKey),
    /// A x-only public key
    XOnlyPubkey(XOnlyPublicKey),
    /// An extended key fingerprint
    Fingerprint(Fingerprint),
}

impl PkOrF {
    fn from_key(k: &DescriptorPublicKey, secp: &SecpCtx) -> Self {
        match k {
            DescriptorPublicKey::Single(SinglePub {
                key: SinglePubKey::FullKey(pk),
                ..
            }) => PkOrF::Pubkey(*pk),
            DescriptorPublicKey::Single(SinglePub {
                key: SinglePubKey::XOnly(pk),
                ..
            }) => PkOrF::XOnlyPubkey(*pk),
            DescriptorPublicKey::XPub(xpub) => PkOrF::Fingerprint(xpub.root_fingerprint(secp)),
        }
    }
}

/// An item that needs to be satisfied
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "UPPERCASE")]
pub enum SatisfiableItem {
    // Leaves
    /// ECDSA Signature for a raw public key
    EcdsaSignature(PkOrF),
    /// Schnorr Signature for a raw public key
    SchnorrSignature(PkOrF),
    /// SHA256 preimage hash
    Sha256Preimage {
        /// The digest value
        hash: sha256::Hash,
    },
    /// Double SHA256 preimage hash
    Hash256Preimage {
        /// The digest value
        hash: hash256::Hash,
    },
    /// RIPEMD160 preimage hash
    Ripemd160Preimage {
        /// The digest value
        hash: ripemd160::Hash,
    },
    /// SHA256 then RIPEMD160 preimage hash
    Hash160Preimage {
        /// The digest value
        hash: hash160::Hash,
    },
    /// Absolute timeclock timestamp
    AbsoluteTimelock {
        /// The timelock value
        value: LockTime,
    },
    /// Relative timelock locktime
    RelativeTimelock {
        /// The timelock value
        value: Sequence,
    },
    /// Multi-signature public keys with threshold count
    Multisig {
        /// The raw public key or extended key fingerprint
        keys: Vec<PkOrF>,
        /// The required threshold count
        threshold: usize,
    },

    // Complex item
    /// Threshold items with threshold count
    Thresh {
        /// The policy items
        items: Vec<Policy>,
        /// The required threshold count
        threshold: usize,
    },
}

impl SatisfiableItem {
    /// Returns whether the [`SatisfiableItem`] is a leaf item
    pub fn is_leaf(&self) -> bool {
        !matches!(
            self,
            SatisfiableItem::Thresh {
                items: _,
                threshold: _,
            }
        )
    }

    /// Returns a unique id for the [`SatisfiableItem`]
    pub fn id(&self) -> String {
        calc_checksum(&serde_json::to_string(self).expect("Failed to serialize a SatisfiableItem"))
            .expect("Failed to compute a SatisfiableItem id")
    }
}

fn combinations(vec: &[usize], size: usize) -> Vec<Vec<usize>> {
    assert!(vec.len() >= size);

    let mut answer = Vec::new();

    let mut queue = VecDeque::new();
    for (index, val) in vec.iter().enumerate() {
        let mut new_vec = Vec::with_capacity(size);
        new_vec.push(*val);
        queue.push_back((index, new_vec));
    }

    while let Some((index, vals)) = queue.pop_front() {
        if vals.len() >= size {
            answer.push(vals);
        } else {
            for (new_index, val) in vec.iter().skip(index + 1).enumerate() {
                let mut cloned = vals.clone();
                cloned.push(*val);
                queue.push_front((new_index, cloned));
            }
        }
    }

    answer
}

fn mix<T: Clone>(vec: Vec<Vec<T>>) -> Vec<Vec<T>> {
    if vec.is_empty() || vec.iter().any(Vec::is_empty) {
        return vec![];
    }

    let mut answer = Vec::new();
    let size = vec.len();

    let mut queue = VecDeque::new();
    for i in &vec[0] {
        let mut new_vec = Vec::with_capacity(size);
        new_vec.push(i.clone());
        queue.push_back(new_vec);
    }

    while let Some(vals) = queue.pop_front() {
        if vals.len() >= size {
            answer.push(vals);
        } else {
            let level = vals.len();
            for i in &vec[level] {
                let mut cloned = vals.clone();
                cloned.push(i.clone());
                queue.push_front(cloned);
            }
        }
    }

    answer
}

/// Type for a map of sets of [`Condition`] items keyed by each set's index
pub type ConditionMap = BTreeMap<usize, HashSet<Condition>>;
/// Type for a map of folded sets of [`Condition`] items keyed by a vector of the combined set's indexes
pub type FoldedConditionMap = BTreeMap<Vec<usize>, HashSet<Condition>>;

fn serialize_folded_cond_map<S>(
    input_map: &FoldedConditionMap,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut map = serializer.serialize_map(Some(input_map.len()))?;
    for (k, v) in input_map {
        let k_string = format!("{:?}", k);
        map.serialize_entry(&k_string, v)?;
    }
    map.end()
}

/// Represent if and how much a policy item is satisfied by the wallet's descriptor
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "UPPERCASE")]
pub enum Satisfaction {
    /// Only a partial satisfaction of some kind of threshold policy
    Partial {
        /// Total number of items
        n: usize,
        /// Threshold
        m: usize,
        /// The items that can be satisfied by the descriptor or are satisfied in the PSBT
        items: Vec<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// Whether the items are sorted in lexicographic order (used by `sortedmulti`)
        sorted: Option<bool>,
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        /// Extra conditions that also need to be satisfied
        conditions: ConditionMap,
    },
    /// Can reach the threshold of some kind of threshold policy
    PartialComplete {
        /// Total number of items
        n: usize,
        /// Threshold
        m: usize,
        /// The items that can be satisfied by the descriptor
        items: Vec<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// Whether the items are sorted in lexicographic order (used by `sortedmulti`)
        sorted: Option<bool>,
        #[serde(
            serialize_with = "serialize_folded_cond_map",
            skip_serializing_if = "BTreeMap::is_empty"
        )]
        /// Extra conditions that also need to be satisfied
        conditions: FoldedConditionMap,
    },

    /// Can satisfy the policy item
    Complete {
        /// Extra conditions that also need to be satisfied
        condition: Condition,
    },
    /// Cannot satisfy or contribute to the policy item
    None,
}

impl Satisfaction {
    /// Returns whether the [`Satisfaction`] is a leaf item
    pub fn is_leaf(&self) -> bool {
        match self {
            Satisfaction::None | Satisfaction::Complete { .. } => true,
            Satisfaction::PartialComplete { .. } | Satisfaction::Partial { .. } => false,
        }
    }

    // add `inner` as one of self's partial items. this only makes sense on partials
    fn add(&mut self, inner: &Satisfaction, inner_index: usize) -> Result<(), PolicyError> {
        match self {
            Satisfaction::None | Satisfaction::Complete { .. } => Err(PolicyError::AddOnLeaf),
            Satisfaction::PartialComplete { .. } => Err(PolicyError::AddOnPartialComplete),
            Satisfaction::Partial {
                n,
                ref mut conditions,
                ref mut items,
                ..
            } => {
                if inner_index >= *n || items.contains(&inner_index) {
                    return Err(PolicyError::IndexOutOfRange(inner_index));
                }

                match inner {
                    // not relevant if not completed yet
                    Satisfaction::None | Satisfaction::Partial { .. } => return Ok(()),
                    Satisfaction::Complete { condition } => {
                        items.push(inner_index);
                        conditions.insert(inner_index, vec![*condition].into_iter().collect());
                    }
                    Satisfaction::PartialComplete {
                        conditions: other_conditions,
                        ..
                    } => {
                        items.push(inner_index);
                        let conditions_set = other_conditions
                            .values()
                            .fold(HashSet::new(), |set, i| set.union(i).cloned().collect());
                        conditions.insert(inner_index, conditions_set);
                    }
                }

                Ok(())
            }
        }
    }

    fn finalize(&mut self) {
        // if partial try to bump it to a partialcomplete
        if let Satisfaction::Partial {
            n,
            m,
            items,
            conditions,
            sorted,
        } = self
        {
            if items.len() >= *m {
                let mut map = BTreeMap::new();
                let indexes = combinations(items, *m);
                // `indexes` at this point is a Vec<Vec<usize>>, with the "n choose k" of items (m of n)
                indexes
                    .into_iter()
                    // .inspect(|x| println!("--- orig --- {:?}", x))
                    // we map each of the combinations of elements into a tuple of ([chosen items], [conditions]). unfortunately, those items have potentially more than one
                    // condition (think about ORs), so we also use `mix` to expand those, i.e. [[0], [1, 2]] becomes [[0, 1], [0, 2]]. This is necessary to make sure that we
                    // consider every possible options and check whether or not they are compatible.
                    // since this step can turn one item of the iterator into multiple ones, we use `flat_map()` to expand them out
                    .flat_map(|i_vec| {
                        mix(i_vec
                            .iter()
                            .map(|i| {
                                conditions
                                    .get(i)
                                    .map(|set| set.clone().into_iter().collect())
                                    .unwrap_or_default()
                            })
                            .collect())
                        .into_iter()
                        .map(|x| (i_vec.clone(), x))
                        .collect::<Vec<(Vec<usize>, Vec<Condition>)>>()
                    })
                    // .inspect(|x| println!("flat {:?}", x))
                    // try to fold all the conditions for this specific combination of indexes/options. if they are not compatible, try_fold will be Err
                    .map(|(key, val)| {
                        (
                            key,
                            val.into_iter()
                                .try_fold(Condition::default(), |acc, v| acc.merge(&v)),
                        )
                    })
                    // .inspect(|x| println!("try_fold {:?}", x))
                    // filter out all the incompatible combinations
                    .filter(|(_, val)| val.is_ok())
                    // .inspect(|x| println!("filter {:?}", x))
                    // push them into the map
                    .for_each(|(key, val)| {
                        map.entry(key)
                            .or_insert_with(HashSet::new)
                            .insert(val.unwrap());
                    });
                // TODO: if the map is empty, the conditions are not compatible, return an error?
                *self = Satisfaction::PartialComplete {
                    n: *n,
                    m: *m,
                    items: items.clone(),
                    conditions: map,
                    sorted: *sorted,
                };
            }
        }
    }
}

impl From<bool> for Satisfaction {
    fn from(other: bool) -> Self {
        if other {
            Satisfaction::Complete {
                condition: Default::default(),
            }
        } else {
            Satisfaction::None
        }
    }
}

/// Descriptor spending policy
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Policy {
    /// Identifier for this policy node
    pub id: String,

    /// Type of this policy node
    #[serde(flatten)]
    pub item: SatisfiableItem,
    /// How much a given PSBT already satisfies this policy node in terms of signatures
    pub satisfaction: Satisfaction,
    /// How the wallet's descriptor can satisfy this policy node
    pub contribution: Satisfaction,
}

/// An extra condition that must be satisfied but that is out of control of the user
/// TODO: use `bitcoin::LockTime` and `bitcoin::Sequence`
#[derive(Hash, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Default, Serialize)]
pub struct Condition {
    /// Optional CheckSequenceVerify condition
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csv: Option<Sequence>,
    /// Optional timelock condition
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timelock: Option<LockTime>,
}

impl Condition {
    fn merge_nlocktime(a: LockTime, b: LockTime) -> Result<LockTime, PolicyError> {
        if !a.is_same_unit(b) {
            Err(PolicyError::MixedTimelockUnits)
        } else if a > b {
            Ok(a)
        } else {
            Ok(b)
        }
    }

    fn merge_nsequence(a: Sequence, b: Sequence) -> Result<Sequence, PolicyError> {
        if a.is_time_locked() != b.is_time_locked() {
            Err(PolicyError::MixedTimelockUnits)
        } else {
            Ok(max(a, b))
        }
    }

    pub(crate) fn merge(mut self, other: &Condition) -> Result<Self, PolicyError> {
        match (self.csv, other.csv) {
            (Some(a), Some(b)) => self.csv = Some(Self::merge_nsequence(a, b)?),
            (None, any) => self.csv = any,
            _ => {}
        }

        match (self.timelock, other.timelock) {
            (Some(a), Some(b)) => self.timelock = Some(Self::merge_nlocktime(a, b)?),
            (None, any) => self.timelock = any,
            _ => {}
        }

        Ok(self)
    }

    /// Returns `true` if there are no extra conditions to verify
    pub fn is_null(&self) -> bool {
        self.csv.is_none() && self.timelock.is_none()
    }
}

/// Errors that can happen while extracting and manipulating policies
#[derive(Debug, PartialEq, Eq)]
pub enum PolicyError {
    /// Not enough items are selected to satisfy a [`SatisfiableItem::Thresh`] or a [`SatisfiableItem::Multisig`]
    NotEnoughItemsSelected(String),
    /// Index out of range for an item to satisfy a [`SatisfiableItem::Thresh`] or a [`SatisfiableItem::Multisig`]
    IndexOutOfRange(usize),
    /// Can not add to an item that is [`Satisfaction::None`] or [`Satisfaction::Complete`]
    AddOnLeaf,
    /// Can not add to an item that is [`Satisfaction::PartialComplete`]
    AddOnPartialComplete,
    /// Can not merge CSV or timelock values unless both are less than or both are equal or greater than 500_000_000
    MixedTimelockUnits,
    /// Incompatible conditions (not currently used)
    IncompatibleConditions,
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for PolicyError {}

impl Policy {
    fn new(item: SatisfiableItem) -> Self {
        Policy {
            id: item.id(),
            item,
            satisfaction: Satisfaction::None,
            contribution: Satisfaction::None,
        }
    }

    fn make_and(a: Option<Policy>, b: Option<Policy>) -> Result<Option<Policy>, PolicyError> {
        match (a, b) {
            (None, None) => Ok(None),
            (Some(x), None) | (None, Some(x)) => Ok(Some(x)),
            (Some(a), Some(b)) => Self::make_thresh(vec![a, b], 2),
        }
    }

    fn make_or(a: Option<Policy>, b: Option<Policy>) -> Result<Option<Policy>, PolicyError> {
        match (a, b) {
            (None, None) => Ok(None),
            (Some(x), None) | (None, Some(x)) => Ok(Some(x)),
            (Some(a), Some(b)) => Self::make_thresh(vec![a, b], 1),
        }
    }

    fn make_thresh(items: Vec<Policy>, threshold: usize) -> Result<Option<Policy>, PolicyError> {
        if threshold == 0 {
            return Ok(None);
        }

        let mut contribution = Satisfaction::Partial {
            n: items.len(),
            m: threshold,
            items: vec![],
            conditions: Default::default(),
            sorted: None,
        };
        let mut satisfaction = contribution.clone();
        for (index, item) in items.iter().enumerate() {
            contribution.add(&item.contribution, index)?;
            satisfaction.add(&item.satisfaction, index)?;
        }

        contribution.finalize();
        satisfaction.finalize();

        let mut policy: Policy = SatisfiableItem::Thresh { items, threshold }.into();
        policy.contribution = contribution;
        policy.satisfaction = satisfaction;

        Ok(Some(policy))
    }

    fn make_multisig<Ctx: ScriptContext + 'static>(
        keys: &[DescriptorPublicKey],
        signers: &SignersContainer,
        build_sat: BuildSatisfaction,
        threshold: usize,
        sorted: bool,
        secp: &SecpCtx,
    ) -> Result<Option<Policy>, PolicyError> {
        if threshold == 0 {
            return Ok(None);
        }

        let parsed_keys = keys.iter().map(|k| PkOrF::from_key(k, secp)).collect();

        let mut contribution = Satisfaction::Partial {
            n: keys.len(),
            m: threshold,
            items: vec![],
            conditions: Default::default(),
            sorted: Some(sorted),
        };
        let mut satisfaction = contribution.clone();

        for (index, key) in keys.iter().enumerate() {
            if signers.find(signer_id(key, secp)).is_some() {
                contribution.add(
                    &Satisfaction::Complete {
                        condition: Default::default(),
                    },
                    index,
                )?;
            }

            if let Some(psbt) = build_sat.psbt() {
                if Ctx::find_signature(psbt, key, secp) {
                    satisfaction.add(
                        &Satisfaction::Complete {
                            condition: Default::default(),
                        },
                        index,
                    )?;
                }
            }
        }
        satisfaction.finalize();
        contribution.finalize();

        let mut policy: Policy = SatisfiableItem::Multisig {
            keys: parsed_keys,
            threshold,
        }
        .into();
        policy.contribution = contribution;
        policy.satisfaction = satisfaction;

        Ok(Some(policy))
    }

    /// Return whether or not a specific path in the policy tree is required to unambiguously
    /// create a transaction
    ///
    /// What this means is that for some spending policies the user should select which paths in
    /// the tree it intends to satisfy while signing, because the transaction must be created differently based
    /// on that.
    pub fn requires_path(&self) -> bool {
        self.get_condition(&BTreeMap::new()).is_err()
    }

    /// Return the conditions that are set by the spending policy for a given path in the
    /// policy tree
    pub fn get_condition(
        &self,
        path: &BTreeMap<String, Vec<usize>>,
    ) -> Result<Condition, PolicyError> {
        // if items.len() == threshold, selected can be omitted and we take all of them by default
        let default = match &self.item {
            SatisfiableItem::Thresh { items, threshold } if items.len() == *threshold => {
                (0..*threshold).collect()
            }
            SatisfiableItem::Multisig { keys, .. } => (0..keys.len()).collect(),
            _ => vec![],
        };
        let selected = match path.get(&self.id) {
            Some(arr) => arr,
            _ => &default,
        };

        match &self.item {
            SatisfiableItem::Thresh { items, threshold } => {
                let mapped_req = items
                    .iter()
                    .map(|i| i.get_condition(path))
                    .collect::<Result<Vec<_>, _>>()?;

                // if all the requirements are null we don't care about `selected` because there
                // are no requirements
                if mapped_req.iter().all(Condition::is_null) {
                    return Ok(Condition::default());
                }

                // if we have something, make sure we have enough items. note that the user can set
                // an empty value for this step in case of n-of-n, because `selected` is set to all
                // the elements above
                if selected.len() < *threshold {
                    return Err(PolicyError::NotEnoughItemsSelected(self.id.clone()));
                }

                // check the selected items, see if there are conflicting requirements
                let mut requirements = Condition::default();
                for item_index in selected {
                    requirements = requirements.merge(
                        mapped_req
                            .get(*item_index)
                            .ok_or(PolicyError::IndexOutOfRange(*item_index))?,
                    )?;
                }

                Ok(requirements)
            }
            SatisfiableItem::Multisig { keys, threshold } => {
                if selected.len() < *threshold {
                    return Err(PolicyError::NotEnoughItemsSelected(self.id.clone()));
                }
                if let Some(item) = selected.iter().find(|i| **i >= keys.len()) {
                    return Err(PolicyError::IndexOutOfRange(*item));
                }

                Ok(Condition::default())
            }
            SatisfiableItem::AbsoluteTimelock { value } => Ok(Condition {
                csv: None,
                timelock: Some(*value),
            }),
            SatisfiableItem::RelativeTimelock { value } => Ok(Condition {
                csv: Some(*value),
                timelock: None,
            }),
            _ => Ok(Condition::default()),
        }
    }
}

impl From<SatisfiableItem> for Policy {
    fn from(other: SatisfiableItem) -> Self {
        Self::new(other)
    }
}

fn signer_id(key: &DescriptorPublicKey, secp: &SecpCtx) -> SignerId {
    // For consistency we always compute the key hash in "ecdsa" form (with the leading sign
    // prefix) even if we are in a taproot descriptor. We just want some kind of unique identifier
    // for a key, so it doesn't really matter how the identifier is computed.
    match key {
        DescriptorPublicKey::Single(SinglePub {
            key: SinglePubKey::FullKey(pk),
            ..
        }) => pk.to_pubkeyhash(SigType::Ecdsa).into(),
        DescriptorPublicKey::Single(SinglePub {
            key: SinglePubKey::XOnly(pk),
            ..
        }) => pk.to_pubkeyhash(SigType::Ecdsa).into(),
        DescriptorPublicKey::XPub(xpub) => xpub.root_fingerprint(secp).into(),
    }
}

fn make_generic_signature<M: Fn() -> SatisfiableItem, F: Fn(&Psbt) -> bool>(
    key: &DescriptorPublicKey,
    signers: &SignersContainer,
    build_sat: BuildSatisfaction,
    secp: &SecpCtx,
    make_policy: M,
    find_sig: F,
) -> Policy {
    let mut policy: Policy = make_policy().into();

    policy.contribution = if signers.find(signer_id(key, secp)).is_some() {
        Satisfaction::Complete {
            condition: Default::default(),
        }
    } else {
        Satisfaction::None
    };

    if let Some(psbt) = build_sat.psbt() {
        policy.satisfaction = if find_sig(psbt) {
            Satisfaction::Complete {
                condition: Default::default(),
            }
        } else {
            Satisfaction::None
        };
    }

    policy
}

fn generic_sig_in_psbt<
    // C is for "check", it's a closure we use to *check* if a psbt input contains the signature
    // for a specific key
    C: Fn(&PsbtInput, &SinglePubKey) -> bool,
    // E is for "extract", it extracts a key from the bip32 derivations found in the psbt input
    E: Fn(&PsbtInput, Fingerprint) -> Option<SinglePubKey>,
>(
    psbt: &Psbt,
    key: &DescriptorPublicKey,
    secp: &SecpCtx,
    check: C,
    extract: E,
) -> bool {
    //TODO check signature validity
    psbt.inputs.iter().all(|input| match key {
        DescriptorPublicKey::Single(SinglePub { key, .. }) => check(input, key),
        DescriptorPublicKey::XPub(xpub) => {
            //TODO check actual derivation matches
            match extract(input, xpub.root_fingerprint(secp)) {
                Some(pubkey) => check(input, &pubkey),
                None => false,
            }
        }
    })
}

trait SigExt: ScriptContext {
    fn make_signature(
        key: &DescriptorPublicKey,
        signers: &SignersContainer,
        build_sat: BuildSatisfaction,
        secp: &SecpCtx,
    ) -> Policy;

    fn find_signature(psbt: &Psbt, key: &DescriptorPublicKey, secp: &SecpCtx) -> bool;
}

impl<T: ScriptContext + 'static> SigExt for T {
    fn make_signature(
        key: &DescriptorPublicKey,
        signers: &SignersContainer,
        build_sat: BuildSatisfaction,
        secp: &SecpCtx,
    ) -> Policy {
        if T::as_enum().is_taproot() {
            make_generic_signature(
                key,
                signers,
                build_sat,
                secp,
                || SatisfiableItem::SchnorrSignature(PkOrF::from_key(key, secp)),
                |psbt| Self::find_signature(psbt, key, secp),
            )
        } else {
            make_generic_signature(
                key,
                signers,
                build_sat,
                secp,
                || SatisfiableItem::EcdsaSignature(PkOrF::from_key(key, secp)),
                |psbt| Self::find_signature(psbt, key, secp),
            )
        }
    }

    fn find_signature(psbt: &Psbt, key: &DescriptorPublicKey, secp: &SecpCtx) -> bool {
        if T::as_enum().is_taproot() {
            generic_sig_in_psbt(
                psbt,
                key,
                secp,
                |input, pk| {
                    let pk = match pk {
                        SinglePubKey::XOnly(pk) => pk,
                        _ => return false,
                    };

                    if input.tap_internal_key == Some(*pk) && input.tap_key_sig.is_some() {
                        true
                    } else {
                        input.tap_script_sigs.keys().any(|(sk, _)| sk == pk)
                    }
                },
                |input, fing| {
                    input
                        .tap_key_origins
                        .iter()
                        .find(|(_, (_, (f, _)))| f == &fing)
                        .map(|(pk, _)| SinglePubKey::XOnly(*pk))
                },
            )
        } else {
            generic_sig_in_psbt(
                psbt,
                key,
                secp,
                |input, pk| match pk {
                    SinglePubKey::FullKey(pk) => input.partial_sigs.contains_key(pk),
                    _ => false,
                },
                |input, fing| {
                    input
                        .bip32_derivation
                        .iter()
                        .find(|(_, (f, _))| f == &fing)
                        .map(|(pk, _)| SinglePubKey::FullKey(PublicKey::new(*pk)))
                },
            )
        }
    }
}

impl<Ctx: ScriptContext + 'static> ExtractPolicy for Miniscript<DescriptorPublicKey, Ctx> {
    fn extract_policy(
        &self,
        signers: &SignersContainer,
        build_sat: BuildSatisfaction,
        secp: &SecpCtx,
    ) -> Result<Option<Policy>, Error> {
        Ok(match &self.node {
            // Leaves
            Terminal::True | Terminal::False => None,
            Terminal::PkK(pubkey) => Some(Ctx::make_signature(pubkey, signers, build_sat, secp)),
            Terminal::PkH(pubkey_hash) => {
                Some(Ctx::make_signature(pubkey_hash, signers, build_sat, secp))
            }
            Terminal::After(value) => {
                let mut policy: Policy = SatisfiableItem::AbsoluteTimelock {
                    value: value.into(),
                }
                .into();
                policy.contribution = Satisfaction::Complete {
                    condition: Condition {
                        timelock: Some(value.into()),
                        csv: None,
                    },
                };
                if let BuildSatisfaction::PsbtTimelocks {
                    current_height,
                    psbt,
                    ..
                } = build_sat
                {
                    let after = After::new(Some(current_height), false);
                    let after_sat =
                        Satisfier::<bitcoin::PublicKey>::check_after(&after, value.into());
                    let inputs_sat = psbt_inputs_sat(psbt).all(|sat| {
                        Satisfier::<bitcoin::PublicKey>::check_after(&sat, value.into())
                    });
                    if after_sat && inputs_sat {
                        policy.satisfaction = policy.contribution.clone();
                    }
                }

                Some(policy)
            }
            Terminal::Older(value) => {
                let mut policy: Policy = SatisfiableItem::RelativeTimelock { value: *value }.into();
                policy.contribution = Satisfaction::Complete {
                    condition: Condition {
                        timelock: None,
                        csv: Some(*value),
                    },
                };
                if let BuildSatisfaction::PsbtTimelocks {
                    current_height,
                    input_max_height,
                    psbt,
                } = build_sat
                {
                    let older = Older::new(Some(current_height), Some(input_max_height), false);
                    let older_sat = Satisfier::<bitcoin::PublicKey>::check_older(&older, *value);
                    let inputs_sat = psbt_inputs_sat(psbt)
                        .all(|sat| Satisfier::<bitcoin::PublicKey>::check_older(&sat, *value));
                    if older_sat && inputs_sat {
                        policy.satisfaction = policy.contribution.clone();
                    }
                }

                Some(policy)
            }
            Terminal::Sha256(hash) => Some(SatisfiableItem::Sha256Preimage { hash: *hash }.into()),
            Terminal::Hash256(hash) => {
                Some(SatisfiableItem::Hash256Preimage { hash: *hash }.into())
            }
            Terminal::Ripemd160(hash) => {
                Some(SatisfiableItem::Ripemd160Preimage { hash: *hash }.into())
            }
            Terminal::Hash160(hash) => {
                Some(SatisfiableItem::Hash160Preimage { hash: *hash }.into())
            }
            Terminal::Multi(k, pks) | Terminal::MultiA(k, pks) => {
                Policy::make_multisig::<Ctx>(pks, signers, build_sat, *k, false, secp)?
            }
            // Identities
            Terminal::Alt(inner)
            | Terminal::Swap(inner)
            | Terminal::Check(inner)
            | Terminal::DupIf(inner)
            | Terminal::Verify(inner)
            | Terminal::NonZero(inner)
            | Terminal::ZeroNotEqual(inner) => inner.extract_policy(signers, build_sat, secp)?,
            // Complex policies
            Terminal::AndV(a, b) | Terminal::AndB(a, b) => Policy::make_and(
                a.extract_policy(signers, build_sat, secp)?,
                b.extract_policy(signers, build_sat, secp)?,
            )?,
            Terminal::AndOr(x, y, z) => Policy::make_or(
                Policy::make_and(
                    x.extract_policy(signers, build_sat, secp)?,
                    y.extract_policy(signers, build_sat, secp)?,
                )?,
                z.extract_policy(signers, build_sat, secp)?,
            )?,
            Terminal::OrB(a, b)
            | Terminal::OrD(a, b)
            | Terminal::OrC(a, b)
            | Terminal::OrI(a, b) => Policy::make_or(
                a.extract_policy(signers, build_sat, secp)?,
                b.extract_policy(signers, build_sat, secp)?,
            )?,
            Terminal::Thresh(k, nodes) => {
                let mut threshold = *k;
                let mapped: Vec<_> = nodes
                    .iter()
                    .map(|n| n.extract_policy(signers, build_sat, secp))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .flatten()
                    .collect();

                if mapped.len() < nodes.len() {
                    threshold = match threshold.checked_sub(nodes.len() - mapped.len()) {
                        None => return Ok(None),
                        Some(x) => x,
                    };
                }

                Policy::make_thresh(mapped, threshold)?
            }

            // Unsupported
            Terminal::RawPkH(_) => None,
        })
    }
}

fn psbt_inputs_sat(psbt: &Psbt) -> impl Iterator<Item = PsbtInputSatisfier> {
    (0..psbt.inputs.len()).map(move |i| PsbtInputSatisfier::new(psbt, i))
}

/// Options to build the satisfaction field in the policy
#[derive(Debug, Clone, Copy)]
pub enum BuildSatisfaction<'a> {
    /// Don't generate `satisfaction` field
    None,
    /// Analyze the given PSBT to check for existing signatures
    Psbt(&'a Psbt),
    /// Like `Psbt` variant and also check for expired timelocks
    PsbtTimelocks {
        /// Given PSBT
        psbt: &'a Psbt,
        /// Current blockchain height
        current_height: u32,
        /// The highest confirmation height between the inputs
        /// CSV should consider different inputs, but we consider the worst condition for the tx as whole
        input_max_height: u32,
    },
}
impl<'a> BuildSatisfaction<'a> {
    fn psbt(&self) -> Option<&'a Psbt> {
        match self {
            BuildSatisfaction::None => None,
            BuildSatisfaction::Psbt(psbt) => Some(psbt),
            BuildSatisfaction::PsbtTimelocks { psbt, .. } => Some(psbt),
        }
    }
}

impl ExtractPolicy for Descriptor<DescriptorPublicKey> {
    fn extract_policy(
        &self,
        signers: &SignersContainer,
        build_sat: BuildSatisfaction,
        secp: &SecpCtx,
    ) -> Result<Option<Policy>, Error> {
        fn make_sortedmulti<Ctx: ScriptContext + 'static>(
            keys: &SortedMultiVec<DescriptorPublicKey, Ctx>,
            signers: &SignersContainer,
            build_sat: BuildSatisfaction,
            secp: &SecpCtx,
        ) -> Result<Option<Policy>, Error> {
            Ok(Policy::make_multisig::<Ctx>(
                keys.pks.as_ref(),
                signers,
                build_sat,
                keys.k,
                true,
                secp,
            )?)
        }

        match self {
            Descriptor::Pkh(pk) => Ok(Some(miniscript::Legacy::make_signature(
                pk.as_inner(),
                signers,
                build_sat,
                secp,
            ))),
            Descriptor::Wpkh(pk) => Ok(Some(miniscript::Segwitv0::make_signature(
                pk.as_inner(),
                signers,
                build_sat,
                secp,
            ))),
            Descriptor::Sh(sh) => match sh.as_inner() {
                ShInner::Wpkh(pk) => Ok(Some(miniscript::Segwitv0::make_signature(
                    pk.as_inner(),
                    signers,
                    build_sat,
                    secp,
                ))),
                ShInner::Ms(ms) => Ok(ms.extract_policy(signers, build_sat, secp)?),
                ShInner::SortedMulti(ref keys) => make_sortedmulti(keys, signers, build_sat, secp),
                ShInner::Wsh(wsh) => match wsh.as_inner() {
                    WshInner::Ms(ms) => Ok(ms.extract_policy(signers, build_sat, secp)?),
                    WshInner::SortedMulti(ref keys) => {
                        make_sortedmulti(keys, signers, build_sat, secp)
                    }
                },
            },
            Descriptor::Wsh(wsh) => match wsh.as_inner() {
                WshInner::Ms(ms) => Ok(ms.extract_policy(signers, build_sat, secp)?),
                WshInner::SortedMulti(ref keys) => make_sortedmulti(keys, signers, build_sat, secp),
            },
            Descriptor::Bare(ms) => Ok(ms.as_inner().extract_policy(signers, build_sat, secp)?),
            Descriptor::Tr(tr) => {
                // If there's no tap tree, treat this as a single sig, otherwise build a `Thresh`
                // node with threshold = 1 and the key spend signature plus all the tree leaves
                let key_spend_sig =
                    miniscript::Tap::make_signature(tr.internal_key(), signers, build_sat, secp);

                if tr.taptree().is_none() {
                    Ok(Some(key_spend_sig))
                } else {
                    let mut items = vec![key_spend_sig];
                    items.append(
                        &mut tr
                            .iter_scripts()
                            .filter_map(|(_, ms)| {
                                ms.extract_policy(signers, build_sat, secp).transpose()
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                    );

                    Ok(Policy::make_thresh(items, 1)?)
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::descriptor;
    use crate::descriptor::{ExtractPolicy, IntoWalletDescriptor};

    use super::*;
    use crate::descriptor::policy::SatisfiableItem::{EcdsaSignature, Multisig, Thresh};
    use crate::keys::{DescriptorKey, IntoDescriptorKey};
    use crate::wallet::signer::SignersContainer;
    use assert_matches::assert_matches;
    use bitcoin::secp256k1::Secp256k1;
    use bitcoin::util::bip32;
    use bitcoin::Network;
    use std::str::FromStr;
    use std::sync::Arc;

    const TPRV0_STR:&str = "tprv8ZgxMBicQKsPdZXrcHNLf5JAJWFAoJ2TrstMRdSKtEggz6PddbuSkvHKM9oKJyFgZV1B7rw8oChspxyYbtmEXYyg1AjfWbL3ho3XHDpHRZf";
    const TPRV1_STR:&str = "tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcLNfjdi5qUvw3VDfgYiH5mNsj5izuiu2N";

    const PATH: &str = "m/44'/1'/0'/0";

    fn setup_keys<Ctx: ScriptContext>(
        tprv: &str,
        path: &str,
        secp: &SecpCtx,
    ) -> (DescriptorKey<Ctx>, DescriptorKey<Ctx>, Fingerprint) {
        let path = bip32::DerivationPath::from_str(path).unwrap();
        let tprv = bip32::ExtendedPrivKey::from_str(tprv).unwrap();
        let tpub = bip32::ExtendedPubKey::from_priv(secp, &tprv);
        let fingerprint = tprv.fingerprint(secp);
        let prvkey = (tprv, path.clone()).into_descriptor_key().unwrap();
        let pubkey = (tpub, path).into_descriptor_key().unwrap();

        (prvkey, pubkey, fingerprint)
    }

    // test ExtractPolicy trait for simple descriptors; wpkh(), sh(multi())

    #[test]
    fn test_extract_policy_for_wpkh() {
        let secp = Secp256k1::new();

        let (prvkey, pubkey, fingerprint) = setup_keys(TPRV0_STR, PATH, &secp);
        let desc = descriptor!(wpkh(pubkey)).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));
        let policy = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::None, &secp)
            .unwrap()
            .unwrap();

        assert_matches!(&policy.item, EcdsaSignature(PkOrF::Fingerprint(f)) if f == &fingerprint);
        assert_matches!(&policy.contribution, Satisfaction::None);

        let desc = descriptor!(wpkh(prvkey)).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));
        let policy = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::None, &secp)
            .unwrap()
            .unwrap();

        assert_matches!(&policy.item, EcdsaSignature(PkOrF::Fingerprint(f)) if f == &fingerprint);
        assert_matches!(&policy.contribution, Satisfaction::Complete {condition} if condition.csv.is_none() && condition.timelock.is_none());
    }

    // 2 pub keys descriptor, required 2 prv keys
    #[test]
    fn test_extract_policy_for_sh_multi_partial_0of2() {
        let secp = Secp256k1::new();
        let (_prvkey0, pubkey0, fingerprint0) = setup_keys(TPRV0_STR, PATH, &secp);
        let (_prvkey1, pubkey1, fingerprint1) = setup_keys(TPRV1_STR, PATH, &secp);
        let desc = descriptor!(sh(multi(2, pubkey0, pubkey1))).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));
        let policy = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::None, &secp)
            .unwrap()
            .unwrap();

        assert_matches!(&policy.item, Multisig { keys, threshold } if threshold == &2usize
            && keys[0] == PkOrF::Fingerprint(fingerprint0)
            && keys[1] == PkOrF::Fingerprint(fingerprint1)
        );
        // TODO should this be "Satisfaction::None" since we have no prv keys?
        // TODO should items and conditions not be empty?
        assert_matches!(&policy.contribution, Satisfaction::Partial { n, m, items, conditions, ..} if n == &2usize
            && m == &2usize
            && items.is_empty()
            && conditions.is_empty()
        );
    }

    // 1 prv and 1 pub key descriptor, required 2 prv keys
    #[test]
    fn test_extract_policy_for_sh_multi_partial_1of2() {
        let secp = Secp256k1::new();
        let (prvkey0, _pubkey0, fingerprint0) = setup_keys(TPRV0_STR, PATH, &secp);
        let (_prvkey1, pubkey1, fingerprint1) = setup_keys(TPRV1_STR, PATH, &secp);
        let desc = descriptor!(sh(multi(2, prvkey0, pubkey1))).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));
        let policy = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::None, &secp)
            .unwrap()
            .unwrap();
        assert_matches!(&policy.item, Multisig { keys, threshold } if threshold == &2usize
            && keys[0] == PkOrF::Fingerprint(fingerprint0)
            && keys[1] == PkOrF::Fingerprint(fingerprint1)
        );

        assert_matches!(&policy.contribution, Satisfaction::Partial { n, m, items, conditions, ..} if n == &2usize
             && m == &2usize
             && items.len() == 1
             && conditions.contains_key(&0)
        );
    }

    // 1 prv and 1 pub key descriptor, required 1 prv keys
    #[test]
    #[ignore] // see https://github.com/bitcoindevkit/bdk/issues/225
    fn test_extract_policy_for_sh_multi_complete_1of2() {
        let secp = Secp256k1::new();

        let (_prvkey0, pubkey0, fingerprint0) = setup_keys(TPRV0_STR, PATH, &secp);
        let (prvkey1, _pubkey1, fingerprint1) = setup_keys(TPRV1_STR, PATH, &secp);
        let desc = descriptor!(sh(multi(1, pubkey0, prvkey1))).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));
        let policy = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::None, &secp)
            .unwrap()
            .unwrap();

        assert_matches!(&policy.item, Multisig { keys, threshold } if threshold == &1
            && keys[0] == PkOrF::Fingerprint(fingerprint0)
            && keys[1] == PkOrF::Fingerprint(fingerprint1)
        );
        assert_matches!(&policy.contribution, Satisfaction::PartialComplete { n, m, items, conditions, .. } if n == &2
             && m == &1
             && items.len() == 2
             && conditions.contains_key(&vec![0])
             && conditions.contains_key(&vec![1])
        );
    }

    // 2 prv keys descriptor, required 2 prv keys
    #[test]
    fn test_extract_policy_for_sh_multi_complete_2of2() {
        let secp = Secp256k1::new();

        let (prvkey0, _pubkey0, fingerprint0) = setup_keys(TPRV0_STR, PATH, &secp);
        let (prvkey1, _pubkey1, fingerprint1) = setup_keys(TPRV1_STR, PATH, &secp);
        let desc = descriptor!(sh(multi(2, prvkey0, prvkey1))).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));
        let policy = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::None, &secp)
            .unwrap()
            .unwrap();

        assert_matches!(&policy.item, Multisig { keys, threshold } if threshold == &2
            && keys[0] == PkOrF::Fingerprint(fingerprint0)
            && keys[1] == PkOrF::Fingerprint(fingerprint1)
        );

        assert_matches!(&policy.contribution, Satisfaction::PartialComplete { n, m, items, conditions, .. } if n == &2
             && m == &2
             && items.len() == 2
             && conditions.contains_key(&vec![0,1])
        );
    }

    // test ExtractPolicy trait with extended and single keys

    #[test]
    fn test_extract_policy_for_single_wpkh() {
        let secp = Secp256k1::new();

        let (prvkey, pubkey, fingerprint) = setup_keys(TPRV0_STR, PATH, &secp);
        let desc = descriptor!(wpkh(pubkey)).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));
        let policy = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::None, &secp)
            .unwrap()
            .unwrap();

        assert_matches!(&policy.item, EcdsaSignature(PkOrF::Fingerprint(f)) if f == &fingerprint);
        assert_matches!(&policy.contribution, Satisfaction::None);

        let desc = descriptor!(wpkh(prvkey)).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));
        let policy = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::None, &secp)
            .unwrap()
            .unwrap();

        assert_matches!(policy.item, EcdsaSignature(PkOrF::Fingerprint(f)) if f == fingerprint);
        assert_matches!(policy.contribution, Satisfaction::Complete {condition} if condition.csv.is_none() && condition.timelock.is_none());
    }

    // single key, 1 prv and 1 pub key descriptor, required 1 prv keys
    #[test]
    #[ignore] // see https://github.com/bitcoindevkit/bdk/issues/225
    fn test_extract_policy_for_single_wsh_multi_complete_1of2() {
        let secp = Secp256k1::new();

        let (_prvkey0, pubkey0, fingerprint0) = setup_keys(TPRV0_STR, PATH, &secp);
        let (prvkey1, _pubkey1, fingerprint1) = setup_keys(TPRV1_STR, PATH, &secp);
        let desc = descriptor!(sh(multi(1, pubkey0, prvkey1))).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));
        let policy = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::None, &secp)
            .unwrap()
            .unwrap();

        assert_matches!(policy.item, Multisig { keys, threshold } if threshold == 1
            && keys[0] == PkOrF::Fingerprint(fingerprint0)
            && keys[1] == PkOrF::Fingerprint(fingerprint1)
        );
        assert_matches!(policy.contribution, Satisfaction::PartialComplete { n, m, items, conditions, .. } if n == 2
             && m == 1
             && items.len() == 2
             && conditions.contains_key(&vec![0])
             && conditions.contains_key(&vec![1])
        );
    }

    // test ExtractPolicy trait with descriptors containing timelocks in a thresh()

    #[test]
    #[ignore] // see https://github.com/bitcoindevkit/bdk/issues/225
    fn test_extract_policy_for_wsh_multi_timelock() {
        let secp = Secp256k1::new();

        let (prvkey0, _pubkey0, _fingerprint0) = setup_keys(TPRV0_STR, PATH, &secp);
        let (_prvkey1, pubkey1, _fingerprint1) = setup_keys(TPRV1_STR, PATH, &secp);
        let sequence = 50;
        #[rustfmt::skip]
        let desc = descriptor!(wsh(thresh(
            2,
            pk(prvkey0),
            s:pk(pubkey1),
            s:d:v:older(sequence)
        )))
        .unwrap();

        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));
        let policy = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::None, &secp)
            .unwrap()
            .unwrap();

        assert_matches!(&policy.item, Thresh { items, threshold } if items.len() == 3 && threshold == &2);

        assert_matches!(&policy.contribution, Satisfaction::PartialComplete { n, m, items, conditions, .. } if n == &3
             && m == &2
             && items.len() == 3
             && conditions.get(&vec![0,1]).unwrap().iter().next().unwrap().csv.is_none()
             && conditions.get(&vec![0,2]).unwrap().iter().next().unwrap().csv == Some(Sequence(sequence))
             && conditions.get(&vec![1,2]).unwrap().iter().next().unwrap().csv == Some(Sequence(sequence))
        );
    }

    // - mixed timelocks should fail

    #[test]
    #[ignore]
    fn test_extract_policy_for_wsh_mixed_timelocks() {
        let secp = Secp256k1::new();
        let (prvkey0, _pubkey0, _fingerprint0) = setup_keys(TPRV0_STR, PATH, &secp);
        let locktime_threshold = 500000000; // if less than this means block number, else block time in seconds
        let locktime_blocks = 100;
        let locktime_seconds = locktime_blocks + locktime_threshold;
        let desc = descriptor!(sh(and_v(
            v: pk(prvkey0),
            and_v(v: after(locktime_seconds), after(locktime_blocks))
        )))
        .unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));
        let policy = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::None, &secp)
            .unwrap()
            .unwrap();
        println!("desc policy = {:?}", policy); // TODO remove
                                                // TODO how should this fail with mixed timelocks?
    }

    // - multiple timelocks of the same type should be correctly merged together
    #[test]
    #[ignore]
    fn test_extract_policy_for_multiple_same_timelocks() {
        let secp = Secp256k1::new();
        let (prvkey0, _pubkey0, _fingerprint0) = setup_keys(TPRV0_STR, PATH, &secp);
        let locktime_blocks0 = 100;
        let locktime_blocks1 = 200;
        let desc = descriptor!(sh(and_v(
            v: pk(prvkey0),
            and_v(v: after(locktime_blocks0), after(locktime_blocks1))
        )))
        .unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));
        let policy = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::None, &secp)
            .unwrap()
            .unwrap();
        println!("desc policy = {:?}", policy); // TODO remove
                                                // TODO how should this merge timelocks?
        let (prvkey1, _pubkey1, _fingerprint1) = setup_keys(TPRV0_STR, PATH, &secp);
        let locktime_seconds0 = 500000100;
        let locktime_seconds1 = 500000200;
        let desc = descriptor!(sh(and_v(
            v: pk(prvkey1),
            and_v(v: after(locktime_seconds0), after(locktime_seconds1))
        )))
        .unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));
        let policy = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::None, &secp)
            .unwrap()
            .unwrap();

        println!("desc policy = {:?}", policy); // TODO remove

        // TODO how should this merge timelocks?
    }

    #[test]
    fn test_get_condition_multisig() {
        let secp = Secp256k1::new();

        let (_, pk0, _) = setup_keys(TPRV0_STR, PATH, &secp);
        let (_, pk1, _) = setup_keys(TPRV1_STR, PATH, &secp);

        let desc = descriptor!(wsh(multi(1, pk0, pk1))).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));

        let policy = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::None, &secp)
            .unwrap()
            .unwrap();

        // no args, choose the default
        let no_args = policy.get_condition(&vec![].into_iter().collect());
        assert_eq!(no_args, Ok(Condition::default()));

        // enough args
        let eq_thresh =
            policy.get_condition(&vec![(policy.id.clone(), vec![0])].into_iter().collect());
        assert_eq!(eq_thresh, Ok(Condition::default()));

        // more args, it doesn't really change anything
        let gt_thresh =
            policy.get_condition(&vec![(policy.id.clone(), vec![0, 1])].into_iter().collect());
        assert_eq!(gt_thresh, Ok(Condition::default()));

        // not enough args, error
        let lt_thresh =
            policy.get_condition(&vec![(policy.id.clone(), vec![])].into_iter().collect());
        assert_eq!(
            lt_thresh,
            Err(PolicyError::NotEnoughItemsSelected(policy.id.clone()))
        );

        // index out of range
        let out_of_range =
            policy.get_condition(&vec![(policy.id.clone(), vec![5])].into_iter().collect());
        assert_eq!(out_of_range, Err(PolicyError::IndexOutOfRange(5)));
    }

    const ALICE_TPRV_STR:&str = "tprv8ZgxMBicQKsPf6T5X327efHnvJDr45Xnb8W4JifNWtEoqXu9MRYS4v1oYe6DFcMVETxy5w3bqpubYRqvcVTqovG1LifFcVUuJcbwJwrhYzP";
    const BOB_TPRV_STR:&str = "tprv8ZgxMBicQKsPeinZ155cJAn117KYhbaN6MV3WeG6sWhxWzcvX1eg1awd4C9GpUN1ncLEM2rzEvunAg3GizdZD4QPPCkisTz99tXXB4wZArp";
    const CAROL_TPRV_STR:&str = "tprv8ZgxMBicQKsPdC3CicFifuLCEyVVdXVUNYorxUWj3iGZ6nimnLAYAY9SYB7ib8rKzRxrCKFcEytCt6szwd2GHnGPRCBLAEAoSVDefSNk4Bt";
    const ALICE_BOB_PATH: &str = "m/0'";

    #[test]
    fn test_extract_satisfaction() {
        const ALICE_SIGNED_PSBT: &str = "cHNidP8BAFMBAAAAAZb0njwT2wRS3AumaaP3yb7T4MxOePpSWih4Nq+jWChMAQAAAAD/////Af4lAAAAAAAAF6kUXv2Fn+YemPP4PUpNR1ZbU16/eRCHAAAAAAABASuJJgAAAAAAACIAIERw5kTLo9DUH9QDJSClHQwPpC7VGJ+ZMDpa8U+2fzcYIgIDeAtjYQk/Vfu4db2+68hyMKjc38+kWl5sP5QH8L42ZstHMEQCIBj0jLjUeVYXNQ6cqB+gbtvuKMjV54wSgWlm1cfcgpHVAiBa3DtC9l/1Mt4IDCvR7mmwQd3eAP/m5++81euhJNSrgQEBBUdSIQN4C2NhCT9V+7h1vb7ryHIwqNzfz6RaXmw/lAfwvjZmyyEC+GE/y+LptI8xmiR6sOe998IGzybox0Qfz4+BQl1nmYhSriIGAvhhP8vi6bSPMZokerDnvffCBs8m6MdEH8+PgUJdZ5mIDBwu7j4AAACAAAAAACIGA3gLY2EJP1X7uHW9vuvIcjCo3N/PpFpebD+UB/C+NmbLDMkRfC4AAACAAAAAAAAA";
        const BOB_SIGNED_PSBT: &str =   "cHNidP8BAFMBAAAAAZb0njwT2wRS3AumaaP3yb7T4MxOePpSWih4Nq+jWChMAQAAAAD/////Af4lAAAAAAAAF6kUXv2Fn+YemPP4PUpNR1ZbU16/eRCHAAAAAAABASuJJgAAAAAAACIAIERw5kTLo9DUH9QDJSClHQwPpC7VGJ+ZMDpa8U+2fzcYIgIC+GE/y+LptI8xmiR6sOe998IGzybox0Qfz4+BQl1nmYhIMEUCIQD5zDtM5MwklurwJ5aW76RsO36Iqyu+6uMdVlhL6ws2GQIgesAiz4dbKS7UmhDsC/c1ezu0o6hp00UUtsCMfUZ4anYBAQVHUiEDeAtjYQk/Vfu4db2+68hyMKjc38+kWl5sP5QH8L42ZsshAvhhP8vi6bSPMZokerDnvffCBs8m6MdEH8+PgUJdZ5mIUq4iBgL4YT/L4um0jzGaJHqw5733wgbPJujHRB/Pj4FCXWeZiAwcLu4+AAAAgAAAAAAiBgN4C2NhCT9V+7h1vb7ryHIwqNzfz6RaXmw/lAfwvjZmywzJEXwuAAAAgAAAAAAAAA==";
        const ALICE_BOB_SIGNED_PSBT: &str =   "cHNidP8BAFMBAAAAAZb0njwT2wRS3AumaaP3yb7T4MxOePpSWih4Nq+jWChMAQAAAAD/////Af4lAAAAAAAAF6kUXv2Fn+YemPP4PUpNR1ZbU16/eRCHAAAAAAABASuJJgAAAAAAACIAIERw5kTLo9DUH9QDJSClHQwPpC7VGJ+ZMDpa8U+2fzcYIgIC+GE/y+LptI8xmiR6sOe998IGzybox0Qfz4+BQl1nmYhIMEUCIQD5zDtM5MwklurwJ5aW76RsO36Iqyu+6uMdVlhL6ws2GQIgesAiz4dbKS7UmhDsC/c1ezu0o6hp00UUtsCMfUZ4anYBIgIDeAtjYQk/Vfu4db2+68hyMKjc38+kWl5sP5QH8L42ZstHMEQCIBj0jLjUeVYXNQ6cqB+gbtvuKMjV54wSgWlm1cfcgpHVAiBa3DtC9l/1Mt4IDCvR7mmwQd3eAP/m5++81euhJNSrgQEBBUdSIQN4C2NhCT9V+7h1vb7ryHIwqNzfz6RaXmw/lAfwvjZmyyEC+GE/y+LptI8xmiR6sOe998IGzybox0Qfz4+BQl1nmYhSriIGAvhhP8vi6bSPMZokerDnvffCBs8m6MdEH8+PgUJdZ5mIDBwu7j4AAACAAAAAACIGA3gLY2EJP1X7uHW9vuvIcjCo3N/PpFpebD+UB/C+NmbLDMkRfC4AAACAAAAAAAEHAAEI2wQARzBEAiAY9Iy41HlWFzUOnKgfoG7b7ijI1eeMEoFpZtXH3IKR1QIgWtw7QvZf9TLeCAwr0e5psEHd3gD/5ufvvNXroSTUq4EBSDBFAiEA+cw7TOTMJJbq8CeWlu+kbDt+iKsrvurjHVZYS+sLNhkCIHrAIs+HWyku1JoQ7Av3NXs7tKOoadNFFLbAjH1GeGp2AUdSIQN4C2NhCT9V+7h1vb7ryHIwqNzfz6RaXmw/lAfwvjZmyyEC+GE/y+LptI8xmiR6sOe998IGzybox0Qfz4+BQl1nmYhSrgAA";

        let secp = Secp256k1::new();

        let (prvkey_alice, _, _) = setup_keys(ALICE_TPRV_STR, ALICE_BOB_PATH, &secp);
        let (prvkey_bob, _, _) = setup_keys(BOB_TPRV_STR, ALICE_BOB_PATH, &secp);

        let desc = descriptor!(wsh(multi(2, prvkey_alice, prvkey_bob))).unwrap();

        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();

        let addr = wallet_desc
            .at_derivation_index(0)
            .address(Network::Testnet)
            .unwrap();
        assert_eq!(
            "tb1qg3cwv3xt50gdg875qvjjpfgaps86gtk4rz0ejvp6ttc5ldnlxuvqlcn0xk",
            addr.to_string()
        );

        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));

        let psbt = Psbt::from_str(ALICE_SIGNED_PSBT).unwrap();

        let policy_alice_psbt = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::Psbt(&psbt), &secp)
            .unwrap()
            .unwrap();
        //println!("{}", serde_json::to_string(&policy_alice_psbt).unwrap());

        assert_matches!(&policy_alice_psbt.satisfaction, Satisfaction::Partial { n, m, items, .. } if n == &2
             && m == &2
             && items == &vec![0]
        );

        let psbt = Psbt::from_str(BOB_SIGNED_PSBT).unwrap();
        let policy_bob_psbt = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::Psbt(&psbt), &secp)
            .unwrap()
            .unwrap();
        //println!("{}", serde_json::to_string(&policy_bob_psbt).unwrap());

        assert_matches!(&policy_bob_psbt.satisfaction, Satisfaction::Partial { n, m, items, .. } if n == &2
             && m == &2
             && items == &vec![1]
        );

        let psbt = Psbt::from_str(ALICE_BOB_SIGNED_PSBT).unwrap();
        let policy_alice_bob_psbt = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::Psbt(&psbt), &secp)
            .unwrap()
            .unwrap();
        assert_matches!(&policy_alice_bob_psbt.satisfaction, Satisfaction::PartialComplete { n, m, items, .. } if n == &2
             && m == &2
             && items == &vec![0, 1]
        );
    }

    #[test]
    fn test_extract_satisfaction_timelock() {
        //const PSBT_POLICY_CONSIDER_TIMELOCK_NOT_EXPIRED: &str = "cHNidP8BAFMBAAAAAdld52uJFGT7Yde0YZdSVh2vL020Zm2exadH5R4GSNScAAAAAAD/////ATrcAAAAAAAAF6kUXv2Fn+YemPP4PUpNR1ZbU16/eRCHAAAAAAABASvI3AAAAAAAACIAILhzvvcBzw/Zfnc9ispRK0PCahxn1F6RHXTZAmw5tqNPAQVSdmNSsmlofCEDeAtjYQk/Vfu4db2+68hyMKjc38+kWl5sP5QH8L42Zsusk3whAvhhP8vi6bSPMZokerDnvffCBs8m6MdEH8+PgUJdZ5mIrJNShyIGAvhhP8vi6bSPMZokerDnvffCBs8m6MdEH8+PgUJdZ5mIDBwu7j4AAACAAAAAACIGA3gLY2EJP1X7uHW9vuvIcjCo3N/PpFpebD+UB/C+NmbLDMkRfC4AAACAAAAAAAAA";
        const PSBT_POLICY_CONSIDER_TIMELOCK_EXPIRED:     &str = "cHNidP8BAFMCAAAAAdld52uJFGT7Yde0YZdSVh2vL020Zm2exadH5R4GSNScAAAAAAACAAAAATrcAAAAAAAAF6kUXv2Fn+YemPP4PUpNR1ZbU16/eRCHAAAAAAABASvI3AAAAAAAACIAILhzvvcBzw/Zfnc9ispRK0PCahxn1F6RHXTZAmw5tqNPAQVSdmNSsmlofCEDeAtjYQk/Vfu4db2+68hyMKjc38+kWl5sP5QH8L42Zsusk3whAvhhP8vi6bSPMZokerDnvffCBs8m6MdEH8+PgUJdZ5mIrJNShyIGAvhhP8vi6bSPMZokerDnvffCBs8m6MdEH8+PgUJdZ5mIDBwu7j4AAACAAAAAACIGA3gLY2EJP1X7uHW9vuvIcjCo3N/PpFpebD+UB/C+NmbLDMkRfC4AAACAAAAAAAAA";
        const PSBT_POLICY_CONSIDER_TIMELOCK_EXPIRED_SIGNED: &str ="cHNidP8BAFMCAAAAAdld52uJFGT7Yde0YZdSVh2vL020Zm2exadH5R4GSNScAAAAAAACAAAAATrcAAAAAAAAF6kUXv2Fn+YemPP4PUpNR1ZbU16/eRCHAAAAAAABASvI3AAAAAAAACIAILhzvvcBzw/Zfnc9ispRK0PCahxn1F6RHXTZAmw5tqNPIgIDeAtjYQk/Vfu4db2+68hyMKjc38+kWl5sP5QH8L42ZstIMEUCIQCtZxNm6H3Ux3pnc64DSpgohMdBj+57xhFHcURYt2BpPAIgG3OnI7bcj/3GtWX1HHyYGSI7QGa/zq5YnsmK1Cw29NABAQVSdmNSsmlofCEDeAtjYQk/Vfu4db2+68hyMKjc38+kWl5sP5QH8L42Zsusk3whAvhhP8vi6bSPMZokerDnvffCBs8m6MdEH8+PgUJdZ5mIrJNShyIGAvhhP8vi6bSPMZokerDnvffCBs8m6MdEH8+PgUJdZ5mIDBwu7j4AAACAAAAAACIGA3gLY2EJP1X7uHW9vuvIcjCo3N/PpFpebD+UB/C+NmbLDMkRfC4AAACAAAAAAAEHAAEIoAQASDBFAiEArWcTZuh91Md6Z3OuA0qYKITHQY/ue8YRR3FEWLdgaTwCIBtzpyO23I/9xrVl9Rx8mBkiO0Bmv86uWJ7JitQsNvTQAQEBUnZjUrJpaHwhA3gLY2EJP1X7uHW9vuvIcjCo3N/PpFpebD+UB/C+NmbLrJN8IQL4YT/L4um0jzGaJHqw5733wgbPJujHRB/Pj4FCXWeZiKyTUocAAA==";

        let secp = Secp256k1::new();

        let (prvkey_alice, _, _) = setup_keys(ALICE_TPRV_STR, ALICE_BOB_PATH, &secp);
        let (prvkey_bob, _, _) = setup_keys(BOB_TPRV_STR, ALICE_BOB_PATH, &secp);

        let desc =
            descriptor!(wsh(thresh(2,n:d:v:older(2),s:pk(prvkey_alice),s:pk(prvkey_bob)))).unwrap();

        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));

        let addr = wallet_desc
            .at_derivation_index(0)
            .address(Network::Testnet)
            .unwrap();
        assert_eq!(
            "tb1qsydsey4hexagwkvercqsmes6yet0ndkyt6uzcphtqnygjd8hmzmsfxrv58",
            addr.to_string()
        );

        let psbt = Psbt::from_str(PSBT_POLICY_CONSIDER_TIMELOCK_EXPIRED).unwrap();

        let build_sat = BuildSatisfaction::PsbtTimelocks {
            psbt: &psbt,
            current_height: 10,
            input_max_height: 9,
        };

        let policy = wallet_desc
            .extract_policy(&signers_container, build_sat, &secp)
            .unwrap()
            .unwrap();
        assert_matches!(&policy.satisfaction, Satisfaction::Partial { n, m, items, .. } if n == &3
             && m == &2
             && items.is_empty()
        );
        //println!("{}", serde_json::to_string(&policy).unwrap());

        let build_sat_expired = BuildSatisfaction::PsbtTimelocks {
            psbt: &psbt,
            current_height: 12,
            input_max_height: 9,
        };

        let policy_expired = wallet_desc
            .extract_policy(&signers_container, build_sat_expired, &secp)
            .unwrap()
            .unwrap();
        assert_matches!(&policy_expired.satisfaction, Satisfaction::Partial { n, m, items, .. } if n == &3
             && m == &2
             && items == &vec![0]
        );
        //println!("{}", serde_json::to_string(&policy_expired).unwrap());

        let psbt_signed = Psbt::from_str(PSBT_POLICY_CONSIDER_TIMELOCK_EXPIRED_SIGNED).unwrap();

        let build_sat_expired_signed = BuildSatisfaction::PsbtTimelocks {
            psbt: &psbt_signed,
            current_height: 12,
            input_max_height: 9,
        };

        let policy_expired_signed = wallet_desc
            .extract_policy(&signers_container, build_sat_expired_signed, &secp)
            .unwrap()
            .unwrap();
        assert_matches!(&policy_expired_signed.satisfaction, Satisfaction::PartialComplete { n, m, items, .. } if n == &3
             && m == &2
             && items == &vec![0, 1]
        );
        //println!("{}", serde_json::to_string(&policy_expired_signed).unwrap());
    }

    #[test]
    fn test_extract_pkh() {
        let secp = Secp256k1::new();

        let (prvkey_alice, _, _) = setup_keys(ALICE_TPRV_STR, ALICE_BOB_PATH, &secp);
        let (prvkey_bob, _, _) = setup_keys(BOB_TPRV_STR, ALICE_BOB_PATH, &secp);
        let (prvkey_carol, _, _) = setup_keys(CAROL_TPRV_STR, ALICE_BOB_PATH, &secp);

        let desc = descriptor!(wsh(c: andor(
            pk(prvkey_alice),
            pk_k(prvkey_bob),
            pk_h(prvkey_carol),
        )))
        .unwrap();

        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));

        let policy = wallet_desc.extract_policy(&signers_container, BuildSatisfaction::None, &secp);
        assert!(policy.is_ok());
    }

    #[test]
    fn test_extract_tr_key_spend() {
        let secp = Secp256k1::new();

        let (prvkey, _, fingerprint) = setup_keys(ALICE_TPRV_STR, ALICE_BOB_PATH, &secp);

        let desc = descriptor!(tr(prvkey)).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));

        let policy = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::None, &secp)
            .unwrap();
        assert_eq!(
            policy,
            Some(Policy {
                id: "48u0tz0n".to_string(),
                item: SatisfiableItem::SchnorrSignature(PkOrF::Fingerprint(fingerprint)),
                satisfaction: Satisfaction::None,
                contribution: Satisfaction::Complete {
                    condition: Condition::default()
                }
            })
        );
    }

    #[test]
    fn test_extract_tr_script_spend() {
        let secp = Secp256k1::new();

        let (alice_prv, _, alice_fing) = setup_keys(ALICE_TPRV_STR, ALICE_BOB_PATH, &secp);
        let (_, bob_pub, bob_fing) = setup_keys(BOB_TPRV_STR, ALICE_BOB_PATH, &secp);

        let desc = descriptor!(tr(bob_pub, pk(alice_prv))).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::build(keymap, &wallet_desc, &secp));

        let policy = wallet_desc
            .extract_policy(&signers_container, BuildSatisfaction::None, &secp)
            .unwrap()
            .unwrap();

        assert_matches!(policy.item, SatisfiableItem::Thresh { ref items, threshold: 1 } if items.len() == 2);
        assert_matches!(policy.contribution, Satisfaction::PartialComplete { n: 2, m: 1, items, .. } if items == vec![1]);

        let alice_sig = SatisfiableItem::SchnorrSignature(PkOrF::Fingerprint(alice_fing));
        let bob_sig = SatisfiableItem::SchnorrSignature(PkOrF::Fingerprint(bob_fing));

        let thresh_items = match policy.item {
            SatisfiableItem::Thresh { items, .. } => items,
            _ => unreachable!(),
        };

        assert_eq!(thresh_items[0].item, bob_sig);
        assert_eq!(thresh_items[1].item, alice_sig);
    }

    #[test]
    fn test_extract_tr_satisfaction_key_spend() {
        const UNSIGNED_PSBT: &str = "cHNidP8BAFMBAAAAAUKgMCqtGLSiGYhsTols2UJ/VQQgQi/SXO38uXs2SahdAQAAAAD/////ARyWmAAAAAAAF6kU4R3W8CnGzZcSsaovTYu0X8vHt3WHAAAAAAABASuAlpgAAAAAACJRIEiEBFjbZa1xdjLfFjrKzuC1F1LeRyI/gL6IuGKNmUuSIRYnkGTDxwXMHP32fkDFoGJY28trxbkkVgR2z7jZa2pOJA0AyRF8LgAAAIADAAAAARcgJ5Bkw8cFzBz99n5AxaBiWNvLa8W5JFYEds+42WtqTiQAAA==";
        const SIGNED_PSBT: &str = "cHNidP8BAFMBAAAAAUKgMCqtGLSiGYhsTols2UJ/VQQgQi/SXO38uXs2SahdAQAAAAD/////ARyWmAAAAAAAF6kU4R3W8CnGzZcSsaovTYu0X8vHt3WHAAAAAAABASuAlpgAAAAAACJRIEiEBFjbZa1xdjLfFjrKzuC1F1LeRyI/gL6IuGKNmUuSARNAIsRvARpRxuyQosVA7guRQT9vXr+S25W2tnP2xOGBsSgq7A4RL8yrbvwDmNlWw9R0Nc/6t+IsyCyy7dD/lbUGgyEWJ5Bkw8cFzBz99n5AxaBiWNvLa8W5JFYEds+42WtqTiQNAMkRfC4AAACAAwAAAAEXICeQZMPHBcwc/fZ+QMWgYljby2vFuSRWBHbPuNlrak4kAAA=";

        let unsigned_psbt = Psbt::from_str(UNSIGNED_PSBT).unwrap();
        let signed_psbt = Psbt::from_str(SIGNED_PSBT).unwrap();

        let secp = Secp256k1::new();

        let (_, pubkey, _) = setup_keys(ALICE_TPRV_STR, ALICE_BOB_PATH, &secp);

        let desc = descriptor!(tr(pubkey)).unwrap();
        let (wallet_desc, _) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();

        let policy_unsigned = wallet_desc
            .extract_policy(
                &SignersContainer::default(),
                BuildSatisfaction::Psbt(&unsigned_psbt),
                &secp,
            )
            .unwrap()
            .unwrap();
        let policy_signed = wallet_desc
            .extract_policy(
                &SignersContainer::default(),
                BuildSatisfaction::Psbt(&signed_psbt),
                &secp,
            )
            .unwrap()
            .unwrap();

        assert_eq!(policy_unsigned.satisfaction, Satisfaction::None);
        assert_eq!(
            policy_signed.satisfaction,
            Satisfaction::Complete {
                condition: Default::default()
            }
        );
    }

    #[test]
    fn test_extract_tr_satisfaction_script_spend() {
        const UNSIGNED_PSBT: &str = "cHNidP8BAFMBAAAAAWZalxaErOL7P3WPIUc8DsjgE68S+ww+uqiqEI2SAwlPAAAAAAD/////AQiWmAAAAAAAF6kU4R3W8CnGzZcSsaovTYu0X8vHt3WHAAAAAAABASuAlpgAAAAAACJRINa6bLPZwp3/CYWoxyI3mLYcSC5f9LInAMUng94nspa2IhXBgiPY+kcolS1Hp0niOK/+7VHz6F+nsz8JVxnzWzkgToYjIHhGyuexxtRVKevRx4YwWR/W0r7LPHt6oS6DLlzyuYQarMAhFnhGyuexxtRVKevRx4YwWR/W0r7LPHt6oS6DLlzyuYQaLQH2onWFc3UR6I9ZhuHVeJCi5LNAf4APVd7mHn4BhdViHRwu7j4AAACAAgAAACEWgiPY+kcolS1Hp0niOK/+7VHz6F+nsz8JVxnzWzkgToYNAMkRfC4AAACAAgAAAAEXIIIj2PpHKJUtR6dJ4jiv/u1R8+hfp7M/CVcZ81s5IE6GARgg9qJ1hXN1EeiPWYbh1XiQouSzQH+AD1Xe5h5+AYXVYh0AAA==";
        const SIGNED_PSBT: &str = "cHNidP8BAFMBAAAAAWZalxaErOL7P3WPIUc8DsjgE68S+ww+uqiqEI2SAwlPAAAAAAD/////AQiWmAAAAAAAF6kU4R3W8CnGzZcSsaovTYu0X8vHt3WHAAAAAAABASuAlpgAAAAAACJRINa6bLPZwp3/CYWoxyI3mLYcSC5f9LInAMUng94nspa2AQcAAQhCAUALcP9w/+Ddly9DWdhHTnQ9uCDWLPZjR6vKbKePswW2Ee6W5KNfrklus/8z98n7BQ1U4vADHk0FbadeeL8rrbHlARNAC3D/cP/g3ZcvQ1nYR050Pbgg1iz2Y0erymynj7MFthHuluSjX65JbrP/M/fJ+wUNVOLwAx5NBW2nXni/K62x5UEUeEbK57HG1FUp69HHhjBZH9bSvss8e3qhLoMuXPK5hBr2onWFc3UR6I9ZhuHVeJCi5LNAf4APVd7mHn4BhdViHUAXNmWieJ80Fs+PMa2C186YOBPZbYG/ieEUkagMwzJ788SoCucNdp5wnxfpuJVygFhglDrXGzujFtC82PrMohwuIhXBgiPY+kcolS1Hp0niOK/+7VHz6F+nsz8JVxnzWzkgToYjIHhGyuexxtRVKevRx4YwWR/W0r7LPHt6oS6DLlzyuYQarMAhFnhGyuexxtRVKevRx4YwWR/W0r7LPHt6oS6DLlzyuYQaLQH2onWFc3UR6I9ZhuHVeJCi5LNAf4APVd7mHn4BhdViHRwu7j4AAACAAgAAACEWgiPY+kcolS1Hp0niOK/+7VHz6F+nsz8JVxnzWzkgToYNAMkRfC4AAACAAgAAAAEXIIIj2PpHKJUtR6dJ4jiv/u1R8+hfp7M/CVcZ81s5IE6GARgg9qJ1hXN1EeiPWYbh1XiQouSzQH+AD1Xe5h5+AYXVYh0AAA==";

        let unsigned_psbt = Psbt::from_str(UNSIGNED_PSBT).unwrap();
        let signed_psbt = Psbt::from_str(SIGNED_PSBT).unwrap();

        let secp = Secp256k1::new();

        let (_, alice_pub, _) = setup_keys(ALICE_TPRV_STR, ALICE_BOB_PATH, &secp);
        let (_, bob_pub, _) = setup_keys(BOB_TPRV_STR, ALICE_BOB_PATH, &secp);

        let desc = descriptor!(tr(bob_pub, pk(alice_pub))).unwrap();
        let (wallet_desc, _) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();

        let policy_unsigned = wallet_desc
            .extract_policy(
                &SignersContainer::default(),
                BuildSatisfaction::Psbt(&unsigned_psbt),
                &secp,
            )
            .unwrap()
            .unwrap();
        let policy_signed = wallet_desc
            .extract_policy(
                &SignersContainer::default(),
                BuildSatisfaction::Psbt(&signed_psbt),
                &secp,
            )
            .unwrap()
            .unwrap();

        assert_matches!(policy_unsigned.item, SatisfiableItem::Thresh { ref items, threshold: 1 } if items.len() == 2);
        assert_matches!(policy_unsigned.satisfaction, Satisfaction::Partial { n: 2, m: 1, items, .. } if items.is_empty());

        assert_matches!(policy_signed.item, SatisfiableItem::Thresh { ref items, threshold: 1 } if items.len() == 2);
        assert_matches!(policy_signed.satisfaction, Satisfaction::PartialComplete { n: 2, m: 1, items, .. } if items == vec![0, 1]);

        let satisfied_items = match policy_signed.item {
            SatisfiableItem::Thresh { items, .. } => items,
            _ => unreachable!(),
        };

        assert_eq!(
            satisfied_items[0].satisfaction,
            Satisfaction::Complete {
                condition: Default::default()
            }
        );
        assert_eq!(
            satisfied_items[1].satisfaction,
            Satisfaction::Complete {
                condition: Default::default()
            }
        );
    }
}
