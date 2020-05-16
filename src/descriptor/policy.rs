use std::cmp::max;
use std::collections::{BTreeMap, HashSet, VecDeque};

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use bitcoin::hashes::*;
use bitcoin::secp256k1::Secp256k1;
use bitcoin::util::bip32::Fingerprint;
use bitcoin::PublicKey;

use miniscript::{Descriptor, Miniscript, Terminal};

#[allow(unused_imports)]
use log::{debug, error, info, trace};

use super::checksum::get_checksum;
use super::error::Error;
use crate::descriptor::{Key, MiniscriptExtractPolicy};
use crate::psbt::PSBTSatisfier;

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
    fn from_key(k: &Box<dyn Key>) -> Self {
        let secp = Secp256k1::gen_new();

        let pubkey = k.as_public_key(&secp, None).unwrap();
        if let Some(fing) = k.fingerprint(&secp) {
            PKOrF {
                fingerprint: Some(fing),
                ..Default::default()
            }
        } else {
            PKOrF {
                pubkey: Some(pubkey),
                ..Default::default()
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "UPPERCASE")]
pub enum SatisfiableItem {
    // Leaves
    Signature(PKOrF),
    SignatureKey(PKOrF),
    SHA256Preimage {
        hash: sha256::Hash,
    },
    HASH256Preimage {
        hash: sha256d::Hash,
    },
    RIPEMD160Preimage {
        hash: ripemd160::Hash,
    },
    HASH160Preimage {
        hash: hash160::Hash,
    },
    AbsoluteTimelock {
        value: u32,
    },
    RelativeTimelock {
        value: u32,
    },

    // Complex item
    Thresh {
        items: Vec<Policy>,
        threshold: usize,
    },
    Multisig {
        keys: Vec<PKOrF>,
        threshold: usize,
    },
}

impl SatisfiableItem {
    pub fn is_leaf(&self) -> bool {
        match self {
            SatisfiableItem::Thresh {
                items: _,
                threshold: _,
            } => false,
            _ => true,
        }
    }

    pub fn id(&self) -> String {
        get_checksum(&serde_json::to_string(self).expect("Failed to serialize a SatisfiableItem"))
            .expect("Failed to compute a SatisfiableItem id")
    }
}

fn combinations(vec: &Vec<usize>, size: usize) -> Vec<Vec<usize>> {
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

pub type ConditionMap = BTreeMap<usize, HashSet<Condition>>;
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

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "UPPERCASE")]
pub enum Satisfaction {
    Partial {
        n: usize,
        m: usize,
        items: Vec<usize>,
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        conditions: ConditionMap,
    },
    PartialComplete {
        n: usize,
        m: usize,
        items: Vec<usize>,
        #[serde(
            serialize_with = "serialize_folded_cond_map",
            skip_serializing_if = "BTreeMap::is_empty"
        )]
        conditions: FoldedConditionMap,
    },

    Complete {
        condition: Condition,
    },
    None,
}

impl Satisfaction {
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

    fn finalize(&mut self) -> Result<(), PolicyError> {
        // if partial try to bump it to a partialcomplete
        if let Satisfaction::Partial {
            n,
            m,
            items,
            conditions,
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
                                    .and_then(|set| Some(set.clone().into_iter().collect()))
                                    .unwrap_or(vec![])
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
                    // try to fold all the conditions for this specific combination of indexes/options. if they are not compatibile, try_fold will be Err
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
                };
            }
        }

        Ok(())
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

#[derive(Debug, Clone, Serialize)]
pub struct Policy {
    id: String,

    #[serde(flatten)]
    item: SatisfiableItem,
    satisfaction: Satisfaction,
    contribution: Satisfaction,
}

#[derive(Hash, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Serialize)]
pub struct Condition {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csv: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timelock: Option<u32>,
}

impl Condition {
    fn merge_timelock(a: u32, b: u32) -> Result<u32, PolicyError> {
        const BLOCKS_TIMELOCK_THRESHOLD: u32 = 500000000;

        if (a < BLOCKS_TIMELOCK_THRESHOLD) != (b < BLOCKS_TIMELOCK_THRESHOLD) {
            Err(PolicyError::MixedTimelockUnits)
        } else {
            Ok(max(a, b))
        }
    }

