#![allow(unused)]
#![allow(missing_docs)]
#![allow(clippy::all)] // FIXME
//! A spending plan or *plan* for short is a representation of a particular spending path on a
//! descriptor. This allows us to analayze a choice of spending path without producing any
//! signatures or other witness data for it.
//!
//! To make a plan you provide the descriptor with "assets" like which keys you are able to use, hash
//! pre-images you have access to, the current block height etc.
//!
//! Once you've got a plan it can tell you its expected satisfaction weight which can be useful for
//! doing coin selection. Furthermore it provides which subset of those keys and hash pre-images you
//! will actually need as well as what locktime or sequence number you need to set.
//!
//! Once you've obstained signatures, hash pre-images etc required by the plan, it can create a
//! witness/script_sig for the input.
use bdk_chain::{bitcoin, collections::*, miniscript};
use bitcoin::{
    absolute,
    bip32::{DerivationPath, Fingerprint, KeySource},
    blockdata::script::witness_version::WitnessVersion,
    blockdata::transaction::Sequence,
    ecdsa,
    hashes::{hash160, ripemd160, sha256},
    secp256k1::Secp256k1,
    taproot::{self, LeafVersion, TapLeafHash},
    ScriptBuf, TxIn, Witness,
};
use miniscript::{
    descriptor::{InnerXKey, Tr},
    hash256, DefiniteDescriptorKey, Descriptor, DescriptorPublicKey, ScriptContext, ToPublicKey,
};

pub(crate) fn varint_len(v: usize) -> usize {
    bitcoin::VarInt(v as u64).size() as usize
}

mod plan_impls;
mod requirements;
mod template;
pub use requirements::*;
pub use template::PlanKey;
use template::TemplateItem;

#[derive(Clone, Debug)]
enum TrSpend {
    KeySpend,
    LeafSpend {
        script: ScriptBuf,
        leaf_version: LeafVersion,
    },
}

#[derive(Clone, Debug)]
enum Target {
    Legacy,
    Segwitv0 {
        script_code: ScriptBuf,
    },
    Segwitv1 {
        tr: Tr<DefiniteDescriptorKey>,
        tr_plan: TrSpend,
    },
}

impl Target {}

#[derive(Clone, Debug)]
/// A plan represents a particular spending path for a descriptor.
///
/// See the module level documentation for more info.
pub struct Plan<AK> {
    template: Vec<TemplateItem<AK>>,
    target: Target,
    set_locktime: Option<absolute::LockTime>,
    set_sequence: Option<Sequence>,
}

impl Default for Target {
    fn default() -> Self {
        Target::Legacy
    }
}

#[derive(Clone, Debug, Default)]
/// Signatures and hash pre-images that can be used to complete a plan.
pub struct SatisfactionMaterial {
    /// Schnorr signautres under their keys
    pub schnorr_sigs: BTreeMap<DefiniteDescriptorKey, taproot::Signature>,
    /// ECDSA signatures under their keys
    pub ecdsa_sigs: BTreeMap<DefiniteDescriptorKey, ecdsa::Signature>,
    /// SHA256 pre-images under their images
    pub sha256_preimages: BTreeMap<sha256::Hash, Vec<u8>>,
    /// hash160 pre-images under their images
    pub hash160_preimages: BTreeMap<hash160::Hash, Vec<u8>>,
    /// hash256 pre-images under their images
    pub hash256_preimages: BTreeMap<hash256::Hash, Vec<u8>>,
    /// ripemd160 pre-images under their images
    pub ripemd160_preimages: BTreeMap<ripemd160::Hash, Vec<u8>>,
}

