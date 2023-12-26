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

//! Generalized signers
//!
//! This module provides the ability to add customized signers to a [`Wallet`](super::Wallet)
//! through the [`Wallet::add_signer`](super::Wallet::add_signer) function.
//!
//! ```
//! # use alloc::sync::Arc;
//! # use core::str::FromStr;
//! # use bitcoin::secp256k1::{Secp256k1, All};
//! # use bitcoin::*;
//! # use bitcoin::psbt;
//! # use bdk::signer::*;
//! # use bdk::*;
//! # #[derive(Debug)]
//! # struct CustomHSM;
//! # impl CustomHSM {
//! #     fn hsm_sign_input(&self, _psbt: &mut psbt::PartiallySignedTransaction, _input: usize) -> Result<(), SignerError> {
//! #         Ok(())
//! #     }
//! #     fn connect() -> Self {
//! #         CustomHSM
//! #     }
//! #     fn get_id(&self) -> SignerId {
//! #         SignerId::Dummy(0)
//! #     }
//! # }
//! #[derive(Debug)]
//! struct CustomSigner {
//!     device: CustomHSM,
//! }
//!
//! impl CustomSigner {
//!     fn connect() -> Self {
//!         CustomSigner { device: CustomHSM::connect() }
//!     }
//! }
//!
//! impl SignerCommon for CustomSigner {
//!     fn id(&self, _secp: &Secp256k1<All>) -> SignerId {
//!         self.device.get_id()
//!     }
//! }
//!
//! impl InputSigner for CustomSigner {
//!     fn sign_input(
//!         &self,
//!         psbt: &mut psbt::PartiallySignedTransaction,
//!         input_index: usize,
//!         _sign_options: &SignOptions,
//!         _secp: &Secp256k1<All>,
//!     ) -> Result<(), SignerError> {
//!         self.device.hsm_sign_input(psbt, input_index)?;
//!
//!         Ok(())
//!     }
//! }
//!
//! let custom_signer = CustomSigner::connect();
//!
//! let descriptor = "wpkh(tpubD6NzVbkrYhZ4Xferm7Pz4VnjdcDPFyjVu5K4iZXQ4pVN8Cks4pHVowTBXBKRhX64pkRyJZJN5xAKj4UDNnLPb5p2sSKXhewoYx5GbTdUFWq/*)";
//! let mut wallet = Wallet::builder(descriptor)
//!     .with_network(Network::Testnet)
//!     .init_without_persistence()?;
//! wallet.add_signer(
//!     KeychainKind::External,
//!     SignerOrdering(200),
//!     Arc::new(custom_signer)
//! );
//!
//! # Ok::<_, anyhow::Error>(())
//! ```

use crate::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt;
use core::ops::{Bound::Included, Deref};

use bitcoin::bip32::{ChildNumber, DerivationPath, ExtendedPrivKey, Fingerprint};
use bitcoin::hashes::hash160;
use bitcoin::secp256k1::Message;
use bitcoin::sighash::{EcdsaSighashType, TapSighash, TapSighashType};
use bitcoin::{ecdsa, psbt, sighash, taproot};
use bitcoin::{key::TapTweak, key::XOnlyPublicKey, secp256k1};
use bitcoin::{PrivateKey, PublicKey};

use miniscript::descriptor::{
    Descriptor, DescriptorMultiXKey, DescriptorPublicKey, DescriptorSecretKey, DescriptorXKey,
    InnerXKey, KeyMap, SinglePriv, SinglePubKey,
};
use miniscript::{Legacy, Segwitv0, SigType, Tap, ToPublicKey};

use super::utils::SecpCtx;
use crate::descriptor::{DescriptorMeta, XKeyUtils};
use crate::psbt::PsbtUtils;
use crate::wallet::error::MiniscriptPsbtError;

/// Identifier of a signer in the `SignersContainers`. Used as a key to find the right signer among
/// multiple of them
#[derive(Debug, Clone, Ord, PartialOrd, PartialEq, Eq, Hash)]
pub enum SignerId {
    /// Bitcoin HASH160 (RIPEMD160 after SHA256) hash of an ECDSA public key
    PkHash(hash160::Hash),
    /// The fingerprint of a BIP32 extended key
    Fingerprint(Fingerprint),
    /// Dummy identifier
    Dummy(u64),
}

impl From<hash160::Hash> for SignerId {
    fn from(hash: hash160::Hash) -> SignerId {
        SignerId::PkHash(hash)
    }
}

impl From<Fingerprint> for SignerId {
    fn from(fing: Fingerprint) -> SignerId {
        SignerId::Fingerprint(fing)
    }
}