    fn merge(mut self, other: &Condition) -> Result<Self, PolicyError> {
        match (self.csv, other.csv) {
            (Some(a), Some(b)) => self.csv = Some(Self::merge_timelock(a, b)?),
            (None, any) => self.csv = any,
            _ => {}
        }

        match (self.timelock, other.timelock) {
            (Some(a), Some(b)) => self.timelock = Some(Self::merge_timelock(a, b)?),
            (None, any) => self.timelock = any,
            _ => {}
        }

        Ok(self)
    }

    pub fn is_null(&self) -> bool {
        self.csv.is_none() && self.timelock.is_none()
    }
}

#[derive(Debug)]
pub enum PolicyError {
    NotEnoughItemsSelected(String),
    TooManyItemsSelected(String),
    IndexOutOfRange(usize),
    AddOnLeaf,
    AddOnPartialComplete,
    MixedTimelockUnits,
    IncompatibleConditions,
}

impl Policy {
    pub fn new(item: SatisfiableItem) -> Self {
        Policy {
            id: item.id(),
            item,
            satisfaction: Satisfaction::None,
            contribution: Satisfaction::None,
        }
    }

    pub fn make_and(a: Option<Policy>, b: Option<Policy>) -> Result<Option<Policy>, PolicyError> {
        match (a, b) {
            (None, None) => Ok(None),
            (Some(x), None) | (None, Some(x)) => Ok(Some(x)),
            (Some(a), Some(b)) => Self::make_thresh(vec![a, b], 2),
        }
    }

    pub fn make_or(a: Option<Policy>, b: Option<Policy>) -> Result<Option<Policy>, PolicyError> {
        match (a, b) {
            (None, None) => Ok(None),
            (Some(x), None) | (None, Some(x)) => Ok(Some(x)),
            (Some(a), Some(b)) => Self::make_thresh(vec![a, b], 1),
        }
    }

    pub fn make_thresh(
        items: Vec<Policy>,
        threshold: usize,
    ) -> Result<Option<Policy>, PolicyError> {
        if threshold == 0 {
            return Ok(None);
        }

        let mut contribution = Satisfaction::Partial {
            n: items.len(),
            m: threshold,
            items: vec![],
            conditions: Default::default(),
        };
        for (index, item) in items.iter().enumerate() {
            contribution.add(&item.contribution, index)?;
        }
        contribution.finalize()?;

        let mut policy: Policy = SatisfiableItem::Thresh { items, threshold }.into();
        policy.contribution = contribution;

        Ok(Some(policy))
    }

    fn make_multisig(
        keys: Vec<Option<&Box<dyn Key>>>,
        threshold: usize,
    ) -> Result<Option<Policy>, PolicyError> {
        if threshold == 0 {
            return Ok(None);
        }

        let parsed_keys = keys.iter().map(|k| PKOrF::from_key(k.unwrap())).collect();

        let mut contribution = Satisfaction::Partial {
            n: keys.len(),
            m: threshold,
            items: vec![],
            conditions: Default::default(),
        };
        for (index, key) in keys.iter().enumerate() {
            let val = if key.is_some() && key.unwrap().has_secret() {
                Satisfaction::Complete {
                    condition: Default::default(),
                }
            } else {
                Satisfaction::None
            };
            contribution.add(&val, index)?;
        }
        contribution.finalize()?;

        let mut policy: Policy = SatisfiableItem::Multisig {
            keys: parsed_keys,
            threshold,
        }
        .into();
        policy.contribution = contribution;

        Ok(Some(policy))
    }

    pub fn satisfy(&mut self, _satisfier: &PSBTSatisfier, _desc_node: &Terminal<PublicKey>) {
        //self.satisfaction = self.item.satisfy(satisfier, desc_node);
        //self.contribution += &self.satisfaction;
    }

    pub fn requires_path(&self) -> bool {
        self.get_requirements(&BTreeMap::new()).is_err()
    }

