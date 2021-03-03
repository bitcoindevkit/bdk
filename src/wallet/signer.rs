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
//! # use std::sync::Arc;
//! # use std::str::FromStr;
//! # use bitcoin::secp256k1::{Secp256k1, All};
//! # use bitcoin::*;
//! # use bitcoin::util::psbt;
//! # use bdk::signer::*;
//! # use bdk::database::*;
//! # use bdk::*;
//! # #[derive(Debug)]
//! # struct CustomHSM;
//! # impl CustomHSM {
//! #     fn sign_input(&self, _psbt: &mut psbt::PartiallySignedTransaction, _input: usize) -> Result<(), SignerError> {
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
//! impl Signer for CustomSigner {
//!     fn sign(
//!         &self,
//!         psbt: &mut psbt::PartiallySignedTransaction,
//!         input_index: Option<usize>,
//!         _secp: &Secp256k1<All>,
//!     ) -> Result<(), SignerError> {
//!         let input_index = input_index.ok_or(SignerError::InputIndexOutOfRange)?;
//!         self.device.sign_input(psbt, input_index)?;
//!
//!         Ok(())
//!     }
//!
//!     fn id(&self, _secp: &Secp256k1<All>) -> SignerId {
//!         self.device.get_id()
//!     }
//!
//!     fn sign_whole_tx(&self) -> bool {
//!         false
//!     }
//! }
//!
//! let custom_signer = CustomSigner::connect();
//!
//! let descriptor = "wpkh(tpubD6NzVbkrYhZ4Xferm7Pz4VnjdcDPFyjVu5K4iZXQ4pVN8Cks4pHVowTBXBKRhX64pkRyJZJN5xAKj4UDNnLPb5p2sSKXhewoYx5GbTdUFWq/*)";
//! let mut wallet = Wallet::new_offline(descriptor, None, Network::Testnet, MemoryDatabase::default())?;
//! wallet.add_signer(
//!     KeychainKind::External,
//!     SignerOrdering(200),
//!     Arc::new(custom_signer)
//! );
//!
//! # Ok::<_, bdk::Error>(())
//! ```

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::ops::Bound::Included;
use std::sync::Arc;

use bitcoin::blockdata::opcodes;
use bitcoin::blockdata::script::Builder as ScriptBuilder;
use bitcoin::hashes::{hash160, Hash};
use bitcoin::secp256k1::{Message, Secp256k1};
use bitcoin::util::bip32::{ExtendedPrivKey, Fingerprint};
use bitcoin::util::{bip143, psbt};
use bitcoin::{PrivateKey, Script, SigHash, SigHashType};

use miniscript::descriptor::{DescriptorSecretKey, DescriptorSinglePriv, DescriptorXKey, KeyMap};
use miniscript::{Legacy, MiniscriptKey, Segwitv0};

use super::utils::SecpCtx;
use crate::descriptor::XKeyUtils;

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
#[derive(Debug, PartialEq, Eq, Clone)]
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
    /// The `witness_script` field of the transaction is requied to sign this input
    MissingWitnessScript,
    /// The fingerprint and derivation path are missing from the psbt input
    MissingHDKeypath,
}

impl fmt::Display for SignerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for SignerError {}

/// Trait for signers
///
/// This trait can be implemented to provide customized signers to the wallet. For an example see
/// [`this module`](crate::wallet::signer)'s documentation.
pub trait Signer: fmt::Debug + Send + Sync {
    /// Sign a PSBT
    ///
    /// The `input_index` argument is only provided if the wallet doesn't declare to sign the whole
    /// transaction in one go (see [`Signer::sign_whole_tx`]). Otherwise its value is `None` and
    /// can be ignored.
    fn sign(
        &self,
        psbt: &mut psbt::PartiallySignedTransaction,
        input_index: Option<usize>,
        secp: &SecpCtx,
    ) -> Result<(), SignerError>;

    /// Return whether or not the signer signs the whole transaction in one go instead of every
    /// input individually
    fn sign_whole_tx(&self) -> bool;

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

impl Signer for DescriptorXKey<ExtendedPrivKey> {
    fn sign(
        &self,
        psbt: &mut psbt::PartiallySignedTransaction,
        input_index: Option<usize>,
        secp: &SecpCtx,
    ) -> Result<(), SignerError> {
        let input_index = input_index.unwrap();
        if input_index >= psbt.inputs.len() {
            return Err(SignerError::InputIndexOutOfRange);
        }

        let (public_key, deriv_path) = match psbt.inputs[input_index]
            .bip32_derivation
            .iter()
            .filter_map(|(pk, &(fingerprint, ref path))| {
                if self.matches(&(fingerprint, path.clone()), &secp).is_some() {
                    Some((pk, path))
                } else {
                    None
                }
            })
            .next()
        {
            Some((pk, full_path)) => (pk, full_path.clone()),
            None => return Ok(()),
        };

        let derived_key = self.xkey.derive_priv(&secp, &deriv_path).unwrap();
        if &derived_key.private_key.public_key(&secp) != public_key {
            Err(SignerError::InvalidKey)
        } else {
            derived_key.private_key.sign(psbt, Some(input_index), secp)
        }
    }

