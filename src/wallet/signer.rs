use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use bitcoin::blockdata::opcodes;
use bitcoin::blockdata::script::Builder as ScriptBuilder;
use bitcoin::hashes::{hash160, Hash};
use bitcoin::secp256k1::{Message, Secp256k1};
use bitcoin::util::bip32::{ExtendedPrivKey, Fingerprint};
use bitcoin::util::{bip143, psbt};
use bitcoin::{PrivateKey, SigHash, SigHashType};

use miniscript::descriptor::{DescriptorPublicKey, DescriptorSecretKey, DescriptorXKey, KeyMap};
use miniscript::{Legacy, MiniscriptKey, Segwitv0};

use crate::descriptor::XKeyUtils;

/// Identifier of a signer in the `SignersContainers`. Used as a key to find the right signer among
/// many of them
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SignerId<Pk: MiniscriptKey> {
    PkHash(<Pk as MiniscriptKey>::Hash),
    Fingerprint(Fingerprint),
}

impl From<hash160::Hash> for SignerId<DescriptorPublicKey> {
    fn from(hash: hash160::Hash) -> SignerId<DescriptorPublicKey> {
        SignerId::PkHash(hash)
    }
}

impl From<Fingerprint> for SignerId<DescriptorPublicKey> {
    fn from(fing: Fingerprint) -> SignerId<DescriptorPublicKey> {
        SignerId::Fingerprint(fing)
    }
}