impl<Ak> Plan<Ak>
where
    Ak: Clone,
{
    /// The expected satisfaction weight for the plan if it is completed.
    pub fn expected_weight(&self) -> usize {
        let script_sig_size = match self.target {
            Target::Legacy => unimplemented!(), // self
            // .template
            // .iter()
            // .map(|step| {
            //     let size = step.expected_size();
            //     size + push_opcode_size(size)
            // })
            // .sum()
            Target::Segwitv0 { .. } | Target::Segwitv1 { .. } => 1,
        };
        let witness_elem_sizes: Option<Vec<usize>> = match &self.target {
            Target::Legacy => None,
            Target::Segwitv0 { .. } => Some(
                self.template
                    .iter()
                    .map(|step| step.expected_size())
                    .collect(),
            ),
            Target::Segwitv1 { tr, tr_plan } => {
                let mut witness_elems = self
                    .template
                    .iter()
                    .map(|step| step.expected_size())
                    .collect::<Vec<_>>();

                if let TrSpend::LeafSpend {
                    script,
                    leaf_version,
                } = tr_plan
                {
                    let control_block = tr
                        .spend_info()
                        .control_block(&(script.clone(), *leaf_version))
                        .expect("must exist");
                    witness_elems.push(script.len());
                    witness_elems.push(control_block.size());
                }

                Some(witness_elems)
            }
        };

        let witness_size: usize = match witness_elem_sizes {
            Some(elems) => {
                varint_len(elems.len())
                    + elems
                        .into_iter()
                        .map(|elem| varint_len(elem) + elem)
                        .sum::<usize>()
            }
            None => 0,
        };

        script_sig_size * 4 + witness_size
    }

    pub fn requirements(&self) -> Requirements<Ak> {
        match self.try_complete(&SatisfactionMaterial::default()) {
            PlanState::Complete { .. } => Requirements::default(),
            PlanState::Incomplete(requirements) => requirements,
        }
    }

    pub fn try_complete(&self, auth_data: &SatisfactionMaterial) -> PlanState<Ak> {
        let unsatisfied_items = self
            .template
            .iter()
            .filter(|step| match step {
                TemplateItem::Sign(key) => {
                    !auth_data.schnorr_sigs.contains_key(&key.descriptor_key)
                }
                TemplateItem::Hash160(image) => !auth_data.hash160_preimages.contains_key(image),
                TemplateItem::Hash256(image) => !auth_data.hash256_preimages.contains_key(image),
                TemplateItem::Sha256(image) => !auth_data.sha256_preimages.contains_key(image),
                TemplateItem::Ripemd160(image) => {
                    !auth_data.ripemd160_preimages.contains_key(image)
                }
                TemplateItem::Pk { .. } | TemplateItem::One | TemplateItem::Zero => false,
            })
            .collect::<Vec<_>>();

        if unsatisfied_items.is_empty() {
            let mut witness = self
                .template
                .iter()
                .flat_map(|step| step.to_witness_stack(&auth_data))
                .collect::<Vec<_>>();
            match &self.target {
                Target::Segwitv0 { .. } => todo!(),
                Target::Legacy => todo!(),
                Target::Segwitv1 {
                    tr_plan: TrSpend::KeySpend,
                    ..
                } => PlanState::Complete {
                    final_script_sig: None,
                    final_script_witness: Some(Witness::from(witness)),
                },
                Target::Segwitv1 {
                    tr,
                    tr_plan:
                        TrSpend::LeafSpend {
                            script,
                            leaf_version,
                        },
                } => {
                    let spend_info = tr.spend_info();
                    let control_block = spend_info
                        .control_block(&(script.clone(), *leaf_version))
                        .expect("must exist");
                    witness.push(script.clone().into_bytes());
                    witness.push(control_block.serialize());

                    PlanState::Complete {
                        final_script_sig: None,
                        final_script_witness: Some(Witness::from(witness)),
                    }
                }
            }
        } else {
            let mut requirements = Requirements::default();

            match &self.target {
                Target::Legacy => {
                    todo!()
                }
                Target::Segwitv0 { .. } => {
                    todo!()
                }
                Target::Segwitv1 { tr, tr_plan } => {
                    let spend_info = tr.spend_info();
                    match tr_plan {
                        TrSpend::KeySpend => match &self.template[..] {
                            [TemplateItem::Sign(ref plan_key)] => {
                                requirements.signatures = RequiredSignatures::TapKey {
                                    merkle_root: spend_info.merkle_root(),
                                    plan_key: plan_key.clone(),
                                };
                            }
                            _ => unreachable!("tapkey spend will always have only one sign step"),
                        },
                        TrSpend::LeafSpend {
                            script,
                            leaf_version,
                        } => {
                            let leaf_hash = TapLeafHash::from_script(&script, *leaf_version);
                            requirements.signatures = RequiredSignatures::TapScript {
                                leaf_hash,
                                plan_keys: vec![],
                            }
                        }
                    }
                }
            }

            let required_signatures = match requirements.signatures {
                RequiredSignatures::Legacy { .. } => todo!(),
                RequiredSignatures::Segwitv0 { .. } => todo!(),
                RequiredSignatures::TapKey { .. } => return PlanState::Incomplete(requirements),
                RequiredSignatures::TapScript {
                    plan_keys: ref mut keys,
                    ..
                } => keys,
            };

            for step in unsatisfied_items {
                match step {
                    TemplateItem::Sign(plan_key) => {
                        required_signatures.push(plan_key.clone());
                    }
                    TemplateItem::Hash160(image) => {
                        requirements.hash160_images.insert(image.clone());
                    }
                    TemplateItem::Hash256(image) => {
                        requirements.hash256_images.insert(image.clone());
                    }
                    TemplateItem::Sha256(image) => {
                        requirements.sha256_images.insert(image.clone());
                    }
                    TemplateItem::Ripemd160(image) => {
                        requirements.ripemd160_images.insert(image.clone());
                    }
                    TemplateItem::Pk { .. } | TemplateItem::One | TemplateItem::Zero => { /* no requirements */
                    }
                }
            }

            PlanState::Incomplete(requirements)
        }
    }

    /// Witness version for the plan
    pub fn witness_version(&self) -> Option<WitnessVersion> {
        match self.target {
            Target::Legacy => None,
            Target::Segwitv0 { .. } => Some(WitnessVersion::V0),
            Target::Segwitv1 { .. } => Some(WitnessVersion::V1),
        }
    }

    /// The minimum required locktime height or time on the transaction using the plan.
    pub fn required_locktime(&self) -> Option<absolute::LockTime> {
        self.set_locktime.clone()
    }

    /// The minimum required sequence (height or time) on the input to satisfy the plan
    pub fn required_sequence(&self) -> Option<Sequence> {
        self.set_sequence.clone()
    }

    /// The minimum required transaction version required on the transaction using the plan.
    pub fn min_version(&self) -> Option<u32> {
        if let Some(_) = self.set_sequence {
            Some(2)
        } else {
            Some(1)
        }
    }
}