    pub fn get_requirements(
        &self,
        path: &BTreeMap<String, Vec<usize>>,
    ) -> Result<Condition, PolicyError> {
        // if items.len() == threshold, selected can be omitted and we take all of them by default
        let default = match &self.item {
            SatisfiableItem::Thresh { items, threshold } if items.len() == *threshold => {
                (0..*threshold).into_iter().collect()
            }
            _ => vec![],
        };
        let selected = match path.get(&self.id) {
            _ if !default.is_empty() => &default,
            Some(arr) => arr,
            _ => &default,
        };

        match &self.item {
            SatisfiableItem::Thresh { items, threshold } => {
                let mapped_req = items
                    .iter()
                    .map(|i| i.get_requirements(path))
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
                } else if selected.len() > *threshold {
                    return Err(PolicyError::TooManyItemsSelected(self.id.clone()));
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
            _ if !selected.is_empty() => Err(PolicyError::TooManyItemsSelected(self.id.clone())),
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

fn signature_from_string(key: Option<&Box<dyn Key>>) -> Option<Policy> {
    key.map(|k| {
        let mut policy: Policy = SatisfiableItem::Signature(PKOrF::from_key(k)).into();
        policy.contribution = if k.has_secret() {
            Satisfaction::Complete {
                condition: Default::default(),
            }
        } else {
            Satisfaction::None
        };

        policy
    })
}

fn signature_key_from_string(key: Option<&Box<dyn Key>>) -> Option<Policy> {
    let secp = Secp256k1::gen_new();

    key.map(|k| {
        let pubkey = k.as_public_key(&secp, None).unwrap();
        let mut policy: Policy = if let Some(fing) = k.fingerprint(&secp) {
            SatisfiableItem::SignatureKey(PKOrF {
                fingerprint: Some(fing),
                ..Default::default()
            })
        } else {
            SatisfiableItem::SignatureKey(PKOrF {
                pubkey_hash: Some(hash160::Hash::hash(&pubkey.to_bytes())),
                ..Default::default()
            })
        }
        .into();
        policy.contribution = if k.has_secret() {
            Satisfaction::Complete {
                condition: Default::default(),
            }
        } else {
            Satisfaction::None
        };

        policy
    })
}

impl MiniscriptExtractPolicy for Miniscript<String> {
    fn extract_policy(
        &self,
        lookup_map: &BTreeMap<String, Box<dyn Key>>,
    ) -> Result<Option<Policy>, Error> {
        Ok(match &self.node {
            // Leaves
            Terminal::True | Terminal::False => None,
            Terminal::Pk(pubkey) => signature_from_string(lookup_map.get(pubkey)),
            Terminal::PkH(pubkey_hash) => signature_key_from_string(lookup_map.get(pubkey_hash)),
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
            Terminal::ThreshM(k, pks) => {
                Policy::make_multisig(pks.iter().map(|s| lookup_map.get(s)).collect(), *k)?
            }
            // Identities
            Terminal::Alt(inner)
            | Terminal::Swap(inner)
            | Terminal::Check(inner)
            | Terminal::DupIf(inner)
            | Terminal::Verify(inner)
            | Terminal::NonZero(inner)
            | Terminal::ZeroNotEqual(inner) => inner.extract_policy(lookup_map)?,
            // Complex policies
            Terminal::AndV(a, b) | Terminal::AndB(a, b) => {
                Policy::make_and(a.extract_policy(lookup_map)?, b.extract_policy(lookup_map)?)?
            }
            Terminal::AndOr(x, y, z) => Policy::make_or(
                Policy::make_and(x.extract_policy(lookup_map)?, y.extract_policy(lookup_map)?)?,
                z.extract_policy(lookup_map)?,
            )?,
            Terminal::OrB(a, b)
            | Terminal::OrD(a, b)
            | Terminal::OrC(a, b)
            | Terminal::OrI(a, b) => {
                Policy::make_or(a.extract_policy(lookup_map)?, b.extract_policy(lookup_map)?)?
            }
            Terminal::Thresh(k, nodes) => {
                let mut threshold = *k;
                let mapped: Vec<_> = nodes
                    .iter()
                    .map(|n| n.extract_policy(lookup_map))
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

impl MiniscriptExtractPolicy for Descriptor<String> {
    fn extract_policy(
        &self,
        lookup_map: &BTreeMap<String, Box<dyn Key>>,
    ) -> Result<Option<Policy>, Error> {
        match self {
            Descriptor::Pk(pubkey)
            | Descriptor::Pkh(pubkey)
            | Descriptor::Wpkh(pubkey)
            | Descriptor::ShWpkh(pubkey) => Ok(signature_from_string(lookup_map.get(pubkey))),
            Descriptor::Bare(inner)
            | Descriptor::Sh(inner)
            | Descriptor::Wsh(inner)
            | Descriptor::ShWsh(inner) => Ok(inner.extract_policy(lookup_map)?),
        }
    }
}
