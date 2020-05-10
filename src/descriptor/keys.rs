use bitcoin::secp256k1::{All, Secp256k1};
use bitcoin::{PrivateKey, PublicKey};

use bitcoin::util::bip32::{
    ChildNumber, DerivationPath, ExtendedPrivKey, ExtendedPubKey, Fingerprint,
};

use super::error::Error;
use super::extended_key::DerivationIndex;
use super::DescriptorExtendedKey;

pub(super) trait Key: std::fmt::Debug + std::fmt::Display {
    fn fingerprint(&self, secp: &Secp256k1<All>) -> Option<Fingerprint>;
    fn as_public_key(&self, secp: &Secp256k1<All>, index: Option<u32>) -> Result<PublicKey, Error>;
    fn as_secret_key(&self) -> Option<PrivateKey>;
    fn xprv(&self) -> Option<ExtendedPrivKey>;
    fn full_path(&self, index: u32) -> Option<DerivationPath>;
    fn is_fixed(&self) -> bool;

    fn has_secret(&self) -> bool {
        self.xprv().is_some() || self.as_secret_key().is_some()
    }

    fn public(&self, secp: &Secp256k1<All>) -> Result<Box<dyn Key>, Error> {
        Ok(Box::new(self.as_public_key(secp, None)?))
    }
}

impl Key for PublicKey {
    fn fingerprint(&self, _secp: &Secp256k1<All>) -> Option<Fingerprint> {
        None
    }

    fn as_public_key(
        &self,
        _secp: &Secp256k1<All>,
        _index: Option<u32>,
    ) -> Result<PublicKey, Error> {
        Ok(PublicKey::clone(self))
    }

    fn as_secret_key(&self) -> Option<PrivateKey> {
        None
    }

    fn xprv(&self) -> Option<ExtendedPrivKey> {
        None
    }

    fn full_path(&self, _index: u32) -> Option<DerivationPath> {
        None
    }

    fn is_fixed(&self) -> bool {
        true
    }
}

impl Key for PrivateKey {
    fn fingerprint(&self, _secp: &Secp256k1<All>) -> Option<Fingerprint> {
        None
    }

    fn as_public_key(
        &self,
        secp: &Secp256k1<All>,
        _index: Option<u32>,
    ) -> Result<PublicKey, Error> {
        Ok(self.public_key(secp))
    }

    fn as_secret_key(&self) -> Option<PrivateKey> {
        Some(PrivateKey::clone(self))
    }

    fn xprv(&self) -> Option<ExtendedPrivKey> {
        None
    }

    fn full_path(&self, _index: u32) -> Option<DerivationPath> {
        None
    }

    fn is_fixed(&self) -> bool {
        true
    }
}

impl Key for DescriptorExtendedKey {
    fn fingerprint(&self, secp: &Secp256k1<All>) -> Option<Fingerprint> {
        if let Some(fing) = self.master_fingerprint {
            Some(fing.clone())
        } else {
            Some(self.root_xpub(secp).fingerprint())
        }
    }

    fn as_public_key(&self, secp: &Secp256k1<All>, index: Option<u32>) -> Result<PublicKey, Error> {
        Ok(self.derive_xpub(secp, index.unwrap_or(0))?.public_key)
    }

    fn public(&self, secp: &Secp256k1<All>) -> Result<Box<dyn Key>, Error> {
        if self.final_index == DerivationIndex::Hardened {
            return Err(Error::HardenedDerivationOnXpub);
        }

        if self.xprv().is_none() {
            return Ok(Box::new(self.clone()));
        }

        // copy the part of the path that can be derived on the xpub
        let path = self
            .path
            .into_iter()
            .rev()
            .take_while(|child| match child {
                ChildNumber::Normal { .. } => true,
                _ => false,
            })
            .cloned()
            .collect::<Vec<_>>();
        // take the prefix that has to be derived on the xprv
        let master_derivation_add = self
            .path
            .into_iter()
            .take(self.path.as_ref().len() - path.len())
            .cloned()
            .collect::<Vec<_>>();
        let has_derived = !master_derivation_add.is_empty();

        let derived_xprv = self
            .secret
            .as_ref()
            .unwrap()
            .derive_priv(secp, &master_derivation_add)?;
        let pubkey = ExtendedPubKey::from_private(secp, &derived_xprv);

        let master_derivation = self
            .master_derivation
            .as_ref()
            .map_or(vec![], |path| path.as_ref().to_vec())
            .into_iter()
            .chain(master_derivation_add.into_iter())
            .collect::<Vec<_>>();
        let master_derivation = match &master_derivation[..] {
            &[] => None,
            child_vec => Some(child_vec.into()),
        };

        let master_fingerprint = match self.master_fingerprint {
            Some(desc) => Some(desc.clone()),
            None if has_derived => Some(self.fingerprint(secp).unwrap()),
            _ => None,
        };

        Ok(Box::new(DescriptorExtendedKey {
            master_fingerprint,
            master_derivation,
            pubkey,
            secret: None,
            path: path.into(),
            final_index: self.final_index,
        }))
    }

    fn as_secret_key(&self) -> Option<PrivateKey> {
        None
    }

    fn xprv(&self) -> Option<ExtendedPrivKey> {
        self.secret
    }

    fn full_path(&self, index: u32) -> Option<DerivationPath> {
        Some(self.full_path(index))
    }

    fn is_fixed(&self) -> bool {
        self.final_index == DerivationIndex::Fixed
    }
}
