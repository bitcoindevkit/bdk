use std::collections::BTreeMap;

use serde::Serialize;

use bitcoin::hashes::*;
use bitcoin::secp256k1::Secp256k1;
use bitcoin::util::bip32::Fingerprint;
use bitcoin::PublicKey;

use miniscript::{Descriptor, Miniscript, Terminal};

use descriptor::{Key, MiniscriptExtractPolicy};

#[derive(Debug, Serialize)]
pub struct PKOrF {
    #[serde(skip_serializing_if = "Option::is_none")]
    pubkey: Option<PublicKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fingerprint: Option<Fingerprint>,
}

impl PKOrF {
    fn from_key(k: &Box<dyn Key>) -> Self {
        let secp = Secp256k1::gen_new();

        if let Some(fing) = k.fingerprint(&secp) {
            PKOrF {
                fingerprint: Some(fing),
                pubkey: None,
            }
        } else {
            PKOrF {
                fingerprint: None,
                pubkey: Some(k.as_public_key(&secp, None).unwrap()),
            }
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "UPPERCASE")]
pub enum SatisfiableItem {
    // Leaves
    Signature(PKOrF),
    SignatureKey {
        #[serde(skip_serializing_if = "Option::is_none")]
        fingerprint: Option<Fingerprint>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pubkey_hash: Option<hash160::Hash>,
    },
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
        height: u32,
    },
    RelativeTimelock {
        blocks: u32,
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
}

#[derive(Debug, Serialize)]
pub enum ItemSatisfier {
    Us,
    Other(Option<Fingerprint>),
    Timelock(Option<u32>), // remaining blocks. TODO: time-based timelocks
}

#[derive(Debug, Serialize)]
pub struct Policy {
    #[serde(flatten)]
    item: SatisfiableItem,
    #[serde(skip_serializing_if = "Option::is_none")]
    satisfier: Option<ItemSatisfier>,
}

#[derive(Debug, Default)]
pub struct PathRequirements {
    pub csv: Option<u32>,
    pub timelock: Option<u32>,
}

impl PathRequirements {
    pub fn merge(&mut self, other: &Self) -> Result<(), PolicyError> {
        if other.is_null() {
            return Ok(());
        }

        match (self.csv, other.csv) {
            (Some(old), Some(new)) if old != new => Err(PolicyError::DifferentCSV(old, new)),
            _ => {
                self.csv = self.csv.or(other.csv);
                Ok(())
            }
        }?;

        match (self.timelock, other.timelock) {
            (Some(old), Some(new)) if old != new => Err(PolicyError::DifferentTimelock(old, new)),
            _ => {
                self.timelock = self.timelock.or(other.timelock);
                Ok(())
            }
        }?;

        Ok(())
    }

    pub fn is_null(&self) -> bool {
        self.csv.is_none() && self.timelock.is_none()
    }
}

#[derive(Debug)]
pub enum PolicyError {
    NotEnoughItemsSelected(usize),
    TooManyItemsSelected(usize),
    IndexOutOfRange(usize, usize),
    DifferentCSV(u32, u32),
    DifferentTimelock(u32, u32),
}

impl Policy {
    pub fn new(item: SatisfiableItem) -> Self {
        Policy {
            item,
            satisfier: None,
        }
    }

    pub fn make_and(a: Option<Policy>, b: Option<Policy>) -> Option<Policy> {
        match (a, b) {
            (None, None) => None,
            (Some(x), None) | (None, Some(x)) => Some(x),
            (Some(a), Some(b)) => Some(
                SatisfiableItem::Thresh {
                    items: vec![a, b],
                    threshold: 2,
                }
                .into(),
            ),
        }
    }

    pub fn make_or(a: Option<Policy>, b: Option<Policy>) -> Option<Policy> {
        match (a, b) {
            (None, None) => None,
            (Some(x), None) | (None, Some(x)) => Some(x),
            (Some(a), Some(b)) => Some(
                SatisfiableItem::Thresh {
                    items: vec![a, b],
                    threshold: 1,
                }
                .into(),
            ),
        }
    }

    pub fn make_thresh(items: Vec<Policy>, mut threshold: usize) -> Option<Policy> {
        if threshold == 0 {
            return None;
        }
        if threshold > items.len() {
            threshold = items.len();
        }

        Some(SatisfiableItem::Thresh { items, threshold }.into())
    }

    fn make_multisig(pubkeys: Vec<Option<&Box<dyn Key>>>, threshold: usize) -> Option<Policy> {
        let keys = pubkeys
            .into_iter()
            .map(|k| PKOrF::from_key(k.unwrap()))
            .collect();
        Some(SatisfiableItem::Multisig { keys, threshold }.into())
    }

    pub fn requires_path(&self) -> bool {
        self.get_requirements(&vec![]).is_err()
    }

    pub fn get_requirements(
        &self,
        path: &Vec<Vec<usize>>,
    ) -> Result<PathRequirements, PolicyError> {
        self.recursive_get_requirements(path, 0)
    }