/// Signing error
#[derive(Debug)]
pub enum SignerError {
    /// The private key is missing for the required public key
    MissingKey,
    /// The private key in use has the right fingerprint but derives differently than expected
    InvalidKey,
    /// The user canceled the operation
    UserCanceled,
    /// Input index is out of range
    InputIndexOutOfRange,
    /// The `non_witness_utxo` field of the transaction is required to sign this input
    MissingNonWitnessUtxo,
    /// The `non_witness_utxo` specified is invalid
    InvalidNonWitnessUtxo,
    /// The `witness_utxo` field of the transaction is required to sign this input
    MissingWitnessUtxo,
    /// The `witness_script` field of the transaction is required to sign this input
    MissingWitnessScript,
    /// The fingerprint and derivation path are missing from the psbt input
    MissingHdKeypath,
    /// The psbt contains a non-`SIGHASH_ALL` sighash in one of its input and the user hasn't
    /// explicitly allowed them
    ///
    /// To enable signing transactions with non-standard sighashes set
    /// [`SignOptions::allow_all_sighashes`] to `true`.
    NonStandardSighash,
    /// Invalid SIGHASH for the signing context in use
    InvalidSighash,
    /// Error while computing the hash to sign
    SighashError(sighash::Error),
    /// Miniscript PSBT error
    MiniscriptPsbt(MiniscriptPsbtError),
    /// Error while signing using hardware wallets
    #[cfg(feature = "hardware-signer")]
    HWIError(hwi::error::Error),
}

#[cfg(feature = "hardware-signer")]
impl From<hwi::error::Error> for SignerError {
    fn from(e: hwi::error::Error) -> Self {
        SignerError::HWIError(e)
    }
}

impl From<sighash::Error> for SignerError {
    fn from(e: sighash::Error) -> Self {
        SignerError::SighashError(e)
    }
}

impl fmt::Display for SignerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingKey => write!(f, "Missing private key"),
            Self::InvalidKey => write!(f, "The private key in use has the right fingerprint but derives differently than expected"),
            Self::UserCanceled => write!(f, "The user canceled the operation"),
            Self::InputIndexOutOfRange => write!(f, "Input index out of range"),
            Self::MissingNonWitnessUtxo => write!(f, "Missing non-witness UTXO"),
            Self::InvalidNonWitnessUtxo => write!(f, "Invalid non-witness UTXO"),
            Self::MissingWitnessUtxo => write!(f, "Missing witness UTXO"),
            Self::MissingWitnessScript => write!(f, "Missing witness script"),
            Self::MissingHdKeypath => write!(f, "Missing fingerprint and derivation path"),
            Self::NonStandardSighash => write!(f, "The psbt contains a non standard sighash"),
            Self::InvalidSighash => write!(f, "Invalid SIGHASH for the signing context in use"),
            Self::SighashError(err) => write!(f, "Error while computing the hash to sign: {}", err),
            Self::MiniscriptPsbt(err) => write!(f, "Miniscript PSBT error: {}", err),
            #[cfg(feature = "hardware-signer")]
            Self::HWIError(err) => write!(f, "Error while signing using hardware wallets: {}", err),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for SignerError {}

/// Signing context
///
/// Used by our software signers to determine the type of signatures to make
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignerContext {
    /// Legacy context
    Legacy,
    /// Segwit v0 context (BIP 143)
    Segwitv0,
    /// Taproot context (BIP 340)
    Tap {
        /// Whether the signer can sign for the internal key or not
        is_internal_key: bool,
    },
}

/// Wrapper to pair a signer with its context
#[derive(Debug, Clone)]
pub struct SignerWrapper<S: Sized + fmt::Debug + Clone> {
    signer: S,
    ctx: SignerContext,
}

impl<S: Sized + fmt::Debug + Clone> SignerWrapper<S> {
    /// Create a wrapped signer from a signer and a context
    pub fn new(signer: S, ctx: SignerContext) -> Self {
        SignerWrapper { signer, ctx }
    }
}

impl<S: Sized + fmt::Debug + Clone> Deref for SignerWrapper<S> {
    type Target = S;

    fn deref(&self) -> &Self::Target {
        &self.signer
    }
}

/// Common signer methods
pub trait SignerCommon: fmt::Debug + Send + Sync {
    /// Return the [`SignerId`] for this signer
    ///
    /// The [`SignerId`] can be used to lookup a signer in the [`Wallet`](crate::Wallet)'s signers map or to
    /// compare two signers.
    fn id(&self, secp: &SecpCtx) -> SignerId;

    /// Return the secret key for the signer
    ///
    /// This is used internally to reconstruct the original descriptor that may contain secrets.
    /// External signers that are meant to keep key isolated should just return `None` here (which
    /// is the default for this method, if not overridden).
    fn descriptor_secret_key(&self) -> Option<DescriptorSecretKey> {
        None
    }
}

/// PSBT Input signer
///
/// This trait can be implemented to provide custom signers to the wallet. If the signer supports signing
/// individual inputs, this trait should be implemented and BDK will provide automatically an implementation
/// for [`TransactionSigner`].
pub trait InputSigner: SignerCommon {
    /// Sign a single psbt input
    fn sign_input(
        &self,
        psbt: &mut psbt::PartiallySignedTransaction,
        input_index: usize,
        sign_options: &SignOptions,
        secp: &SecpCtx,
    ) -> Result<(), SignerError>;
}

/// PSBT signer
///
/// This trait can be implemented when the signer can't sign inputs individually, but signs the whole transaction
/// at once.
pub trait TransactionSigner: SignerCommon {
    /// Sign all the inputs of the psbt
    fn sign_transaction(
        &self,
        psbt: &mut psbt::PartiallySignedTransaction,
        sign_options: &SignOptions,
        secp: &SecpCtx,
    ) -> Result<(), SignerError>;
}