    fn sign_whole_tx(&self) -> bool {
        false
    }

    fn id(&self, secp: &SecpCtx) -> SignerId {
        SignerId::from(self.root_fingerprint(&secp))
    }

    fn descriptor_secret_key(&self) -> Option<DescriptorSecretKey> {
        Some(DescriptorSecretKey::XPrv(self.clone()))
    }
}

impl Signer for PrivateKey {
    fn sign(
        &self,
        psbt: &mut psbt::PartiallySignedTransaction,
        input_index: Option<usize>,
        secp: &SecpCtx,
    ) -> Result<(), SignerError> {
        let input_index = input_index.unwrap();
        if input_index >= psbt.inputs.len() {
            return Err(SignerError::InputIndexOutOfRange);
        }

        let pubkey = self.public_key(&secp);
        if psbt.inputs[input_index].partial_sigs.contains_key(&pubkey) {
            return Ok(());
        }

        // FIXME: use the presence of `witness_utxo` as an indication that we should make a bip143
        // sig. Does this make sense? Should we add an extra argument to explicitly swith between
        // these? The original idea was to declare sign() as sign<Ctx: ScriptContex>() and use Ctx,
        // but that violates the rules for trait-objects, so we can't do it.
        let (hash, sighash) = match psbt.inputs[input_index].witness_utxo {
            Some(_) => Segwitv0::sighash(psbt, input_index)?,
            None => Legacy::sighash(psbt, input_index)?,
        };

        let signature = secp.sign(
            &Message::from_slice(&hash.into_inner()[..]).unwrap(),
            &self.key,
        );

        let mut final_signature = Vec::with_capacity(75);
        final_signature.extend_from_slice(&signature.serialize_der());
        final_signature.push(sighash.as_u32() as u8);

        psbt.inputs[input_index]
            .partial_sigs
            .insert(pubkey, final_signature);

        Ok(())
    }

    fn sign_whole_tx(&self) -> bool {
        false
    }

    fn id(&self, secp: &SecpCtx) -> SignerId {
        SignerId::from(self.public_key(secp).to_pubkeyhash())
    }