/// The returned value from [`Plan::try_complete`].
pub enum PlanState<Ak> {
    /// The plan is complete
    Complete {
        /// The script sig that should be set on the input
        final_script_sig: Option<ScriptBuf>,
        /// The witness that should be set on the input
        final_script_witness: Option<Witness>,
    },
    Incomplete(Requirements<Ak>),
}

#[derive(Clone, Debug)]
pub struct Assets<K> {
    pub keys: Vec<K>,
    pub txo_age: Option<Sequence>,
    pub max_locktime: Option<absolute::LockTime>,
    pub sha256: Vec<sha256::Hash>,
    pub hash256: Vec<hash256::Hash>,
    pub ripemd160: Vec<ripemd160::Hash>,
    pub hash160: Vec<hash160::Hash>,
}

impl<K> Default for Assets<K> {
    fn default() -> Self {
        Self {
            keys: Default::default(),
            txo_age: Default::default(),
            max_locktime: Default::default(),
            sha256: Default::default(),
            hash256: Default::default(),
            ripemd160: Default::default(),
            hash160: Default::default(),
        }
    }
}

pub trait CanDerive {
    fn can_derive(&self, key: &DefiniteDescriptorKey) -> Option<DerivationPath>;
}

impl CanDerive for KeySource {
    fn can_derive(&self, key: &DefiniteDescriptorKey) -> Option<DerivationPath> {
        match DescriptorPublicKey::from(key.clone()) {
            DescriptorPublicKey::Single(single_pub) => {
                path_to_child(self, single_pub.origin.as_ref()?, None)
            }
            DescriptorPublicKey::XPub(dxk) => {
                let origin = dxk.origin.clone().unwrap_or_else(|| {
                    let secp = Secp256k1::signing_only();
                    (dxk.xkey.xkey_fingerprint(&secp), DerivationPath::master())
                });

                path_to_child(self, &origin, Some(&dxk.derivation_path))
            }
            DescriptorPublicKey::MultiXPub(_) => {
                // This crate will be replaced by
                // https://github.com/rust-bitcoin/rust-miniscript/pull/481 anyways
                todo!();
            }
        }
    }
}

impl CanDerive for DescriptorPublicKey {
    fn can_derive(&self, key: &DefiniteDescriptorKey) -> Option<DerivationPath> {
        match (self, DescriptorPublicKey::from(key.clone())) {
            (parent, child) if parent == &child => Some(DerivationPath::master()),
            (DescriptorPublicKey::XPub(parent), _) => {
                let origin = parent.origin.clone().unwrap_or_else(|| {
                    let secp = Secp256k1::signing_only();
                    (
                        parent.xkey.xkey_fingerprint(&secp),
                        DerivationPath::master(),
                    )
                });
                KeySource::from(origin).can_derive(key)
            }
            _ => None,
        }
    }
}

fn path_to_child(
    parent: &KeySource,
    child_origin: &(Fingerprint, DerivationPath),
    child_derivation: Option<&DerivationPath>,
) -> Option<DerivationPath> {
    if parent.0 == child_origin.0 {
        let mut remaining_derivation =
            DerivationPath::from(child_origin.1[..].strip_prefix(&parent.1[..])?);
        remaining_derivation =
            remaining_derivation.extend(child_derivation.unwrap_or(&DerivationPath::master()));
        Some(remaining_derivation)
    } else {
        None
    }
}

pub fn plan_satisfaction<Ak>(
    desc: &Descriptor<DefiniteDescriptorKey>,
    assets: &Assets<Ak>,
) -> Option<Plan<Ak>>
where
    Ak: CanDerive + Clone,
{
    match desc {
        Descriptor::Bare(_) => todo!(),
        Descriptor::Pkh(_) => todo!(),
        Descriptor::Wpkh(_) => todo!(),
        Descriptor::Sh(_) => todo!(),
        Descriptor::Wsh(_) => todo!(),
        Descriptor::Tr(tr) => crate::plan_impls::plan_satisfaction_tr(tr, assets),
    }
}
