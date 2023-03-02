use bdk_chain::{bitcoin, miniscript};
use bitcoin::{
    hashes::{hash160, ripemd160, sha256},
    util::bip32::DerivationPath,
};

use super::*;
use crate::{hash256, varint_len, DefiniteDescriptorKey};

#[derive(Clone, Debug)]
pub(crate) enum TemplateItem<Ak> {
    Sign(PlanKey<Ak>),
    Pk { key: DefiniteDescriptorKey },
    One,
    Zero,
    Sha256(sha256::Hash),
    Hash256(hash256::Hash),
    Ripemd160(ripemd160::Hash),
    Hash160(hash160::Hash),
}

/// A plan key contains the asset key originally provided along with key in the descriptor it
/// purports to be able to derive for along with a "hint" on how to derive it.
#[derive(Clone, Debug)]
pub struct PlanKey<Ak> {
    /// The key the planner will sign with
    pub asset_key: Ak,
    /// A hint from how to get from the asset key to the concrete key we need to sign with.
    pub derivation_hint: DerivationPath,
    /// The key that was in the descriptor that we are satisfying with the signature from the asset
    /// key.
    pub descriptor_key: DefiniteDescriptorKey,
}

impl<Ak> TemplateItem<Ak> {
    pub fn expected_size(&self) -> usize {
        match self {
            TemplateItem::Sign { .. } => 64, /*size of sig TODO: take into consideration sighash falg*/
            TemplateItem::Pk { .. } => 32,
            TemplateItem::One => varint_len(1),
            TemplateItem::Zero => 0, /* zero means an empty witness element */
            // I'm not sure if it should be 32 here (it's a 20 byte hash) but that's what other
            // parts of the code were doing.
            TemplateItem::Hash160(_) | TemplateItem::Ripemd160(_) => 32,
            TemplateItem::Sha256(_) | TemplateItem::Hash256(_) => 32,
        }
    }

    // this can only be called if we are sure that auth_data has what we need
    pub(super) fn to_witness_stack(&self, auth_data: &SatisfactionMaterial) -> Vec<Vec<u8>> {
        match self {
            TemplateItem::Sign(plan_key) => {
                vec![auth_data
                    .schnorr_sigs
                    .get(&plan_key.descriptor_key)
                    .unwrap()
                    .to_vec()]
            }
            TemplateItem::One => vec![vec![1]],
            TemplateItem::Zero => vec![vec![]],
            TemplateItem::Sha256(image) => {
                vec![auth_data.sha256_preimages.get(image).unwrap().to_vec()]
            }
            TemplateItem::Hash160(image) => {
                vec![auth_data.hash160_preimages.get(image).unwrap().to_vec()]
            }
            TemplateItem::Ripemd160(image) => {
                vec![auth_data.ripemd160_preimages.get(image).unwrap().to_vec()]
            }
            TemplateItem::Hash256(image) => {
                vec![auth_data.hash256_preimages.get(image).unwrap().to_vec()]
            }
            TemplateItem::Pk { key } => vec![key.to_public_key().to_bytes()],
        }
    }
}