impl<T: InputSigner> TransactionSigner for T {
    fn sign_transaction(
        &self,
        psbt: &mut psbt::PartiallySignedTransaction,
        sign_options: &SignOptions,
        secp: &SecpCtx,
    ) -> Result<(), SignerError> {
        for input_index in 0..psbt.inputs.len() {
            self.sign_input(psbt, input_index, sign_options, secp)?;
        }

        Ok(())
    }
}

impl SignerCommon for SignerWrapper<DescriptorXKey<ExtendedPrivKey>> {
    fn id(&self, secp: &SecpCtx) -> SignerId {
        SignerId::from(self.root_fingerprint(secp))
    }

    fn descriptor_secret_key(&self) -> Option<DescriptorSecretKey> {
        Some(DescriptorSecretKey::XPrv(self.signer.clone()))
    }
}

impl InputSigner for SignerWrapper<DescriptorXKey<ExtendedPrivKey>> {
    fn sign_input(
        &self,
        psbt: &mut psbt::PartiallySignedTransaction,
        input_index: usize,
        sign_options: &SignOptions,
        secp: &SecpCtx,
    ) -> Result<(), SignerError> {
        if input_index >= psbt.inputs.len() {
            return Err(SignerError::InputIndexOutOfRange);
        }

        if psbt.inputs[input_index].final_script_sig.is_some()
            || psbt.inputs[input_index].final_script_witness.is_some()
        {
            return Ok(());
        }

        let tap_key_origins = psbt.inputs[input_index]
            .tap_key_origins
            .iter()
            .map(|(pk, (_, keysource))| (SinglePubKey::XOnly(*pk), keysource));
        let (public_key, full_path) = match psbt.inputs[input_index]
            .bip32_derivation
            .iter()
            .map(|(pk, keysource)| (SinglePubKey::FullKey(PublicKey::new(*pk)), keysource))
            .chain(tap_key_origins)
            .find_map(|(pk, keysource)| {
                if self.matches(keysource, secp).is_some() {
                    Some((pk, keysource.1.clone()))
                } else {
                    None
                }
            }) {
            Some((pk, full_path)) => (pk, full_path),
            None => return Ok(()),
        };

        let derived_key = match self.origin.clone() {
            Some((_fingerprint, origin_path)) => {
                let deriv_path = DerivationPath::from(
                    &full_path.into_iter().cloned().collect::<Vec<ChildNumber>>()
                        [origin_path.len()..],
                );
                self.xkey.derive_priv(secp, &deriv_path).unwrap()
            }
            None => self.xkey.derive_priv(secp, &full_path).unwrap(),
        };

        let computed_pk = secp256k1::PublicKey::from_secret_key(secp, &derived_key.private_key);
        let valid_key = match public_key {
            SinglePubKey::FullKey(pk) if pk.inner == computed_pk => true,
            SinglePubKey::XOnly(x_only) if XOnlyPublicKey::from(computed_pk) == x_only => true,
            _ => false,
        };
        if !valid_key {
            Err(SignerError::InvalidKey)
        } else {
            // HD wallets imply compressed keys
            let priv_key = PrivateKey {
                compressed: true,
                network: self.xkey.network,
                inner: derived_key.private_key,
            };

            SignerWrapper::new(priv_key, self.ctx).sign_input(psbt, input_index, sign_options, secp)
        }
    }
}

fn multikey_to_xkeys<K: InnerXKey + Clone>(
    multikey: DescriptorMultiXKey<K>,
) -> Vec<DescriptorXKey<K>> {
    multikey
        .derivation_paths
        .into_paths()
        .into_iter()
        .map(|derivation_path| DescriptorXKey {
            origin: multikey.origin.clone(),
            xkey: multikey.xkey.clone(),
            derivation_path,
            wildcard: multikey.wildcard,
        })
        .collect()
}

impl SignerCommon for SignerWrapper<DescriptorMultiXKey<ExtendedPrivKey>> {
    fn id(&self, secp: &SecpCtx) -> SignerId {
        SignerId::from(self.root_fingerprint(secp))
    }

    fn descriptor_secret_key(&self) -> Option<DescriptorSecretKey> {
        Some(DescriptorSecretKey::MultiXPrv(self.signer.clone()))
    }
}

impl InputSigner for SignerWrapper<DescriptorMultiXKey<ExtendedPrivKey>> {
    fn sign_input(
        &self,
        psbt: &mut psbt::PartiallySignedTransaction,
        input_index: usize,
        sign_options: &SignOptions,
        secp: &SecpCtx,
    ) -> Result<(), SignerError> {
        let xkeys = multikey_to_xkeys(self.signer.clone());
        for xkey in xkeys {
            SignerWrapper::new(xkey, self.ctx).sign_input(psbt, input_index, sign_options, secp)?
        }
        Ok(())
    }
}

impl SignerCommon for SignerWrapper<PrivateKey> {
    fn id(&self, secp: &SecpCtx) -> SignerId {
        SignerId::from(self.public_key(secp).to_pubkeyhash(SigType::Ecdsa))
    }

    fn descriptor_secret_key(&self) -> Option<DescriptorSecretKey> {
        Some(DescriptorSecretKey::Single(SinglePriv {
            key: self.signer,
            origin: None,
        }))
    }
}

