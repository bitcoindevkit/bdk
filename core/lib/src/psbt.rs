use std::collections::BTreeMap;

use bitcoin::hashes::Hash;
use bitcoin::util::bip143::SighashComponents;
use bitcoin::util::bip32::{DerivationPath, ExtendedPrivKey, Fingerprint};
use bitcoin::util::psbt;
use bitcoin::{PrivateKey, PublicKey, Script, SigHashType, Transaction};

use bitcoin::secp256k1::{self, All, Message, Secp256k1};

use miniscript::{BitcoinSig, Satisfier};

use crate::descriptor::ExtendedDescriptor;
use crate::error::Error;
use crate::signer::Signer;

pub struct PSBTSatisfier<'a> {
    input: &'a psbt::Input,
    create_height: Option<u32>,
    current_height: Option<u32>,
}

impl<'a> PSBTSatisfier<'a> {
    pub fn new(
        input: &'a psbt::Input,
        create_height: Option<u32>,
        current_height: Option<u32>,
    ) -> Self {
        PSBTSatisfier {
            input,
            create_height,
            current_height,
        }
    }
}

// TODO: also support hash preimages through the "unknown" section of PSBT
impl<'a> Satisfier<bitcoin::PublicKey> for PSBTSatisfier<'a> {
    // from https://docs.rs/miniscript/0.12.0/src/miniscript/psbt/mod.rs.html#96
    fn lookup_sig(&self, pk: &bitcoin::PublicKey) -> Option<BitcoinSig> {
        if let Some(rawsig) = self.input.partial_sigs.get(pk) {
            let (flag, sig) = rawsig.split_last().unwrap();
            let flag = bitcoin::SigHashType::from_u32(*flag as u32);
            let sig = match secp256k1::Signature::from_der(sig) {
                Ok(sig) => sig,
                Err(..) => return None,
            };
            Some((sig, flag))
        } else {
            None
        }
    }

    fn check_older(&self, height: u32) -> bool {
        self.current_height.unwrap_or(0) >= self.create_height.unwrap_or(0) + height
    }

    fn check_after(&self, height: u32) -> bool {
        // TODO: gt or gte ?
        self.current_height.unwrap_or(0) >= height
    }
}

#[derive(Debug)]
pub struct PSBTSigner<'a> {
    tx: &'a Transaction,
    secp: Secp256k1<All>,

    // psbt: &'b psbt::PartiallySignedTransaction,
    extended_keys: BTreeMap<Fingerprint, ExtendedPrivKey>,
    private_keys: BTreeMap<PublicKey, PrivateKey>,
}

impl<'a> PSBTSigner<'a> {
    pub fn from_descriptor(tx: &'a Transaction, desc: &ExtendedDescriptor) -> Result<Self, Error> {
        let secp = Secp256k1::gen_new();

        let mut extended_keys = BTreeMap::new();
        for xprv in desc.get_xprv() {
            let fing = xprv.fingerprint(&secp);
            extended_keys.insert(fing, xprv);
        }

        let mut private_keys = BTreeMap::new();
        for privkey in desc.get_secret_keys() {
            let pubkey = privkey.public_key(&secp);
            private_keys.insert(pubkey, privkey);
        }

        Ok(PSBTSigner {
            tx,
            secp,
            extended_keys,
            private_keys,
        })
    }
}

impl<'a> Signer for PSBTSigner<'a> {
    fn sig_legacy_from_fingerprint(
        &self,
        index: usize,
        sighash: SigHashType,
        fingerprint: &Fingerprint,
        path: &DerivationPath,
        script: &Script,
    ) -> Result<Option<BitcoinSig>, Error> {
        self.extended_keys
            .get(fingerprint)
            .map_or(Ok(None), |xprv| {
                let privkey = xprv.derive_priv(&self.secp, path)?;
                // let derived_pubkey = secp256k1::PublicKey::from_secret_key(&self.secp, &privkey.private_key.key);

                let hash = self.tx.signature_hash(index, script, sighash.as_u32());

                let signature = self.secp.sign(
                    &Message::from_slice(&hash.into_inner()[..])?,
                    &privkey.private_key.key,
                );

                Ok(Some((signature, sighash)))
            })
    }

    fn sig_legacy_from_pubkey(
        &self,
        index: usize,
        sighash: SigHashType,
        public_key: &PublicKey,
        script: &Script,
    ) -> Result<Option<BitcoinSig>, Error> {
        self.private_keys
            .get(public_key)
            .map_or(Ok(None), |privkey| {
                let hash = self.tx.signature_hash(index, script, sighash.as_u32());

                let signature = self
                    .secp
                    .sign(&Message::from_slice(&hash.into_inner()[..])?, &privkey.key);

                Ok(Some((signature, sighash)))
            })
    }

    fn sig_segwit_from_fingerprint(
        &self,
        index: usize,
        sighash: SigHashType,
        fingerprint: &Fingerprint,
        path: &DerivationPath,
        script: &Script,
        value: u64,
    ) -> Result<Option<BitcoinSig>, Error> {
        self.extended_keys
            .get(fingerprint)
            .map_or(Ok(None), |xprv| {
                let privkey = xprv.derive_priv(&self.secp, path)?;

                let hash = SighashComponents::new(self.tx).sighash_all(
                    &self.tx.input[index],
                    script,
                    value,
                );

                let signature = self.secp.sign(
                    &Message::from_slice(&hash.into_inner()[..])?,
                    &privkey.private_key.key,
                );

                Ok(Some((signature, sighash)))
            })
    }

    fn sig_segwit_from_pubkey(
        &self,
        index: usize,
        sighash: SigHashType,
        public_key: &PublicKey,
        script: &Script,
        value: u64,
    ) -> Result<Option<BitcoinSig>, Error> {
        self.private_keys
            .get(public_key)
            .map_or(Ok(None), |privkey| {
                let hash = SighashComponents::new(self.tx).sighash_all(
                    &self.tx.input[index],
                    script,
                    value,
                );

                let signature = self
                    .secp
                    .sign(&Message::from_slice(&hash.into_inner()[..])?, &privkey.key);

                Ok(Some((signature, sighash)))
            })
    }
}