    fn descriptor_secret_key(&self) -> Option<DescriptorSecretKey> {
        Some(DescriptorSecretKey::SinglePriv(DescriptorSinglePriv {
            key: *self,
            origin: None,
        }))
    }
}

/// Defines the order in which signers are called
///
/// The default value is `100`. Signers with an ordering above that will be called later,
/// and they will thus see the partial signatures added to the transaction once they get to sign
/// themselves.
#[derive(Debug, Clone, PartialOrd, PartialEq, Ord, Eq)]
pub struct SignerOrdering(pub usize);

impl std::default::Default for SignerOrdering {
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
pub struct SignersContainer(BTreeMap<SignersContainerKey, Arc<dyn Signer>>);

impl SignersContainer {
    /// Create a map of public keys to secret keys
    pub fn as_key_map(&self, secp: &SecpCtx) -> KeyMap {
        self.0
            .values()
            .filter_map(|signer| signer.descriptor_secret_key())
            .filter_map(|secret| secret.as_public(secp).ok().map(|public| (public, secret)))
            .collect()
    }
}

impl From<KeyMap> for SignersContainer {
    fn from(keymap: KeyMap) -> SignersContainer {
        let secp = Secp256k1::new();
        let mut container = SignersContainer::new();

        for (_, secret) in keymap {
            match secret {
                DescriptorSecretKey::SinglePriv(private_key) => container.add_external(
                    SignerId::from(private_key.key.public_key(&secp).to_pubkeyhash()),
                    SignerOrdering::default(),
                    Arc::new(private_key.key),
                ),
                DescriptorSecretKey::XPrv(xprv) => container.add_external(
                    SignerId::from(xprv.root_fingerprint(&secp)),
                    SignerOrdering::default(),
                    Arc::new(xprv),
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
        signer: Arc<dyn Signer>,
    ) -> Option<Arc<dyn Signer>> {
        self.0.insert((id, ordering).into(), signer)
    }

    /// Removes a signer from the container and returns it
    pub fn remove(&mut self, id: SignerId, ordering: SignerOrdering) -> Option<Arc<dyn Signer>> {
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
    pub fn signers(&self) -> Vec<&Arc<dyn Signer>> {
        self.0.values().collect()
    }

    /// Finds the signer with lowest ordering for a given id in the container.
    pub fn find(&self, id: SignerId) -> Option<&Arc<dyn Signer>> {
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

pub(crate) trait ComputeSighash {
    fn sighash(
        psbt: &psbt::PartiallySignedTransaction,
        input_index: usize,
    ) -> Result<(SigHash, SigHashType), SignerError>;
}

impl ComputeSighash for Legacy {
    fn sighash(
        psbt: &psbt::PartiallySignedTransaction,
        input_index: usize,
    ) -> Result<(SigHash, SigHashType), SignerError> {
        if input_index >= psbt.inputs.len() {
            return Err(SignerError::InputIndexOutOfRange);
        }

        let psbt_input = &psbt.inputs[input_index];
        let tx_input = &psbt.global.unsigned_tx.input[input_index];

        let sighash = psbt_input.sighash_type.unwrap_or(SigHashType::All);
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
            psbt.global
                .unsigned_tx
                .signature_hash(input_index, &script, sighash.as_u32()),
            sighash,
        ))
    }
}

fn p2wpkh_script_code(script: &Script) -> Script {
    ScriptBuilder::new()
        .push_opcode(opcodes::all::OP_DUP)
        .push_opcode(opcodes::all::OP_HASH160)
        .push_slice(&script[2..])
        .push_opcode(opcodes::all::OP_EQUALVERIFY)
        .push_opcode(opcodes::all::OP_CHECKSIG)
        .into_script()
}

impl ComputeSighash for Segwitv0 {
    fn sighash(
        psbt: &psbt::PartiallySignedTransaction,
        input_index: usize,
    ) -> Result<(SigHash, SigHashType), SignerError> {
        if input_index >= psbt.inputs.len() {
            return Err(SignerError::InputIndexOutOfRange);
        }

        let psbt_input = &psbt.inputs[input_index];

        let sighash = psbt_input.sighash_type.unwrap_or(SigHashType::All);

        let witness_utxo = psbt_input
            .witness_utxo
            .as_ref()
            .ok_or(SignerError::MissingNonWitnessUtxo)?;
        let value = witness_utxo.value;

        let script = match psbt_input.witness_script {
            Some(ref witness_script) => witness_script.clone(),
            None => {
                if witness_utxo.script_pubkey.is_v0_p2wpkh() {
                    p2wpkh_script_code(&witness_utxo.script_pubkey)
                } else if psbt_input
                    .redeem_script
                    .as_ref()
                    .map(Script::is_v0_p2wpkh)
                    .unwrap_or(false)
                {
                    p2wpkh_script_code(&psbt_input.redeem_script.as_ref().unwrap())
                } else {
                    return Err(SignerError::MissingWitnessScript);
                }
            }
        };

        Ok((
            bip143::SigHashCache::new(&psbt.global.unsigned_tx).signature_hash(
                input_index,
                &script,
                value,
                sighash,
            ),
            sighash,
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
    use bitcoin::secp256k1::{All, Secp256k1};
    use bitcoin::util::bip32;
    use bitcoin::util::psbt::PartiallySignedTransaction;
    use bitcoin::Network;
    use miniscript::ScriptContext;
    use std::str::FromStr;

    fn is_equal(this: &Arc<dyn Signer>, that: &Arc<DummySigner>) -> bool {
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
        let (_, keymap) = desc
            .into_wallet_descriptor(&secp, Network::Testnet)
            .unwrap();

        let signers = SignersContainer::from(keymap);
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

        assert!(matches!(signers.find(id1), Some(signer) if is_equal(signer, &signer1)));
        assert!(matches!(signers.find(id2), Some(signer) if is_equal(signer, &signer2)));
        assert!(matches!(signers.find(id3.clone()), Some(signer) if is_equal(signer, &signer3)));

        // The `signer4` has the same ID as `signer3` but lower ordering.
        // It should be found by `id3` instead of `signer3`.
        signers.add_external(id3.clone(), SignerOrdering(2), signer4.clone());
        assert!(matches!(signers.find(id3), Some(signer) if is_equal(signer, &signer4)));

        // Can't find anything with ID that doesn't exist
        assert!(matches!(signers.find(id_nonexistent), None));
    }

    #[derive(Debug, Clone, Copy)]
    struct DummySigner {
        number: u64,
    }

    impl Signer for DummySigner {
        fn sign(
            &self,
            _psbt: &mut PartiallySignedTransaction,
            _input_index: Option<usize>,
            _secp: &SecpCtx,
        ) -> Result<(), SignerError> {
            Ok(())
        }

        fn id(&self, _secp: &SecpCtx) -> SignerId {
            SignerId::Dummy(self.number)
        }

        fn sign_whole_tx(&self) -> bool {
            true
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
        let tpub = bip32::ExtendedPubKey::from_private(&secp, &tprv);
        let fingerprint = tprv.fingerprint(&secp);
        let prvkey = (tprv, path.clone()).into_descriptor_key().unwrap();
        let pubkey = (tpub, path).into_descriptor_key().unwrap();

        (prvkey, pubkey, fingerprint)
    }
}