impl InputSigner for SignerWrapper<PrivateKey> {
    fn sign_input(
        &self,
        psbt: &mut psbt::PartiallySignedTransaction,
        input_index: usize,
        sign_options: &SignOptions,
        secp: &SecpCtx,
    ) -> Result<(), SignerError> {
        if input_index >= psbt.inputs.len() || input_index >= psbt.unsigned_tx.input.len() {
            return Err(SignerError::InputIndexOutOfRange);
        }

        if psbt.inputs[input_index].final_script_sig.is_some()
            || psbt.inputs[input_index].final_script_witness.is_some()
        {
            return Ok(());
        }

        let pubkey = PublicKey::from_private_key(secp, self);
        let x_only_pubkey = XOnlyPublicKey::from(pubkey.inner);

        if let SignerContext::Tap { is_internal_key } = self.ctx {
            if let Some(psbt_internal_key) = psbt.inputs[input_index].tap_internal_key {
                if is_internal_key
                    && psbt.inputs[input_index].tap_key_sig.is_none()
                    && sign_options.sign_with_tap_internal_key
                    && x_only_pubkey == psbt_internal_key
                {
                    let (hash, hash_ty) = Tap::sighash(psbt, input_index, None)?;
                    sign_psbt_schnorr(
                        &self.inner,
                        x_only_pubkey,
                        None,
                        &mut psbt.inputs[input_index],
                        hash,
                        hash_ty,
                        secp,
                    );
                }
            }

            if let Some((leaf_hashes, _)) =
                psbt.inputs[input_index].tap_key_origins.get(&x_only_pubkey)
            {
                let leaf_hashes = leaf_hashes
                    .iter()
                    .filter(|lh| {
                        // Removing the leaves we shouldn't sign for
                        let should_sign = match &sign_options.tap_leaves_options {
                            TapLeavesOptions::All => true,
                            TapLeavesOptions::Include(v) => v.contains(lh),
                            TapLeavesOptions::Exclude(v) => !v.contains(lh),
                            TapLeavesOptions::None => false,
                        };
                        // Filtering out the leaves without our key
                        should_sign
                            && !psbt.inputs[input_index]
                                .tap_script_sigs
                                .contains_key(&(x_only_pubkey, **lh))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                for lh in leaf_hashes {
                    let (hash, hash_ty) = Tap::sighash(psbt, input_index, Some(lh))?;
                    sign_psbt_schnorr(
                        &self.inner,
                        x_only_pubkey,
                        Some(lh),
                        &mut psbt.inputs[input_index],
                        hash,
                        hash_ty,
                        secp,
                    );
                }
            }

            return Ok(());
        }

        if psbt.inputs[input_index].partial_sigs.contains_key(&pubkey) {
            return Ok(());
        }

        let (hash, hash_ty) = match self.ctx {
            SignerContext::Segwitv0 => {
                let (h, t) = Segwitv0::sighash(psbt, input_index, ())?;
                let h = h.to_raw_hash();
                (h, t)
            }
            SignerContext::Legacy => {
                let (h, t) = Legacy::sighash(psbt, input_index, ())?;
                let h = h.to_raw_hash();
                (h, t)
            }
            _ => return Ok(()), // handled above
        };
        sign_psbt_ecdsa(
            &self.inner,
            pubkey,
            &mut psbt.inputs[input_index],
            hash,
            hash_ty,
            secp,
            sign_options.allow_grinding,
        );

        Ok(())
    }
}

fn sign_psbt_ecdsa(
    secret_key: &secp256k1::SecretKey,
    pubkey: PublicKey,
    psbt_input: &mut psbt::Input,
    hash: impl bitcoin::hashes::Hash + bitcoin::secp256k1::ThirtyTwoByteHash,
    hash_ty: EcdsaSighashType,
    secp: &SecpCtx,
    allow_grinding: bool,
) {
    let msg = &Message::from(hash);
    let sig = if allow_grinding {
        secp.sign_ecdsa_low_r(msg, secret_key)
    } else {
        secp.sign_ecdsa(msg, secret_key)
    };
    secp.verify_ecdsa(msg, &sig, &pubkey.inner)
        .expect("invalid or corrupted ecdsa signature");

    let final_signature = ecdsa::Signature { sig, hash_ty };
    psbt_input.partial_sigs.insert(pubkey, final_signature);
}

// Calling this with `leaf_hash` = `None` will sign for key-spend
fn sign_psbt_schnorr(
    secret_key: &secp256k1::SecretKey,
    pubkey: XOnlyPublicKey,
    leaf_hash: Option<taproot::TapLeafHash>,
    psbt_input: &mut psbt::Input,
    hash: TapSighash,
    hash_ty: TapSighashType,
    secp: &SecpCtx,
) {
    let keypair = secp256k1::KeyPair::from_seckey_slice(secp, secret_key.as_ref()).unwrap();
    let keypair = match leaf_hash {
        None => keypair
            .tap_tweak(secp, psbt_input.tap_merkle_root)
            .to_inner(),
        Some(_) => keypair, // no tweak for script spend
    };

    let msg = &Message::from(hash);
    let sig = secp.sign_schnorr(msg, &keypair);
    secp.verify_schnorr(&sig, msg, &XOnlyPublicKey::from_keypair(&keypair).0)
        .expect("invalid or corrupted schnorr signature");

    let final_signature = taproot::Signature { sig, hash_ty };

    if let Some(lh) = leaf_hash {
        psbt_input
            .tap_script_sigs
            .insert((pubkey, lh), final_signature);
    } else {
        psbt_input.tap_key_sig = Some(final_signature);
    }
}

/// Defines the order in which signers are called
///
/// The default value is `100`. Signers with an ordering above that will be called later,
/// and they will thus see the partial signatures added to the transaction once they get to sign
/// themselves.
#[derive(Debug, Clone, PartialOrd, PartialEq, Ord, Eq)]
pub struct SignerOrdering(pub usize);

impl Default for SignerOrdering {
    fn default() -> Self {
        SignerOrdering(100)
    }
}

#[derive(Debug, Clone)]
struct SignersContainerKey {
    id: SignerId,
    ordering: SignerOrdering,
}

impl From<(SignerId, SignerOrdering)> for SignersContainerKey {
    fn from(tuple: (SignerId, SignerOrdering)) -> Self {
        SignersContainerKey {
            id: tuple.0,
            ordering: tuple.1,
        }
    }
}

/// Container for multiple signers
#[derive(Debug, Default, Clone)]
pub struct SignersContainer(BTreeMap<SignersContainerKey, Arc<dyn TransactionSigner>>);

impl SignersContainer {
    /// Create a map of public keys to secret keys
    pub fn as_key_map(&self, secp: &SecpCtx) -> KeyMap {
        self.0
            .values()
            .filter_map(|signer| signer.descriptor_secret_key())
            .filter_map(|secret| secret.to_public(secp).ok().map(|public| (public, secret)))
            .collect()
    }

    /// Build a new signer container from a [`KeyMap`]
    ///
    /// Also looks at the corresponding descriptor to determine the [`SignerContext`] to attach to
    /// the signers
    pub fn build(
        keymap: KeyMap,
        descriptor: &Descriptor<DescriptorPublicKey>,
        secp: &SecpCtx,
    ) -> SignersContainer {
        let mut container = SignersContainer::new();

        for (pubkey, secret) in keymap {
            let ctx = match descriptor {
                Descriptor::Tr(tr) => SignerContext::Tap {
                    is_internal_key: tr.internal_key() == &pubkey,
                },
                _ if descriptor.is_witness() => SignerContext::Segwitv0,
                _ => SignerContext::Legacy,
            };

            match secret {
                DescriptorSecretKey::Single(private_key) => container.add_external(
                    SignerId::from(
                        private_key
                            .key
                            .public_key(secp)
                            .to_pubkeyhash(SigType::Ecdsa),
                    ),
                    SignerOrdering::default(),
                    Arc::new(SignerWrapper::new(private_key.key, ctx)),
                ),
                DescriptorSecretKey::XPrv(xprv) => container.add_external(
                    SignerId::from(xprv.root_fingerprint(secp)),
                    SignerOrdering::default(),
                    Arc::new(SignerWrapper::new(xprv, ctx)),
                ),
                DescriptorSecretKey::MultiXPrv(xprv) => container.add_external(
                    SignerId::from(xprv.root_fingerprint(secp)),
                    SignerOrdering::default(),
                    Arc::new(SignerWrapper::new(xprv, ctx)),
                ),
            };
        }

        container
    }
}

impl SignersContainer {
    /// Default constructor
    pub fn new() -> Self {
        SignersContainer(Default::default())
    }

    /// Adds an external signer to the container for the specified id. Optionally returns the
    /// signer that was previously in the container, if any
    pub fn add_external(
        &mut self,
        id: SignerId,
        ordering: SignerOrdering,
        signer: Arc<dyn TransactionSigner>,
    ) -> Option<Arc<dyn TransactionSigner>> {
        self.0.insert((id, ordering).into(), signer)
    }

    /// Removes a signer from the container and returns it
    pub fn remove(
        &mut self,
        id: SignerId,
        ordering: SignerOrdering,
    ) -> Option<Arc<dyn TransactionSigner>> {
        self.0.remove(&(id, ordering).into())
    }

    /// Returns the list of identifiers of all the signers in the container
    pub fn ids(&self) -> Vec<&SignerId> {
        self.0
            .keys()
            .map(|SignersContainerKey { id, .. }| id)
            .collect()
    }

    /// Returns the list of signers in the container, sorted by lowest to highest `ordering`
    pub fn signers(&self) -> Vec<&Arc<dyn TransactionSigner>> {
        self.0.values().collect()
    }

    /// Finds the signer with lowest ordering for a given id in the container.
    pub fn find(&self, id: SignerId) -> Option<&Arc<dyn TransactionSigner>> {
        self.0
            .range((
                Included(&(id.clone(), SignerOrdering(0)).into()),
                Included(&(id.clone(), SignerOrdering(usize::MAX)).into()),
            ))
            .filter(|(k, _)| k.id == id)
            .map(|(_, v)| v)
            .next()
    }
}

/// Options for a software signer
///
/// Adjust the behavior of our software signers and the way a transaction is finalized
#[derive(Debug, Clone)]
pub struct SignOptions {
    /// Whether the signer should trust the `witness_utxo`, if the `non_witness_utxo` hasn't been
    /// provided
    ///
    /// Defaults to `false` to mitigate the "SegWit bug" which should trick the wallet into
    /// paying a fee larger than expected.
    ///
    /// Some wallets, especially if relatively old, might not provide the `non_witness_utxo` for
    /// SegWit transactions in the PSBT they generate: in those cases setting this to `true`
    /// should correctly produce a signature, at the expense of an increased trust in the creator
    /// of the PSBT.
    ///
    /// For more details see: <https://blog.trezor.io/details-of-firmware-updates-for-trezor-one-version-1-9-1-and-trezor-model-t-version-2-3-1-1eba8f60f2dd>
    pub trust_witness_utxo: bool,

    /// Whether the wallet should assume a specific height has been reached when trying to finalize
    /// a transaction
    ///
    /// The wallet will only "use" a timelock to satisfy the spending policy of an input if the
    /// timelock height has already been reached. This option allows overriding the "current height" to let the
    /// wallet use timelocks in the future to spend a coin.
    pub assume_height: Option<u32>,

    /// Whether the signer should use the `sighash_type` set in the PSBT when signing, no matter
    /// what its value is
    ///
    /// Defaults to `false` which will only allow signing using `SIGHASH_ALL`.
    pub allow_all_sighashes: bool,

    /// Whether to remove partial signatures from the PSBT inputs while finalizing PSBT.
    ///
    /// Defaults to `true` which will remove partial signatures during finalization.
    pub remove_partial_sigs: bool,

    /// Whether to try finalizing the PSBT after the inputs are signed.
    ///
    /// Defaults to `true` which will try finalizing PSBT after inputs are signed.
    pub try_finalize: bool,

    /// Specifies which Taproot script-spend leaves we should sign for. This option is
    /// ignored if we're signing a non-taproot PSBT.
    ///
    /// Defaults to All, i.e., the wallet will sign all the leaves it has a key for.
    pub tap_leaves_options: TapLeavesOptions,

    /// Whether we should try to sign a taproot transaction with the taproot internal key
    /// or not. This option is ignored if we're signing a non-taproot PSBT.
    ///
    /// Defaults to `true`, i.e., we always try to sign with the taproot internal key.
    pub sign_with_tap_internal_key: bool,

    /// Whether we should grind ECDSA signature to ensure signing with low r
    /// or not.
    /// Defaults to `true`, i.e., we always grind ECDSA signature to sign with low r.
    pub allow_grinding: bool,
}

/// Customize which taproot script-path leaves the signer should sign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TapLeavesOptions {
    /// The signer will sign all the leaves it has a key for.
    All,
    /// The signer won't sign leaves other than the ones specified. Note that it could still ignore
    /// some of the specified leaves, if it doesn't have the right key to sign them.
    Include(Vec<taproot::TapLeafHash>),
    /// The signer won't sign the specified leaves.
    Exclude(Vec<taproot::TapLeafHash>),
    /// The signer won't sign any leaf.
    None,
}

impl Default for TapLeavesOptions {
    fn default() -> Self {
        TapLeavesOptions::All
    }
}

#[allow(clippy::derivable_impls)]
impl Default for SignOptions {
    fn default() -> Self {
        SignOptions {
            trust_witness_utxo: false,
            assume_height: None,
            allow_all_sighashes: false,
            remove_partial_sigs: true,
            try_finalize: true,
            tap_leaves_options: TapLeavesOptions::default(),
            sign_with_tap_internal_key: true,
            allow_grinding: true,
        }
    }
}

pub(crate) trait ComputeSighash {
    type Extra;
    type Sighash;
    type SighashType;

