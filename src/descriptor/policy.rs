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
//! # use bdk::bitcoin::secp256k1::Secp256k1;
//! let secp = Secp256k1::new();
//! let desc = "wsh(and_v(v:pk(cV3oCth6zxZ1UVsHLnGothsWNsaoxRhC6aeNi5VbSdFpwUkgkEci),or_d(pk(cVMTy7uebJgvFaSBwcgvwk8qn8xSLc97dKow4MBetjrrahZoimm2),older(12960))))";
//!
//! let (extended_desc, key_map) = ExtendedDescriptor::parse_descriptor(&secp, desc)?;
//! println!("{:?}", extended_desc);
//!
//! let signers = Arc::new(key_map.into());
//! let policy = extended_desc.extract_policy(&signers, &secp)?;
//! println!("policy: {}", serde_json::to_string(&policy)?);
//! # Ok::<(), bdk::Error>(())
//! ```

use std::cmp::max;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fmt;

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use bitcoin::hashes::*;
use bitcoin::util::bip32::Fingerprint;
use bitcoin::PublicKey;

use miniscript::descriptor::{DescriptorPublicKey, ShInner, SortedMultiVec, WshInner};
use miniscript::{Descriptor, Miniscript, MiniscriptKey, ScriptContext, Terminal, ToPublicKey};

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use crate::descriptor::{DerivedDescriptorKey, ExtractPolicy};
use crate::wallet::signer::{SignerId, SignersContainer};
use crate::wallet::utils::{self, SecpCtx};

use super::checksum::get_checksum;
use super::error::Error;
use super::XKeyUtils;

/// Raw public key or extended key fingerprint
#[derive(Debug, Clone, Default, Serialize)]
pub struct PKOrF {
    #[serde(skip_serializing_if = "Option::is_none")]
    pubkey: Option<PublicKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pubkey_hash: Option<hash160::Hash>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fingerprint: Option<Fingerprint>,
}

impl PKOrF {
    fn from_key(k: &DescriptorPublicKey, secp: &SecpCtx) -> Self {
        match k {
            DescriptorPublicKey::SinglePub(pubkey) => PKOrF {
                pubkey: Some(pubkey.key),
                ..Default::default()
            },
            DescriptorPublicKey::XPub(xpub) => PKOrF {
                fingerprint: Some(xpub.root_fingerprint(secp)),
                ..Default::default()
            },
        }
    }

    fn from_key_hash(k: hash160::Hash) -> Self {
        PKOrF {
            pubkey_hash: Some(k),
            ..Default::default()
        }
    }
}