/// Signing error
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum SignerError {
    /// The private key is missing for the required public key
    MissingKey,
    /// The user canceled the operation
    UserCanceled,
    /// The sighash is missing in the PSBT input
    MissingSighash,
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

/// Trait for signers
pub trait Signer: fmt::Debug {
    fn sign(
        &self,
        psbt: &mut psbt::PartiallySignedTransaction,
        input_index: usize,
    ) -> Result<(), SignerError>;

    fn descriptor_secret_key(&self) -> Option<DescriptorSecretKey> {
        None
    }
}

impl Signer for DescriptorXKey<ExtendedPrivKey> {
    fn sign(
        &self,
        psbt: &mut psbt::PartiallySignedTransaction,
        input_index: usize,
    ) -> Result<(), SignerError> {
        if input_index >= psbt.inputs.len() {
            return Err(SignerError::InputIndexOutOfRange);
        }

        let deriv_path = match psbt.inputs[input_index]
            .hd_keypaths
            .iter()
            .filter_map(|(_, &(fingerprint, ref path))| self.matches(fingerprint.clone(), &path))
            .next()
        {
            Some(deriv_path) => deriv_path,
            None => return Ok(()), // TODO: should report an error maybe?
        };

        let ctx = Secp256k1::signing_only();

        let derived_key = self.xkey.derive_priv(&ctx, &deriv_path).unwrap();
        derived_key.private_key.sign(psbt, input_index)
    }

    fn descriptor_secret_key(&self) -> Option<DescriptorSecretKey> {
        Some(DescriptorSecretKey::XPrv(self.clone()))
    }
}

impl Signer for PrivateKey {
    fn sign(
        &self,
        psbt: &mut psbt::PartiallySignedTransaction,
        input_index: usize,
    ) -> Result<(), SignerError> {
        if input_index >= psbt.inputs.len() {
            return Err(SignerError::InputIndexOutOfRange);
        }

        let ctx = Secp256k1::signing_only();

        let pubkey = self.public_key(&ctx);
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

        let signature = ctx.sign(
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

    fn descriptor_secret_key(&self) -> Option<DescriptorSecretKey> {
        Some(DescriptorSecretKey::PrivKey(self.clone()))
    }
}

/// Container for multiple signers
#[derive(Debug, Default, Clone)]
pub struct SignersContainer<Pk: MiniscriptKey>(HashMap<SignerId<Pk>, Arc<Box<dyn Signer>>>);

impl SignersContainer<DescriptorPublicKey> {
    pub fn as_key_map(&self) -> KeyMap {
        self.0
            .values()
            .filter_map(|signer| signer.descriptor_secret_key())
            .filter_map(|secret| secret.as_public().ok().map(|public| (public, secret)))
            .collect()
    }
}

impl<Pk: MiniscriptKey + Any> Signer for SignersContainer<Pk> {
    fn sign(
        &self,
        psbt: &mut psbt::PartiallySignedTransaction,
        input_index: usize,
    ) -> Result<(), SignerError> {
        for signer in self.0.values() {
            signer.sign(psbt, input_index)?;
        }

        Ok(())
    }
}

impl From<KeyMap> for SignersContainer<DescriptorPublicKey> {
    fn from(keymap: KeyMap) -> SignersContainer<DescriptorPublicKey> {
        let mut container = SignersContainer::new();

        for (_, secret) in keymap {
            match secret {
                DescriptorSecretKey::PrivKey(private_key) => container.add_external(
                    SignerId::from(
                        private_key
                            .public_key(&Secp256k1::signing_only())
                            .to_pubkeyhash(),
                    ),
                    Arc::new(Box::new(private_key)),
                ),
                DescriptorSecretKey::XPrv(xprv) => container.add_external(
                    SignerId::from(xprv.root_fingerprint()),
                    Arc::new(Box::new(xprv)),
                ),
            };
        }

        container
    }
}

impl<Pk: MiniscriptKey> SignersContainer<Pk> {
    /// Default constructor
    pub fn new() -> Self {
        SignersContainer(HashMap::new())
    }

    /// Adds an external signer to the container for the specified id. Optionally returns the
    /// signer that was previosuly in the container, if any
    pub fn add_external(
        &mut self,
        id: SignerId<Pk>,
        signer: Arc<Box<dyn Signer>>,
    ) -> Option<Arc<Box<dyn Signer>>> {
        self.0.insert(id, signer)
    }

    /// Removes a signer from the container and returns it
    pub fn remove(&mut self, id: SignerId<Pk>) -> Option<Arc<Box<dyn Signer>>> {
        self.0.remove(&id)
    }

    /// Returns the list of identifiers of all the signers in the container
    pub fn ids(&self) -> Vec<&SignerId<Pk>> {
        self.0.keys().collect()
    }

    /// Finds the signer with a given id in the container
    pub fn find(&self, id: SignerId<Pk>) -> Option<&Arc<Box<dyn Signer>>> {
        self.0.get(&id)
    }
}

pub trait ComputeSighash {
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

        let sighash = psbt_input.sighash_type.ok_or(SignerError::MissingSighash)?;
        let script = match &psbt_input.redeem_script {
            &Some(ref redeem_script) => redeem_script.clone(),
            &None => {
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

impl ComputeSighash for Segwitv0 {
    fn sighash(
        psbt: &psbt::PartiallySignedTransaction,
        input_index: usize,
    ) -> Result<(SigHash, SigHashType), SignerError> {
        if input_index >= psbt.inputs.len() {
            return Err(SignerError::InputIndexOutOfRange);
        }

        let psbt_input = &psbt.inputs[input_index];

        let sighash = psbt_input.sighash_type.ok_or(SignerError::MissingSighash)?;

        let witness_utxo = psbt_input
            .witness_utxo
            .as_ref()
            .ok_or(SignerError::MissingNonWitnessUtxo)?;
        let value = witness_utxo.value;

        let script = match &psbt_input.witness_script {
            &Some(ref witness_script) => witness_script.clone(),
            &None => {
                if witness_utxo.script_pubkey.is_v0_p2wpkh() {
                    ScriptBuilder::new()
                        .push_opcode(opcodes::all::OP_DUP)
                        .push_opcode(opcodes::all::OP_HASH160)
                        .push_slice(&witness_utxo.script_pubkey[2..])
                        .push_opcode(opcodes::all::OP_EQUALVERIFY)
                        .push_opcode(opcodes::all::OP_CHECKSIG)
                        .into_script()
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
