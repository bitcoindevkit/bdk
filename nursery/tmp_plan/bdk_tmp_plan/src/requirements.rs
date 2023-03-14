use bdk_chain::{bitcoin, collections::*, miniscript};
use core::ops::Deref;

use bitcoin::{
    hashes::{hash160, ripemd160, sha256},
    psbt::Prevouts,
    secp256k1::{KeyPair, Message, PublicKey, Signing, Verification},
    util::{bip32, sighash, sighash::SighashCache, taproot},
    EcdsaSighashType, SchnorrSighashType, Transaction, TxOut, XOnlyPublicKey,
};

use super::*;
use miniscript::{
    descriptor::{DescriptorSecretKey, KeyMap},
    hash256,
};

#[derive(Clone, Debug)]
/// Signatures and hash pre-images that must be provided to complete the plan.
pub struct Requirements<Ak> {
    /// required signatures
    pub signatures: RequiredSignatures<Ak>,
    /// required sha256 pre-images
    pub sha256_images: HashSet<sha256::Hash>,
    /// required hash160 pre-images
    pub hash160_images: HashSet<hash160::Hash>,
    /// required hash256 pre-images
    pub hash256_images: HashSet<hash256::Hash>,
    /// required ripemd160 pre-images
    pub ripemd160_images: HashSet<ripemd160::Hash>,
}

impl<Ak> Default for RequiredSignatures<Ak> {
    fn default() -> Self {
        RequiredSignatures::Legacy {
            keys: Default::default(),
        }
    }
}

impl<Ak> Default for Requirements<Ak> {
    fn default() -> Self {
        Self {
            signatures: Default::default(),
            sha256_images: Default::default(),
            hash160_images: Default::default(),
            hash256_images: Default::default(),
            ripemd160_images: Default::default(),
        }
    }
}

impl<Ak> Requirements<Ak> {
    /// Whether any hash pre-images are required in the plan
    pub fn requires_hash_preimages(&self) -> bool {
        !(self.sha256_images.is_empty()
            && self.hash160_images.is_empty()
            && self.hash256_images.is_empty()
            && self.ripemd160_images.is_empty())
    }
}

/// The signatures required to complete the plan
#[derive(Clone, Debug)]
pub enum RequiredSignatures<Ak> {
    /// Legacy ECDSA signatures are required
    Legacy { keys: Vec<PlanKey<Ak>> },
    /// Segwitv0 ECDSA signatures are required
    Segwitv0 { keys: Vec<PlanKey<Ak>> },
    /// A Taproot key spend signature is required
    TapKey {
        /// the internal key
        plan_key: PlanKey<Ak>,
        /// The merkle root of the taproot output
        merkle_root: Option<TapBranchHash>,
    },
    /// Taproot script path signatures are required
    TapScript {
        /// The leaf hash of the script being used
        leaf_hash: TapLeafHash,
        /// The keys in the script that require signatures
        plan_keys: Vec<PlanKey<Ak>>,
    },
}

#[derive(Clone, Debug)]
pub enum SigningError {
    SigHashError(sighash::Error),
    DerivationError(bip32::Error),
}

impl From<sighash::Error> for SigningError {
    fn from(e: sighash::Error) -> Self {
        Self::SigHashError(e)
    }
}

impl core::fmt::Display for SigningError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SigningError::SigHashError(e) => e.fmt(f),
            SigningError::DerivationError(e) => e.fmt(f),
        }
    }
}

impl From<bip32::Error> for SigningError {
    fn from(e: bip32::Error) -> Self {
        Self::DerivationError(e)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for SigningError {}

impl RequiredSignatures<DescriptorPublicKey> {
    pub fn sign_with_keymap<T: Deref<Target = Transaction>>(
        &self,
        input_index: usize,
        keymap: &KeyMap,
        prevouts: &Prevouts<'_, impl core::borrow::Borrow<TxOut>>,
        schnorr_sighashty: Option<SchnorrSighashType>,
        _ecdsa_sighashty: Option<EcdsaSighashType>,
        sighash_cache: &mut SighashCache<T>,
        auth_data: &mut SatisfactionMaterial,
        secp: &Secp256k1<impl Signing + Verification>,
    ) -> Result<bool, SigningError> {
        match self {
            RequiredSignatures::Legacy { .. } | RequiredSignatures::Segwitv0 { .. } => todo!(),
            RequiredSignatures::TapKey {
                plan_key,
                merkle_root,
            } => {
                let schnorr_sighashty = schnorr_sighashty.unwrap_or(SchnorrSighashType::Default);
                let sighash = sighash_cache.taproot_key_spend_signature_hash(
                    input_index,
                    prevouts,
                    schnorr_sighashty,
                )?;
                let secret_key = match keymap.get(&plan_key.asset_key) {
                    Some(secret_key) => secret_key,
                    None => return Ok(false),
                };
                let secret_key = match secret_key {
                    DescriptorSecretKey::Single(single) => single.key.inner,
                    DescriptorSecretKey::XPrv(xprv) => {
                        xprv.xkey
                            .derive_priv(&secp, &plan_key.derivation_hint)?
                            .private_key
                    }
                };

                let pubkey = PublicKey::from_secret_key(&secp, &secret_key);
                let x_only_pubkey = XOnlyPublicKey::from(pubkey);

                let tweak =
                    taproot::TapTweakHash::from_key_and_tweak(x_only_pubkey, merkle_root.clone());
                let keypair = KeyPair::from_secret_key(&secp, &secret_key.clone())
                    .add_xonly_tweak(&secp, &tweak.to_scalar())
                    .unwrap();

                let msg = Message::from_slice(sighash.as_ref()).expect("Sighashes are 32 bytes");
                let sig = secp.sign_schnorr_no_aux_rand(&msg, &keypair);

                let bitcoin_sig = SchnorrSig {
                    sig,
                    hash_ty: schnorr_sighashty,
                };

                auth_data
                    .schnorr_sigs
                    .insert(plan_key.descriptor_key.clone(), bitcoin_sig);
                Ok(true)
            }
            RequiredSignatures::TapScript {
                leaf_hash,
                plan_keys,
            } => {
                let sighash_type = schnorr_sighashty.unwrap_or(SchnorrSighashType::Default);
                let sighash = sighash_cache.taproot_script_spend_signature_hash(
                    input_index,
                    prevouts,
                    *leaf_hash,
                    sighash_type,
                )?;

                let mut modified = false;

                for plan_key in plan_keys {
                    if let Some(secret_key) = keymap.get(&plan_key.asset_key) {
                        let secret_key = match secret_key {
                            DescriptorSecretKey::Single(single) => single.key.inner,
                            DescriptorSecretKey::XPrv(xprv) => {
                                xprv.xkey
                                    .derive_priv(&secp, &plan_key.derivation_hint)?
                                    .private_key
                            }
                        };
                        let keypair = KeyPair::from_secret_key(&secp, &secret_key.clone());
                        let msg =
                            Message::from_slice(sighash.as_ref()).expect("Sighashes are 32 bytes");
                        let sig = secp.sign_schnorr_no_aux_rand(&msg, &keypair);
                        let bitcoin_sig = SchnorrSig {
                            sig,
                            hash_ty: sighash_type,
                        };

                        auth_data
                            .schnorr_sigs
                            .insert(plan_key.descriptor_key.clone(), bitcoin_sig);
                        modified = true;
                    }
                }
                Ok(modified)
            }
        }
    }
}