    fn sighash(
        psbt: &psbt::PartiallySignedTransaction,
        input_index: usize,
        extra: Self::Extra,
    ) -> Result<(Self::Sighash, Self::SighashType), SignerError>;
}

impl ComputeSighash for Legacy {
    type Extra = ();
    type Sighash = sighash::LegacySighash;
    type SighashType = EcdsaSighashType;

    fn sighash(
        psbt: &psbt::PartiallySignedTransaction,
        input_index: usize,
        _extra: (),
    ) -> Result<(Self::Sighash, Self::SighashType), SignerError> {
        if input_index >= psbt.inputs.len() || input_index >= psbt.unsigned_tx.input.len() {
            return Err(SignerError::InputIndexOutOfRange);
        }

        let psbt_input = &psbt.inputs[input_index];
        let tx_input = &psbt.unsigned_tx.input[input_index];

        let sighash = psbt_input
            .sighash_type
            .unwrap_or_else(|| EcdsaSighashType::All.into())
            .ecdsa_hash_ty()
            .map_err(|_| SignerError::InvalidSighash)?;
        let script = match psbt_input.redeem_script {
            Some(ref redeem_script) => redeem_script.clone(),
            None => {
                let non_witness_utxo = psbt_input
                    .non_witness_utxo
                    .as_ref()
                    .ok_or(SignerError::MissingNonWitnessUtxo)?;
                let prev_out = non_witness_utxo
                    .output
                    .get(tx_input.previous_output.vout as usize)
                    .ok_or(SignerError::InvalidNonWitnessUtxo)?;

                prev_out.script_pubkey.clone()
            }
        };

        Ok((
            sighash::SighashCache::new(&psbt.unsigned_tx).legacy_signature_hash(
                input_index,
                &script,
                sighash.to_u32(),
            )?,
            sighash,
        ))
    }
}

impl ComputeSighash for Segwitv0 {
    type Extra = ();
    type Sighash = sighash::SegwitV0Sighash;
    type SighashType = EcdsaSighashType;