/// An item that needs to be satisfied
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "UPPERCASE")]
pub enum SatisfiableItem {
    // Leaves
    /// Signature for a raw public key
    Signature(PKOrF),
    /// Signature for an extended key fingerprint
    SignatureKey(PKOrF),
    /// SHA256 preimage hash
    SHA256Preimage {
        /// The digest value
        hash: sha256::Hash,
    },
    /// Double SHA256 preimage hash
    HASH256Preimage {
        /// The digest value
        hash: sha256d::Hash,
    },
    /// RIPEMD160 preimage hash
    RIPEMD160Preimage {
        /// The digest value
        hash: ripemd160::Hash,
    },
    /// SHA256 then RIPEMD160 preimage hash
    HASH160Preimage {
        /// The digest value
        hash: hash160::Hash,
    },
    /// Absolute timeclock timestamp
    AbsoluteTimelock {
        /// The timestamp value
        value: u32,
    },
    /// Relative timelock locktime
    RelativeTimelock {
        /// The locktime value
        value: u32,
    },
    /// Multi-signature public keys with threshold count
    Multisig {
        /// The raw public key or extended key fingerprint
        keys: Vec<PKOrF>,
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
        get_checksum(&serde_json::to_string(self).expect("Failed to serialize a SatisfiableItem"))
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
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "UPPERCASE")]
pub enum Satisfaction {
    /// Only a partial satisfaction of some kind of threshold policy
    Partial {
        /// Total number of items
        n: usize,
        /// Threshold
        m: usize,
        /// The items that can be satisfied by the descriptor
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
                            .fold(HashSet::new(), |set, i| set.union(&i).cloned().collect());
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
                    // we map each of the combinations of elements into a tuple of ([choosen items], [conditions]). unfortunately, those items have potentially more than one
                    // condition (think about ORs), so we also use `mix` to expand those, i.e. [[0], [1, 2]] becomes [[0, 1], [0, 2]]. This is necessary to make sure that we
                    // consider every possibile options and check whether or not they are compatible.
                    .map(|i_vec| {
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
                    // .inspect(|x: &Vec<(Vec<usize>, Vec<Condition>)>| println!("fetch {:?}", x))
                    // since the previous step can turn one item of the iterator into multiple ones, we call flatten to expand them out
                    .flatten()
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
#[derive(Debug, Clone, Serialize)]
pub struct Policy {
    /// Identifier for this policy node
    pub id: String,

    /// Type of this policy node
    #[serde(flatten)]
    pub item: SatisfiableItem,
    /// How a much given PSBT already satisfies this polcy node **(currently unused)**
    pub satisfaction: Satisfaction,
    /// How the wallet's descriptor can satisfy this policy node
    pub contribution: Satisfaction,
}

/// An extra condition that must be satisfied but that is out of control of the user
#[derive(Hash, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Serialize)]
pub struct Condition {
    /// Optional CheckSequenceVerify condition
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csv: Option<u32>,
    /// Optional timelock condition
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timelock: Option<u32>,
}

impl Condition {
    fn merge_nlocktime(a: u32, b: u32) -> Result<u32, PolicyError> {
        if (a < utils::BLOCKS_TIMELOCK_THRESHOLD) != (b < utils::BLOCKS_TIMELOCK_THRESHOLD) {
            Err(PolicyError::MixedTimelockUnits)
        } else {
            Ok(max(a, b))
        }
    }

    fn merge_nsequence(a: u32, b: u32) -> Result<u32, PolicyError> {
        let mask = utils::SEQUENCE_LOCKTIME_TYPE_FLAG | utils::SEQUENCE_LOCKTIME_MASK;

        let a = a & mask;
        let b = b & mask;

        if (a < utils::SEQUENCE_LOCKTIME_TYPE_FLAG) != (b < utils::SEQUENCE_LOCKTIME_TYPE_FLAG) {
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
        for (index, item) in items.iter().enumerate() {
            contribution.add(&item.contribution, index)?;
        }
        contribution.finalize();

        let mut policy: Policy = SatisfiableItem::Thresh { items, threshold }.into();
        policy.contribution = contribution;

        Ok(Some(policy))
    }

    fn make_multisig(
        keys: &[DescriptorPublicKey],
        signers: &SignersContainer,
        threshold: usize,
        sorted: bool,
        secp: &SecpCtx,
    ) -> Result<Option<Policy>, PolicyError> {
        if threshold == 0 {
            return Ok(None);
        }

        let parsed_keys = keys.iter().map(|k| PKOrF::from_key(k, secp)).collect();

        let mut contribution = Satisfaction::Partial {
            n: keys.len(),
            m: threshold,
            items: vec![],
            conditions: Default::default(),
            sorted: Some(sorted),
        };
        for (index, key) in keys.iter().enumerate() {
            if signers.find(signer_id(key, secp)).is_some() {
                contribution.add(
                    &Satisfaction::Complete {
                        condition: Default::default(),
                    },
                    index,
                )?;
            }
        }
        contribution.finalize();

        let mut policy: Policy = SatisfiableItem::Multisig {
            keys: parsed_keys,
            threshold,
        }
        .into();
        policy.contribution = contribution;

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
    match key {
        DescriptorPublicKey::SinglePub(pubkey) => pubkey.key.to_pubkeyhash().into(),
        DescriptorPublicKey::XPub(xpub) => xpub.root_fingerprint(secp).into(),
    }
}

fn signature(key: &DescriptorPublicKey, signers: &SignersContainer, secp: &SecpCtx) -> Policy {
    let mut policy: Policy = SatisfiableItem::Signature(PKOrF::from_key(key, secp)).into();

    policy.contribution = if signers.find(signer_id(key, secp)).is_some() {
        Satisfaction::Complete {
            condition: Default::default(),
        }
    } else {
        Satisfaction::None
    };

    policy
}

fn signature_key(
    key: &<DescriptorPublicKey as MiniscriptKey>::Hash,
    signers: &SignersContainer,
    secp: &SecpCtx,
) -> Policy {
    let key_hash = DerivedDescriptorKey::new(key.clone(), secp)
        .to_public_key()
        .to_pubkeyhash();
    let mut policy: Policy = SatisfiableItem::Signature(PKOrF::from_key_hash(key_hash)).into();

    if signers.find(SignerId::PkHash(key_hash)).is_some() {
        policy.contribution = Satisfaction::Complete {
            condition: Default::default(),
        }
    }

    policy
}

impl<Ctx: ScriptContext> ExtractPolicy for Miniscript<DescriptorPublicKey, Ctx> {
    fn extract_policy(
        &self,
        signers: &SignersContainer,
        secp: &SecpCtx,
    ) -> Result<Option<Policy>, Error> {
        Ok(match &self.node {
            // Leaves
            Terminal::True | Terminal::False => None,
            Terminal::PkK(pubkey) => Some(signature(pubkey, signers, secp)),
            Terminal::PkH(pubkey_hash) => Some(signature_key(pubkey_hash, signers, secp)),
            Terminal::After(value) => {
                let mut policy: Policy = SatisfiableItem::AbsoluteTimelock { value: *value }.into();
                policy.contribution = Satisfaction::Complete {
                    condition: Condition {
                        timelock: Some(*value),
                        csv: None,
                    },
                };

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

                Some(policy)
            }
            Terminal::Sha256(hash) => Some(SatisfiableItem::SHA256Preimage { hash: *hash }.into()),
            Terminal::Hash256(hash) => {
                Some(SatisfiableItem::HASH256Preimage { hash: *hash }.into())
            }
            Terminal::Ripemd160(hash) => {
                Some(SatisfiableItem::RIPEMD160Preimage { hash: *hash }.into())
            }
            Terminal::Hash160(hash) => {
                Some(SatisfiableItem::HASH160Preimage { hash: *hash }.into())
            }
            Terminal::Multi(k, pks) => Policy::make_multisig(pks, signers, *k, false, secp)?,
            // Identities
            Terminal::Alt(inner)
            | Terminal::Swap(inner)
            | Terminal::Check(inner)
            | Terminal::DupIf(inner)
            | Terminal::Verify(inner)
            | Terminal::NonZero(inner)
            | Terminal::ZeroNotEqual(inner) => inner.extract_policy(signers, secp)?,
            // Complex policies
            Terminal::AndV(a, b) | Terminal::AndB(a, b) => Policy::make_and(
                a.extract_policy(signers, secp)?,
                b.extract_policy(signers, secp)?,
            )?,
            Terminal::AndOr(x, y, z) => Policy::make_or(
                Policy::make_and(
                    x.extract_policy(signers, secp)?,
                    y.extract_policy(signers, secp)?,
                )?,
                z.extract_policy(signers, secp)?,
            )?,
            Terminal::OrB(a, b)
            | Terminal::OrD(a, b)
            | Terminal::OrC(a, b)
            | Terminal::OrI(a, b) => Policy::make_or(
                a.extract_policy(signers, secp)?,
                b.extract_policy(signers, secp)?,
            )?,
            Terminal::Thresh(k, nodes) => {
                let mut threshold = *k;
                let mapped: Vec<_> = nodes
                    .iter()
                    .map(|n| n.extract_policy(signers, secp))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .filter_map(|x| x)
                    .collect();

                if mapped.len() < nodes.len() {
                    threshold = match threshold.checked_sub(nodes.len() - mapped.len()) {
                        None => return Ok(None),
                        Some(x) => x,
                    };
                }

                Policy::make_thresh(mapped, threshold)?
            }
        })
    }
}

impl ExtractPolicy for Descriptor<DescriptorPublicKey> {
    fn extract_policy(
        &self,
        signers: &SignersContainer,
        secp: &SecpCtx,
    ) -> Result<Option<Policy>, Error> {
        fn make_sortedmulti<Ctx: ScriptContext>(
            keys: &SortedMultiVec<DescriptorPublicKey, Ctx>,
            signers: &SignersContainer,
            secp: &SecpCtx,
        ) -> Result<Option<Policy>, Error> {
            Ok(Policy::make_multisig(
                keys.pks.as_ref(),
                signers,
                keys.k,
                true,
                secp,
            )?)
        }

        match self {
            Descriptor::Pkh(pk) => Ok(Some(signature(pk.as_inner(), signers, secp))),
            Descriptor::Wpkh(pk) => Ok(Some(signature(pk.as_inner(), signers, secp))),
            Descriptor::Sh(sh) => match sh.as_inner() {
                ShInner::Wpkh(pk) => Ok(Some(signature(pk.as_inner(), signers, secp))),
                ShInner::Ms(ms) => Ok(ms.extract_policy(signers, secp)?),
                ShInner::SortedMulti(ref keys) => make_sortedmulti(keys, signers, secp),
                ShInner::Wsh(wsh) => match wsh.as_inner() {
                    WshInner::Ms(ms) => Ok(ms.extract_policy(signers, secp)?),
                    WshInner::SortedMulti(ref keys) => make_sortedmulti(keys, signers, secp),
                },
            },
            Descriptor::Wsh(wsh) => match wsh.as_inner() {
                WshInner::Ms(ms) => Ok(ms.extract_policy(signers, secp)?),
                WshInner::SortedMulti(ref keys) => make_sortedmulti(keys, signers, secp),
            },
            Descriptor::Bare(ms) => Ok(ms.as_inner().extract_policy(signers, secp)?),
        }
    }
}

#[cfg(test)]
mod test {
    use crate::descriptor;
    use crate::descriptor::{ExtractPolicy, IntoWalletDescriptor};

    use super::*;
    use crate::descriptor::policy::SatisfiableItem::{Multisig, Signature, Thresh};
    use crate::keys::{DescriptorKey, IntoDescriptorKey};
    use crate::wallet::signer::SignersContainer;
    use bitcoin::secp256k1::{All, Secp256k1};
    use bitcoin::util::bip32;
    use bitcoin::Network;
    use std::str::FromStr;
    use std::sync::Arc;

    const TPRV0_STR:&str = "tprv8ZgxMBicQKsPdZXrcHNLf5JAJWFAoJ2TrstMRdSKtEggz6PddbuSkvHKM9oKJyFgZV1B7rw8oChspxyYbtmEXYyg1AjfWbL3ho3XHDpHRZf";
    const TPRV1_STR:&str = "tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcLNfjdi5qUvw3VDfgYiH5mNsj5izuiu2N";

    const PATH: &str = "m/44'/1'/0'/0";

    fn setup_keys<Ctx: ScriptContext>(
        tprv: &str,
    ) -> (DescriptorKey<Ctx>, DescriptorKey<Ctx>, Fingerprint) {
        let secp: Secp256k1<All> = Secp256k1::new();
        let path = bip32::DerivationPath::from_str(PATH).unwrap();
        let tprv = bip32::ExtendedPrivKey::from_str(tprv).unwrap();
        let tpub = bip32::ExtendedPubKey::from_private(&secp, &tprv);
        let fingerprint = tprv.fingerprint(&secp);
        let prvkey = (tprv, path.clone()).into_descriptor_key().unwrap();
        let pubkey = (tpub, path).into_descriptor_key().unwrap();

        (prvkey, pubkey, fingerprint)
    }

    // test ExtractPolicy trait for simple descriptors; wpkh(), sh(multi())

    #[test]
    fn test_extract_policy_for_wpkh() {
        let secp = Secp256k1::new();

        let (prvkey, pubkey, fingerprint) = setup_keys(TPRV0_STR);
        let desc = descriptor!(wpkh(pubkey)).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::from(keymap));
        let policy = wallet_desc
            .extract_policy(&signers_container, &Secp256k1::new())
            .unwrap()
            .unwrap();

        assert!(
            matches!(&policy.item, Signature(pk_or_f) if pk_or_f.fingerprint.unwrap() == fingerprint)
        );
        assert!(matches!(&policy.contribution, Satisfaction::None));

        let desc = descriptor!(wpkh(prvkey)).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::from(keymap));
        let policy = wallet_desc
            .extract_policy(&signers_container, &Secp256k1::new())
            .unwrap()
            .unwrap();

        assert!(
            matches!(&policy.item, Signature(pk_or_f) if pk_or_f.fingerprint.unwrap() == fingerprint)
        );
        assert!(
            matches!(&policy.contribution, Satisfaction::Complete {condition} if condition.csv == None && condition.timelock == None)
        );
    }

    // 2 pub keys descriptor, required 2 prv keys
    // #[test]
    // fn test_extract_policy_for_sh_multi_partial_0of2() {
    //     let (_prvkey0, pubkey0, fingerprint0) = setup_keys(TPRV0_STR);
    //     let (_prvkey1, pubkey1, fingerprint1) = setup_keys(TPRV1_STR);
    //     let desc = descriptor!(sh(multi 2, pubkey0, pubkey1)).unwrap();
    //     let (wallet_desc, keymap) = desc.to_wallet_descriptor(Network::Testnet).unwrap();
    //     let signers_container = Arc::new(SignersContainer::from(keymap));
    //     let policy = wallet_desc
    //         .extract_policy(signers_container)
    //         .unwrap()
    //         .unwrap();
    //
    //     assert!(
    //         matches!(&policy.item, Multisig { keys, threshold } if threshold == &2
    //         && &keys[0].fingerprint.unwrap() == &fingerprint0
    //         && &keys[1].fingerprint.unwrap() == &fingerprint1)
    //     );
    //
    //     // TODO should this be "Satisfaction::None" since we have no prv keys?
    //     // TODO should items and conditions not be empty?
    //     assert!(
    //         matches!(&policy.contribution, Satisfaction::Partial { n, m, items, conditions} if n == &2
    //         && m == &2
    //         && items.is_empty()
    //         && conditions.is_empty()
    //         )
    //     );
    // }

    // 1 prv and 1 pub key descriptor, required 2 prv keys
    // #[test]
    // fn test_extract_policy_for_sh_multi_partial_1of2() {
    //     let (prvkey0, _pubkey0, fingerprint0) = setup_keys(TPRV0_STR);
    //     let (_prvkey1, pubkey1, fingerprint1) = setup_keys(TPRV1_STR);
    //     let desc = descriptor!(sh(multi 2, prvkey0, pubkey1)).unwrap();
    //     let (wallet_desc, keymap) = desc.to_wallet_descriptor(Network::Testnet).unwrap();
    //     let signers_container = Arc::new(SignersContainer::from(keymap));
    //     let policy = wallet_desc
    //         .extract_policy(signers_container)
    //         .unwrap()
    //         .unwrap();
    //
    //     assert!(
    //         matches!(&policy.item, Multisig { keys, threshold } if threshold == &2
    //         && &keys[0].fingerprint.unwrap() == &fingerprint0
    //         && &keys[1].fingerprint.unwrap() == &fingerprint1)
    //     );
    //
    //     // TODO should this be "Satisfaction::Partial" since we have only one of two prv keys?
    //     assert!(
    //         matches!(&policy.contribution, Satisfaction::PartialComplete { n, m, items, conditions} if n == &2
    //          && m == &2
    //          && items.len() == 2
    //          && conditions.contains_key(&vec![0,1])
    //         )
    //     );
    // }

    // 1 prv and 1 pub key descriptor, required 1 prv keys
    #[test]
    #[ignore] // see https://github.com/bitcoindevkit/bdk/issues/225
    fn test_extract_policy_for_sh_multi_complete_1of2() {
        let secp = Secp256k1::new();

        let (_prvkey0, pubkey0, fingerprint0) = setup_keys(TPRV0_STR);
        let (prvkey1, _pubkey1, fingerprint1) = setup_keys(TPRV1_STR);
        let desc = descriptor!(sh(multi(1, pubkey0, prvkey1))).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::from(keymap));
        let policy = wallet_desc
            .extract_policy(&signers_container, &Secp256k1::new())
            .unwrap()
            .unwrap();

        assert!(
            matches!(&policy.item, Multisig { keys, threshold } if threshold == &1
            && keys[0].fingerprint.unwrap() == fingerprint0
            && keys[1].fingerprint.unwrap() == fingerprint1)
        );
        assert!(
            matches!(&policy.contribution, Satisfaction::PartialComplete { n, m, items, conditions, .. } if n == &2
             && m == &1
             && items.len() == 2
             && conditions.contains_key(&vec![0])
             && conditions.contains_key(&vec![1])
            )
        );
    }

    // 2 prv keys descriptor, required 2 prv keys
    #[test]
    fn test_extract_policy_for_sh_multi_complete_2of2() {
        let secp = Secp256k1::new();

        let (prvkey0, _pubkey0, fingerprint0) = setup_keys(TPRV0_STR);
        let (prvkey1, _pubkey1, fingerprint1) = setup_keys(TPRV1_STR);
        let desc = descriptor!(sh(multi(2, prvkey0, prvkey1))).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers_container = Arc::new(SignersContainer::from(keymap));
        let policy = wallet_desc
            .extract_policy(&signers_container, &Secp256k1::new())
            .unwrap()
            .unwrap();

        assert!(
            matches!(&policy.item, Multisig { keys, threshold } if threshold == &2
            && keys[0].fingerprint.unwrap() == fingerprint0
            && keys[1].fingerprint.unwrap() == fingerprint1)
        );

        assert!(
            matches!(&policy.contribution, Satisfaction::PartialComplete { n, m, items, conditions, .. } if n == &2
             && m == &2
             && items.len() == 2
             && conditions.contains_key(&vec![0,1])
            )
        );
    }

    // test ExtractPolicy trait with extended and single keys

    #[test]
    fn test_extract_policy_for_single_wpkh() {
        let secp = Secp256k1::new();

        let (prvkey, pubkey, fingerprint) = setup_keys(TPRV0_STR);
        let desc = descriptor!(wpkh(pubkey)).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let single_key = wallet_desc.derive(0);
        let signers_container = Arc::new(SignersContainer::from(keymap));
        let policy = single_key
            .extract_policy(&signers_container, &Secp256k1::new())
            .unwrap()
            .unwrap();

        assert!(
            matches!(&policy.item, Signature(pk_or_f) if pk_or_f.fingerprint.unwrap() == fingerprint)
        );
        assert!(matches!(&policy.contribution, Satisfaction::None));

        let desc = descriptor!(wpkh(prvkey)).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let single_key = wallet_desc.derive(0);
        let signers_container = Arc::new(SignersContainer::from(keymap));
        let policy = single_key
            .extract_policy(&signers_container, &Secp256k1::new())
            .unwrap()
            .unwrap();

        assert!(
            matches!(&policy.item, Signature(pk_or_f) if pk_or_f.fingerprint.unwrap() == fingerprint)
        );
        assert!(
            matches!(&policy.contribution, Satisfaction::Complete {condition} if condition.csv == None && condition.timelock == None)
        );
    }

    // single key, 1 prv and 1 pub key descriptor, required 1 prv keys
    #[test]
    #[ignore] // see https://github.com/bitcoindevkit/bdk/issues/225
    fn test_extract_policy_for_single_wsh_multi_complete_1of2() {
        let secp = Secp256k1::new();

        let (_prvkey0, pubkey0, fingerprint0) = setup_keys(TPRV0_STR);
        let (prvkey1, _pubkey1, fingerprint1) = setup_keys(TPRV1_STR);
        let desc = descriptor!(sh(multi(1, pubkey0, prvkey1))).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let single_key = wallet_desc.derive(0);
        let signers_container = Arc::new(SignersContainer::from(keymap));
        let policy = single_key
            .extract_policy(&signers_container, &Secp256k1::new())
            .unwrap()
            .unwrap();

        assert!(
            matches!(&policy.item, Multisig { keys, threshold } if threshold == &1
            && keys[0].fingerprint.unwrap() == fingerprint0
            && keys[1].fingerprint.unwrap() == fingerprint1)
        );
        assert!(
            matches!(&policy.contribution, Satisfaction::PartialComplete { n, m, items, conditions, .. } if n == &2
             && m == &1
             && items.len() == 2
             && conditions.contains_key(&vec![0])
             && conditions.contains_key(&vec![1])
            )
        );
    }

    // test ExtractPolicy trait with descriptors containing timelocks in a thresh()

    #[test]
    #[ignore] // see https://github.com/bitcoindevkit/bdk/issues/225
    fn test_extract_policy_for_wsh_multi_timelock() {
        let secp = Secp256k1::new();

        let (prvkey0, _pubkey0, _fingerprint0) = setup_keys(TPRV0_STR);
        let (_prvkey1, pubkey1, _fingerprint1) = setup_keys(TPRV1_STR);
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
        let signers_container = Arc::new(SignersContainer::from(keymap));
        let policy = wallet_desc
            .extract_policy(&signers_container, &Secp256k1::new())
            .unwrap()
            .unwrap();

        assert!(
            matches!(&policy.item, Thresh { items, threshold } if items.len() == 3 && threshold == &2)
        );

        assert!(
            matches!(&policy.contribution, Satisfaction::PartialComplete { n, m, items, conditions, .. } if n == &3
             && m == &2
             && items.len() == 3
             && conditions.get(&vec![0,1]).unwrap().iter().next().unwrap().csv.is_none()
             && conditions.get(&vec![0,2]).unwrap().iter().next().unwrap().csv == Some(sequence)
             && conditions.get(&vec![1,2]).unwrap().iter().next().unwrap().csv == Some(sequence)
            )
        );
    }

    // - mixed timelocks should fail

    // #[test]
    // fn test_extract_policy_for_wsh_mixed_timelocks() {
    //     let (prvkey0, _pubkey0, _fingerprint0) = setup_keys(TPRV0_STR);
    //     let locktime_threshold = 500000000; // if less than this means block number, else block time in seconds
    //     let locktime_blocks = 100;
    //     let locktime_seconds = locktime_blocks + locktime_threshold;
    //     let desc = descriptor!(sh (and_v (+v pk prvkey0), (and_v (+v after locktime_seconds), (after locktime_blocks)))).unwrap();
    //     let (wallet_desc, keymap) = desc.to_wallet_descriptor(Network::Testnet).unwrap();
    //     let signers_container = Arc::new(SignersContainer::from(keymap));
    //     let policy = wallet_desc
    //         .extract_policy(signers_container)
    //         .unwrap()
    //         .unwrap();
    //
    //     println!("desc policy = {:?}", policy); // TODO remove
    //
    //     // TODO how should this fail with mixed timelocks?
    // }

    // - multiple timelocks of the same type should be correctly merged together

    // #[test]
    // fn test_extract_policy_for_multiple_same_timelocks() {
    //     let (prvkey0, _pubkey0, _fingerprint0) = setup_keys(TPRV0_STR);
    //     let locktime_blocks0 = 100;
    //     let locktime_blocks1 = 200;
    //     let desc = descriptor!(sh (and_v (+v pk prvkey0), (and_v (+v after locktime_blocks0), (after locktime_blocks1)))).unwrap();
    //     let (wallet_desc, keymap) = desc.to_wallet_descriptor(Network::Testnet).unwrap();
    //     let signers_container = Arc::new(SignersContainer::from(keymap));
    //     let policy = wallet_desc
    //         .extract_policy(signers_container)
    //         .unwrap()
    //         .unwrap();
    //
    //     println!("desc policy = {:?}", policy); // TODO remove
    //
    //     // TODO how should this merge timelocks?
    //
    //     let (prvkey1, _pubkey1, _fingerprint1) = setup_keys(TPRV0_STR);
    //     let locktime_seconds0 = 500000100;
    //     let locktime_seconds1 = 500000200;
    //     let desc = descriptor!(sh (and_v (+v pk prvkey1), (and_v (+v after locktime_seconds0), (after locktime_seconds1)))).unwrap();
    //     let (wallet_desc, keymap) = desc.to_wallet_descriptor(Network::Testnet).unwrap();
    //     let signers_container = Arc::new(SignersContainer::from(keymap));
    //     let policy = wallet_desc
    //         .extract_policy(signers_container)
    //         .unwrap()
    //         .unwrap();
    //
    //     println!("desc policy = {:?}", policy); // TODO remove
    //
    //     // TODO how should this merge timelocks?
    // }

    #[test]
    fn test_get_condition_multisig() {
        let secp = Secp256k1::gen_new();

        let (_, pk0, _) = setup_keys(TPRV0_STR);
        let (_, pk1, _) = setup_keys(TPRV1_STR);

        let desc = descriptor!(wsh(multi(1, pk0, pk1))).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();
        let signers = keymap.into();

        let policy = wallet_desc
            .extract_policy(&signers, &secp)
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
}
