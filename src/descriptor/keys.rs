use std::fmt;
use std::str::FromStr;

use bitcoin::secp256k1::{All, Secp256k1};
use bitcoin::{PrivateKey, PublicKey};

use bitcoin::util::bip32::{
    ChildNumber, DerivationPath, ExtendedPrivKey, ExtendedPubKey, Fingerprint,
};

use super::error::Error;
use super::extended_key::DerivationIndex;
use super::DescriptorExtendedKey;

#[derive(Debug, Clone)]
pub struct KeyAlias {
    alias: String,
    has_secret: bool,
}

impl KeyAlias {
    pub(crate) fn new_boxed(alias: &str, has_secret: bool) -> Box<dyn Key> {
        Box::new(KeyAlias {
            alias: alias.into(),
            has_secret,
        })
    }
}

pub(crate) fn parse_key(string: &str) -> Result<(String, Box<dyn RealKey>), Error> {
    if let Ok(pk) = PublicKey::from_str(string) {
        return Ok((string.to_string(), Box::new(pk)));
    } else if let Ok(sk) = PrivateKey::from_wif(string) {
        return Ok((string.to_string(), Box::new(sk)));
    } else if let Ok(ext_key) = DescriptorExtendedKey::from_str(string) {
        return Ok((string.to_string(), Box::new(ext_key)));
    }

    return Err(Error::KeyParsingError(string.to_string()));
}

pub trait Key: std::fmt::Debug + std::fmt::Display {
    fn as_public_key(&self, secp: &Secp256k1<All>, index: Option<u32>) -> Result<PublicKey, Error>;
    fn is_fixed(&self) -> bool;

    fn alias(&self) -> Option<&str> {
        None
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

    fn fingerprint(&self, _secp: &Secp256k1<All>) -> Option<Fingerprint> {
        None
    }

    fn has_secret(&self) -> bool {
        self.xprv().is_some() || self.as_secret_key().is_some()
    }

    fn public(&self, secp: &Secp256k1<All>) -> Result<Box<dyn RealKey>, Error> {
        Ok(Box::new(self.as_public_key(secp, None)?))
    }
}

pub trait RealKey: Key {
    fn into_key(&self) -> Box<dyn Key>;
}

impl<T: RealKey + 'static> From<T> for Box<dyn RealKey> {
    fn from(key: T) -> Self {
        Box::new(key)
    }
}

impl Key for PublicKey {
    fn as_public_key(
        &self,
        _secp: &Secp256k1<All>,
        _index: Option<u32>,
    ) -> Result<PublicKey, Error> {
        Ok(PublicKey::clone(self))
    }

    fn is_fixed(&self) -> bool {
        true
    }
}

impl RealKey for PublicKey {
    fn into_key(&self) -> Box<dyn Key> {
        Box::new(self.clone())
    }
}

impl Key for PrivateKey {
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

    fn is_fixed(&self) -> bool {
        true
    }
}
impl RealKey for PrivateKey {
    fn into_key(&self) -> Box<dyn Key> {
        Box::new(self.clone())
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

    fn public(&self, secp: &Secp256k1<All>) -> Result<Box<dyn RealKey>, Error> {
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
impl RealKey for DescriptorExtendedKey {
    fn into_key(&self) -> Box<dyn Key> {
        Box::new(self.clone())
    }
}

impl std::fmt::Display for KeyAlias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let flag = if self.has_secret { "#" } else { "" };

        write!(f, "{}{}", flag, self.alias)
    }
}

impl Key for KeyAlias {
    fn as_public_key(
        &self,
        _secp: &Secp256k1<All>,
        _index: Option<u32>,
    ) -> Result<PublicKey, Error> {
        Err(Error::AliasAsPublicKey)
    }

    fn is_fixed(&self) -> bool {
        true
    }

    fn alias(&self) -> Option<&str> {
        Some(self.alias.as_str())
    }

    fn has_secret(&self) -> bool {
        self.has_secret
    }

    fn public(&self, _secp: &Secp256k1<All>) -> Result<Box<dyn RealKey>, Error> {
        Err(Error::AliasAsPublicKey)
    }
}

#[derive(Debug, Clone, Hash, PartialEq, PartialOrd, Eq, Ord, Default)]
pub(crate) struct DummyKey();

impl fmt::Display for DummyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DummyKey")
    }
}

impl std::str::FromStr for DummyKey {
    type Err = ();

    fn from_str(_: &str) -> Result<Self, Self::Err> {
        Ok(DummyKey::default())
    }
}

impl miniscript::MiniscriptKey for DummyKey {
    type Hash = DummyKey;

    fn to_pubkeyhash(&self) -> DummyKey {
        DummyKey::default()
    }
}