    fn sighash(
        psbt: &psbt::PartiallySignedTransaction,
        input_index: usize,
        _extra: (),
    ) -> Result<(Self::Sighash, Self::SighashType), SignerError> {
        if input_index >= psbt.inputs.len() || input_index >= psbt.unsigned_tx.input.len() {
            return Err(SignerError::InputIndexOutOfRange);
        }

        let psbt_input = &psbt.inputs[input_index];
        let tx_input = &psbt.unsigned_tx.input[input_index];

        let sighash = psbt_input
            .sighash_type
            .unwrap_or_else(|| EcdsaSighashType::All.into())
            .ecdsa_hash_ty()
            .map_err(|_| SignerError::InvalidSighash)?;

        // Always try first with the non-witness utxo
        let utxo = if let Some(prev_tx) = &psbt_input.non_witness_utxo {
            // Check the provided prev-tx
            if prev_tx.txid() != tx_input.previous_output.txid {
                return Err(SignerError::InvalidNonWitnessUtxo);
            }

            // The output should be present, if it's missing the `non_witness_utxo` is invalid
            prev_tx
                .output
                .get(tx_input.previous_output.vout as usize)
                .ok_or(SignerError::InvalidNonWitnessUtxo)?
        } else if let Some(witness_utxo) = &psbt_input.witness_utxo {
            // Fallback to the witness_utxo. If we aren't allowed to use it, signing should fail
            // before we get to this point
            witness_utxo
        } else {
            // Nothing has been provided
            return Err(SignerError::MissingNonWitnessUtxo);
        };
        let value = utxo.value;

        let script = match psbt_input.witness_script {
            Some(ref witness_script) => witness_script.clone(),
            None => {
                if utxo.script_pubkey.is_v0_p2wpkh() {
                    utxo.script_pubkey
                        .p2wpkh_script_code()
                        .expect("We check above that the spk is a p2wpkh")
                } else if psbt_input
                    .redeem_script
                    .as_ref()
                    .map(|s| s.is_v0_p2wpkh())
                    .unwrap_or(false)
                {
                    psbt_input
                        .redeem_script
                        .as_ref()
                        .unwrap()
                        .p2wpkh_script_code()
                        .expect("We check above that the spk is a p2wpkh")
                } else {
                    return Err(SignerError::MissingWitnessScript);
                }
            }
        };

        Ok((
            sighash::SighashCache::new(&psbt.unsigned_tx).segwit_signature_hash(
                input_index,
                &script,
                value,
                sighash,
            )?,
            sighash,
        ))
    }
}

impl ComputeSighash for Tap {
    type Extra = Option<taproot::TapLeafHash>;
    type Sighash = TapSighash;
    type SighashType = TapSighashType;

