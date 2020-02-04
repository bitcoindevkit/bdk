use std::cell::RefCell;
use std::collections::BTreeMap;
use std::convert::{Into, TryFrom};
use std::fmt;
use std::str::FromStr;

use bitcoin::blockdata::script::Script;
use bitcoin::hashes::{hash160, Hash};
use bitcoin::secp256k1::{All, Secp256k1};
use bitcoin::util::bip32::{DerivationPath, ExtendedPrivKey, Fingerprint};
use bitcoin::{PrivateKey, PublicKey};

pub use miniscript::descriptor::Descriptor;

use serde::{Deserialize, Serialize};

pub mod error;
pub mod extended_key;

pub use self::error::Error;
pub use self::extended_key::DescriptorExtendedKey;

#[derive(Debug, Clone, Hash, PartialEq, PartialOrd, Eq, Ord, Default)]
struct DummyKey();

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

pub type DerivedDescriptor = Descriptor<PublicKey>;
pub type StringDescriptor = Descriptor<String>;

pub trait DescriptorMeta {
    fn is_witness(&self) -> bool;
    fn psbt_redeem_script(&self) -> Option<Script>;
    fn psbt_witness_script(&self) -> Option<Script>;
}

impl<T> DescriptorMeta for Descriptor<T>
where
    T: miniscript::MiniscriptKey + miniscript::ToPublicKey,
{
    fn is_witness(&self) -> bool {
        match self {
            Descriptor::Bare(_) | Descriptor::Pk(_) | Descriptor::Pkh(_) | Descriptor::Sh(_) => {
                false
            }
            Descriptor::Wpkh(_)
            | Descriptor::ShWpkh(_)
            | Descriptor::Wsh(_)
            | Descriptor::ShWsh(_) => true,
        }
    }

    fn psbt_redeem_script(&self) -> Option<Script> {
        match self {
            Descriptor::ShWpkh(ref pk) => {
                let addr =
                    bitcoin::Address::p2shwpkh(&pk.to_public_key(), bitcoin::Network::Bitcoin);
                Some(addr.script_pubkey())
            }
            Descriptor::ShWsh(ref script) => Some(script.encode().to_v0_p2wsh()),
            Descriptor::Sh(ref script) => Some(script.encode()),
            _ => None,
        }
    }

    fn psbt_witness_script(&self) -> Option<Script> {
        match self {
            Descriptor::Wsh(ref script) => Some(script.encode()),
            _ => None,
        }
    }
}

trait Key: std::fmt::Debug {
    fn fingerprint(&self, secp: &Secp256k1<All>) -> Option<Fingerprint>;
    fn as_public_key(&self, secp: &Secp256k1<All>, index: Option<u32>) -> Result<PublicKey, Error>;
    fn as_secret_key(&self) -> Option<PrivateKey>;
    fn xprv(&self) -> Option<ExtendedPrivKey>;
    fn full_path(&self, index: u32) -> Option<DerivationPath>;
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
}

impl Key for DescriptorExtendedKey {
    fn fingerprint(&self, secp: &Secp256k1<All>) -> Option<Fingerprint> {
        Some(self.root_xpub(secp).fingerprint())
    }

    fn as_public_key(&self, secp: &Secp256k1<All>, index: Option<u32>) -> Result<PublicKey, Error> {
        Ok(self.derive_xpub(secp, index.unwrap_or(0))?.public_key)
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
}

#[serde(try_from = "&str", into = "String")]
#[derive(Debug, Serialize, Deserialize)]
pub struct ExtendedDescriptor {
    #[serde(flatten)]
    internal: StringDescriptor,

    #[serde(skip)]
    keys: BTreeMap<String, Box<dyn Key>>,

    #[serde(skip)]
    ctx: Secp256k1<All>,
}

impl std::clone::Clone for ExtendedDescriptor {
    fn clone(&self) -> Self {
        Self {
            internal: self.internal.clone(),
            ctx: self.ctx.clone(),
            keys: BTreeMap::new(),
        }
    }
}

impl ExtendedDescriptor {
    fn parse_string(string: &str) -> Result<(String, Box<dyn Key>), Error> {
        if let Ok(pk) = PublicKey::from_str(string) {
            return Ok((string.to_string(), Box::new(pk)));
        } else if let Ok(sk) = PrivateKey::from_wif(string) {
            return Ok((string.to_string(), Box::new(sk)));
        } else if let Ok(ext_key) = DescriptorExtendedKey::from_str(string) {
            return Ok((string.to_string(), Box::new(ext_key)));
        }

        return Err(Error::KeyParsingError(string.to_string()));
    }