    fn recursive_get_requirements(
        &self,
        path: &Vec<Vec<usize>>,
        index: usize,
    ) -> Result<PathRequirements, PolicyError> {
        // if items.len() == threshold, selected can be omitted and we take all of them by default
        let default = match &self.item {
            SatisfiableItem::Thresh { items, threshold } if items.len() == *threshold => {
                (0..*threshold).into_iter().collect()
            }
            _ => vec![],
        };
        let selected = match path.get(index) {
            _ if !default.is_empty() => &default,
            Some(arr) => arr,
            _ => &default,
        };

        match &self.item {
            SatisfiableItem::Thresh { items, threshold } => {
                let mapped_req = items
                    .iter()
                    .map(|i| i.recursive_get_requirements(path, index + 1))
                    .collect::<Result<Vec<_>, _>>()?;

                // if all the requirements are null we don't care about `selected` because there
                // are no requirements
                if mapped_req.iter().all(PathRequirements::is_null) {
                    return Ok(PathRequirements::default());
                }

                // if we have something, make sure we have enough items. note that the user can set
                // an empty value for this step in case of n-of-n, because `selected` is set to all
                // the elements above
                if selected.len() < *threshold {
                    return Err(PolicyError::NotEnoughItemsSelected(index));
                }

                // check the selected items, see if there are conflicting requirements
                let mut requirements = PathRequirements::default();
                for item_index in selected {
                    requirements.merge(
                        mapped_req
                            .get(*item_index)
                            .ok_or(PolicyError::IndexOutOfRange(*item_index, index))?,
                    )?;
                }

                Ok(requirements)
            }
            _ if !selected.is_empty() => Err(PolicyError::TooManyItemsSelected(index)),
            SatisfiableItem::AbsoluteTimelock { height } => Ok(PathRequirements {
                csv: None,
                timelock: Some(*height),
            }),
            SatisfiableItem::RelativeTimelock { blocks } => Ok(PathRequirements {
                csv: Some(*blocks),
                timelock: None,
            }),
            _ => Ok(PathRequirements::default()),
        }
    }
}

impl From<SatisfiableItem> for Policy {
    fn from(other: SatisfiableItem) -> Self {
        Self::new(other)
    }
}

pub trait ExtractPolicy {
    fn extract_policy(&self) -> Option<Policy>;
}

fn signature_from_string(key: Option<&Box<dyn Key>>) -> Option<Policy> {
    key.map(|k| SatisfiableItem::Signature(PKOrF::from_key(k)).into())
}

fn signature_key_from_string(key: Option<&Box<dyn Key>>) -> Option<Policy> {
    let secp = Secp256k1::gen_new();

    key.map(|k| {
        if let Some(fing) = k.fingerprint(&secp) {
            SatisfiableItem::SignatureKey {
                fingerprint: Some(fing),
                pubkey_hash: None,
            }
        } else {
            SatisfiableItem::SignatureKey {
                fingerprint: None,
                pubkey_hash: Some(hash160::Hash::hash(
                    &k.as_public_key(&secp, None).unwrap().to_bytes(),
                )),
            }
        }
        .into()
    })
}

impl MiniscriptExtractPolicy for Miniscript<String> {
    fn extract_policy(&self, lookup_map: &BTreeMap<String, Box<dyn Key>>) -> Option<Policy> {
        match &self.node {
            // Leaves
            Terminal::True | Terminal::False => None,
            Terminal::Pk(pubkey) => signature_from_string(lookup_map.get(pubkey)),
            Terminal::PkH(pubkey_hash) => signature_key_from_string(lookup_map.get(pubkey_hash)),
            Terminal::After(height) => {
                Some(SatisfiableItem::AbsoluteTimelock { height: *height }.into())
            }
            Terminal::Older(blocks) => {
                Some(SatisfiableItem::RelativeTimelock { blocks: *blocks }.into())
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
            // Identities
            Terminal::Alt(inner)
            | Terminal::Swap(inner)
            | Terminal::Check(inner)
            | Terminal::DupIf(inner)
            | Terminal::Verify(inner)
            | Terminal::NonZero(inner)
            | Terminal::ZeroNotEqual(inner) => inner.extract_policy(lookup_map),
            // Complex policies
            Terminal::AndV(a, b) | Terminal::AndB(a, b) => {
                Policy::make_and(a.extract_policy(lookup_map), b.extract_policy(lookup_map))
            }
            Terminal::AndOr(x, y, z) => Policy::make_or(
                Policy::make_and(x.extract_policy(lookup_map), y.extract_policy(lookup_map)),
                z.extract_policy(lookup_map),
            ),
            Terminal::OrB(a, b)
            | Terminal::OrD(a, b)
            | Terminal::OrC(a, b)
            | Terminal::OrI(a, b) => {
                Policy::make_or(a.extract_policy(lookup_map), b.extract_policy(lookup_map))
            }
            Terminal::Thresh(k, nodes) => {
                let mut threshold = *k;
                let mapped: Vec<_> = nodes
                    .iter()
                    .filter_map(|n| n.extract_policy(lookup_map))
                    .collect();

                if mapped.len() < nodes.len() {
                    threshold = match threshold.checked_sub(nodes.len() - mapped.len()) {
                        None => return None,
                        Some(x) => x,
                    };
                }

                Policy::make_thresh(mapped, threshold)
            }
            Terminal::ThreshM(k, pks) => {
                Policy::make_multisig(pks.iter().map(|s| lookup_map.get(s)).collect(), *k)
            }
        }
    }
}

impl MiniscriptExtractPolicy for Descriptor<String> {
    fn extract_policy(&self, lookup_map: &BTreeMap<String, Box<dyn Key>>) -> Option<Policy> {
        match self {
            Descriptor::Pk(pubkey)
            | Descriptor::Pkh(pubkey)
            | Descriptor::Wpkh(pubkey)
            | Descriptor::ShWpkh(pubkey) => signature_from_string(lookup_map.get(pubkey)),
            Descriptor::Bare(inner)
            | Descriptor::Sh(inner)
            | Descriptor::Wsh(inner)
            | Descriptor::ShWsh(inner) => inner.extract_policy(lookup_map),
        }
    }
}