    fn sighash(
        psbt: &psbt::PartiallySignedTransaction,
        input_index: usize,
        extra: Self::Extra,
    ) -> Result<(Self::Sighash, TapSighashType), SignerError> {
        if input_index >= psbt.inputs.len() || input_index >= psbt.unsigned_tx.input.len() {
            return Err(SignerError::InputIndexOutOfRange);
        }

        let psbt_input = &psbt.inputs[input_index];

        let sighash_type = psbt_input
            .sighash_type
            .unwrap_or_else(|| TapSighashType::Default.into())
            .taproot_hash_ty()
            .map_err(|_| SignerError::InvalidSighash)?;
        let witness_utxos = (0..psbt.inputs.len())
            .map(|i| psbt.get_utxo_for(i))
            .collect::<Vec<_>>();
        let mut all_witness_utxos = vec![];

        let mut cache = sighash::SighashCache::new(&psbt.unsigned_tx);
        let is_anyone_can_pay = psbt::PsbtSighashType::from(sighash_type).to_u32() & 0x80 != 0;
        let prevouts = if is_anyone_can_pay {
            sighash::Prevouts::One(
                input_index,
                witness_utxos[input_index]
                    .as_ref()
                    .ok_or(SignerError::MissingWitnessUtxo)?,
            )
        } else if witness_utxos.iter().all(Option::is_some) {
            all_witness_utxos.extend(witness_utxos.iter().filter_map(|x| x.as_ref()));
            sighash::Prevouts::All(&all_witness_utxos)
        } else {
            return Err(SignerError::MissingWitnessUtxo);
        };

        // Assume no OP_CODESEPARATOR
        let extra = extra.map(|leaf_hash| (leaf_hash, 0xFFFFFFFF));

        Ok((
            cache.taproot_signature_hash(input_index, &prevouts, None, extra, sighash_type)?,
            sighash_type,
        ))
    }
}

impl PartialOrd for SignersContainerKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SignersContainerKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.ordering
            .cmp(&other.ordering)
            .then(self.id.cmp(&other.id))
    }
}

impl PartialEq for SignersContainerKey {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.ordering == other.ordering
    }
}

impl Eq for SignersContainerKey {}

#[cfg(test)]
mod signers_container_tests {
    use super::*;
    use crate::descriptor;
    use crate::descriptor::IntoWalletDescriptor;
    use crate::keys::{DescriptorKey, IntoDescriptorKey};
    use assert_matches::assert_matches;
    use bitcoin::bip32;
    use bitcoin::secp256k1::{All, Secp256k1};
    use bitcoin::Network;
    use core::str::FromStr;
    use miniscript::ScriptContext;