    fn new(sd: StringDescriptor) -> Result<Self, Error> {
        let ctx = Secp256k1::gen_new();
        let keys: RefCell<BTreeMap<String, Box<dyn Key>>> = RefCell::new(BTreeMap::new());

        let translatefpk = |string: &String| -> Result<_, Error> {
            let (key, parsed) = Self::parse_string(string)?;
            keys.borrow_mut().insert(key, parsed);

            Ok(DummyKey::default())
        };
        let translatefpkh = |string: &String| -> Result<_, Error> {
            let (key, parsed) = Self::parse_string(string)?;
            keys.borrow_mut().insert(key, parsed);

            Ok(DummyKey::default())
        };

        sd.translate_pk(translatefpk, translatefpkh)?;

        Ok(ExtendedDescriptor {
            internal: sd,
            keys: keys.into_inner(),
            ctx,
        })
    }

    pub fn derive(&self, index: u32) -> Result<DerivedDescriptor, Error> {
        let translatefpk = |xpub: &String| {
            self.keys
                .get(xpub)
                .unwrap()
                .as_public_key(&self.ctx, Some(index))
        };
        let translatefpkh =
            |xpub: &String| Ok(hash160::Hash::hash(&translatefpk(xpub)?.to_bytes()));

        Ok(self.internal.translate_pk(translatefpk, translatefpkh)?)
    }

    pub fn get_xprv(&self) -> Vec<ExtendedPrivKey> {
        self.keys
            .iter()
            .filter(|(_, v)| v.xprv().is_some())
            .map(|(_, v)| v.xprv().unwrap())
            .collect()
    }

    pub fn get_secret_keys(&self) -> Vec<PrivateKey> {
        self.keys
            .iter()
            .filter(|(_, v)| v.as_secret_key().is_some())
            .map(|(_, v)| v.as_secret_key().unwrap())
            .collect()
    }

    pub fn get_hd_keypaths(
        &self,
        index: u32,
    ) -> Result<BTreeMap<PublicKey, (Fingerprint, DerivationPath)>, Error> {
        let mut answer = BTreeMap::new();

        for (_, key) in &self.keys {
            if let Some(fingerprint) = key.fingerprint(&self.ctx) {
                let derivation_path = key.full_path(index).unwrap();
                let pubkey = key.as_public_key(&self.ctx, Some(index))?;

                answer.insert(pubkey, (fingerprint, derivation_path));
            }
        }

        Ok(answer)
    }

    pub fn max_satisfaction_weight(&self) -> usize {
        let fake_pk = PublicKey::from_slice(&[
            2, 140, 40, 169, 123, 248, 41, 139, 192, 210, 61, 140, 116, 148, 82, 163, 46, 105, 75,
            101, 227, 10, 148, 114, 163, 149, 74, 179, 15, 229, 50, 76, 170,
        ])
        .unwrap();
        let translated: Descriptor<PublicKey> = self
            .internal
            .translate_pk(
                |_| -> Result<_, ()> { Ok(fake_pk.clone()) },
                |_| -> Result<_, ()> { Ok(Default::default()) },
            )
            .unwrap();

        translated.max_satisfaction_weight()
    }
}

impl TryFrom<&str> for ExtendedDescriptor {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let internal = StringDescriptor::from_str(value)?;
        ExtendedDescriptor::new(internal)
    }
}

impl TryFrom<StringDescriptor> for ExtendedDescriptor {
    type Error = Error;

    fn try_from(other: StringDescriptor) -> Result<Self, Self::Error> {
        ExtendedDescriptor::new(other)
    }
}

impl FromStr for ExtendedDescriptor {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl Into<String> for ExtendedDescriptor {
    fn into(self) -> String {
        format!("{}", self.internal)
    }
}