    fn is_equal(this: &Arc<dyn TransactionSigner>, that: &Arc<DummySigner>) -> bool {
        let secp = Secp256k1::new();
        this.id(&secp) == that.id(&secp)
    }

    // Signers added with the same ordering (like `Ordering::default`) created from `KeyMap`
    // should be preserved and not overwritten.
    // This happens usually when a set of signers is created from a descriptor with private keys.
    #[test]
    fn signers_with_same_ordering() {
        let secp = Secp256k1::new();

        let (prvkey1, _, _) = setup_keys(TPRV0_STR);
        let (prvkey2, _, _) = setup_keys(TPRV1_STR);
        let desc = descriptor!(sh(multi(2, prvkey1, prvkey2))).unwrap();
        let (wallet_desc, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();

        let signers = SignersContainer::build(keymap, &wallet_desc, &secp);
        assert_eq!(signers.ids().len(), 2);

        let signers = signers.signers();
        assert_eq!(signers.len(), 2);
    }

    #[test]
    fn signers_sorted_by_ordering() {
        let mut signers = SignersContainer::new();
        let signer1 = Arc::new(DummySigner { number: 1 });
        let signer2 = Arc::new(DummySigner { number: 2 });
        let signer3 = Arc::new(DummySigner { number: 3 });

        // Mixed order insertions verifies we are not inserting at head or tail.
        signers.add_external(SignerId::Dummy(2), SignerOrdering(2), signer2.clone());
        signers.add_external(SignerId::Dummy(1), SignerOrdering(1), signer1.clone());
        signers.add_external(SignerId::Dummy(3), SignerOrdering(3), signer3.clone());

        // Check that signers are sorted from lowest to highest ordering
        let signers = signers.signers();

        assert!(is_equal(signers[0], &signer1));
        assert!(is_equal(signers[1], &signer2));
        assert!(is_equal(signers[2], &signer3));
    }

    #[test]
    fn find_signer_by_id() {
        let mut signers = SignersContainer::new();
        let signer1 = Arc::new(DummySigner { number: 1 });
        let signer2 = Arc::new(DummySigner { number: 2 });
        let signer3 = Arc::new(DummySigner { number: 3 });
        let signer4 = Arc::new(DummySigner { number: 3 }); // Same ID as `signer3` but will use lower ordering.

        let id1 = SignerId::Dummy(1);
        let id2 = SignerId::Dummy(2);
        let id3 = SignerId::Dummy(3);
        let id_nonexistent = SignerId::Dummy(999);

        signers.add_external(id1.clone(), SignerOrdering(1), signer1.clone());
        signers.add_external(id2.clone(), SignerOrdering(2), signer2.clone());
        signers.add_external(id3.clone(), SignerOrdering(3), signer3.clone());

        assert_matches!(signers.find(id1), Some(signer) if is_equal(signer, &signer1));
        assert_matches!(signers.find(id2), Some(signer) if is_equal(signer, &signer2));
        assert_matches!(signers.find(id3.clone()), Some(signer) if is_equal(signer, &signer3));

        // The `signer4` has the same ID as `signer3` but lower ordering.
        // It should be found by `id3` instead of `signer3`.
        signers.add_external(id3.clone(), SignerOrdering(2), signer4.clone());
        assert_matches!(signers.find(id3), Some(signer) if is_equal(signer, &signer4));

        // Can't find anything with ID that doesn't exist
        assert_matches!(signers.find(id_nonexistent), None);
    }

    #[derive(Debug, Clone, Copy)]
    struct DummySigner {
        number: u64,
    }

    impl SignerCommon for DummySigner {
        fn id(&self, _secp: &SecpCtx) -> SignerId {
            SignerId::Dummy(self.number)
        }
    }

    impl TransactionSigner for DummySigner {
        fn sign_transaction(
            &self,
            _psbt: &mut psbt::PartiallySignedTransaction,
            _sign_options: &SignOptions,
            _secp: &SecpCtx,
        ) -> Result<(), SignerError> {
            Ok(())
        }
    }

    const TPRV0_STR:&str = "tprv8ZgxMBicQKsPdZXrcHNLf5JAJWFAoJ2TrstMRdSKtEggz6PddbuSkvHKM9oKJyFgZV1B7rw8oChspxyYbtmEXYyg1AjfWbL3ho3XHDpHRZf";
    const TPRV1_STR:&str = "tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcLNfjdi5qUvw3VDfgYiH5mNsj5izuiu2N";

    const PATH: &str = "m/44'/1'/0'/0";

    fn setup_keys<Ctx: ScriptContext>(
        tprv: &str,
    ) -> (DescriptorKey<Ctx>, DescriptorKey<Ctx>, Fingerprint) {
        let secp: Secp256k1<All> = Secp256k1::new();
        let path = bip32::DerivationPath::from_str(PATH).unwrap();
        let tprv = bip32::ExtendedPrivKey::from_str(tprv).unwrap();
        let tpub = bip32::ExtendedPubKey::from_priv(&secp, &tprv);
        let fingerprint = tprv.fingerprint(&secp);
        let prvkey = (tprv, path.clone()).into_descriptor_key().unwrap();
        let pubkey = (tpub, path).into_descriptor_key().unwrap();

        (prvkey, pubkey, fingerprint)
    }
}
