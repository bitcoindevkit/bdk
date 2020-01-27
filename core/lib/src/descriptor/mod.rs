use std::cell::{Ref, RefCell};
use std::collections::BTreeMap;
use std::convert::{Into, TryFrom};
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

use bitcoin::blockdata::script::Script;
use bitcoin::hashes::{hash160, Hash};
use bitcoin::secp256k1::{All, Secp256k1};
use bitcoin::util::bip32::{DerivationPath, ExtendedPrivKey, Fingerprint};
use bitcoin::PublicKey;

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

#[serde(try_from = "&str", into = "String")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtendedDescriptor {
    internal: StringDescriptor,
    #[serde(skip)]
    cache: RefCell<BTreeMap<String, DescriptorExtendedKey>>,
    #[serde(skip)]
    ctx: Secp256k1<All>,
}

impl ExtendedDescriptor {
    fn parse_xpub(
        &self,
        xpub: &str,
    ) -> Result<impl Deref<Target = DescriptorExtendedKey> + '_, Error> {
        // TODO: this sucks, there's got to be a better way
        {
            let mut cache = self.cache.borrow_mut();

            if let None = cache.get(xpub) {
                let parsed = DescriptorExtendedKey::from_str(xpub)?;
                cache.insert(xpub.to_string(), parsed);
            }
        }

        Ok(Ref::map(self.cache.borrow(), |map| map.get(xpub).unwrap()))
    }

    fn get_pubkey(&self, string: &str, index: u32) -> Result<PublicKey, Error> {
        // `string` could be either an xpub/xprv or a raw pubkey. try both, fail if none of them
        // worked out.

        // TODO: parse WIF keys
        match self.parse_xpub(string) {
            Ok(xpub) => Ok(xpub.derive(&self.ctx, index)?),
            Err(Error::Base58(_)) => Ok(PublicKey::from_str(string)?),
            Err(e) => Err(e),
        }
    }

    pub fn derive(&self, index: u32) -> Result<DerivedDescriptor, Error> {
        let translatefpk =
            |xpub: &String| -> Result<_, Error> { Ok(self.get_pubkey(xpub, index)?) };
        let translatefpkh =
            |xpub: &String| Ok(hash160::Hash::hash(&translatefpk(xpub)?.to_bytes()));

        Ok(self.internal.translate_pk(translatefpk, translatefpkh)?)
    }

    pub fn get_xprv(&self) -> Vec<ExtendedPrivKey> {
        // TODO: parse WIF keys

        let mut answer = Vec::new();

        let translatefpk = |string: &String| -> Result<_, ()> {
            let extended_key = match self.parse_xpub(string) {
                Err(_) => return Ok(DummyKey::default()),
                Ok(ek) => ek,
            };

            if let Some(xprv) = extended_key.secret {
                answer.push(xprv.clone());
            }

            Ok(DummyKey::default())
        };
        let translatefpkh = |_: &String| -> Result<_, ()> { Ok(DummyKey::default()) };

        // should never fail. if we can't parse an xpub, we just skip it
        self.internal
            .translate_pk(translatefpk, translatefpkh)
            .unwrap();

        answer
    }

    pub fn get_hd_keypaths(
        &self,
        index: u32,
    ) -> Result<BTreeMap<PublicKey, (Fingerprint, DerivationPath)>, Error> {
        let mut answer = BTreeMap::new();

        let translatefpk = |string: &String| -> Result<_, Error> {
            let extended_key = match self.parse_xpub(string) {
                Err(_) => return Ok(DummyKey::default()),
                Ok(ek) => ek,
            };

            let fingerprint = extended_key.root_xpub(&self.ctx).fingerprint();

            let derivation_path = extended_key.full_path(index);
            let pubkey = extended_key.derive_xpub(&self.ctx, index)?.public_key;

            answer.insert(pubkey, (fingerprint, derivation_path));

            Ok(DummyKey::default())
        };
        let translatefpkh = |_: &String| -> Result<_, Error> { Ok(DummyKey::default()) };

        self.internal.translate_pk(translatefpk, translatefpkh)?;

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
    type Error = miniscript::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Ok(ExtendedDescriptor {
            internal: StringDescriptor::from_str(value)?,
            cache: RefCell::new(BTreeMap::new()),
            ctx: Secp256k1::gen_new(),
        })
    }
}

impl FromStr for ExtendedDescriptor {
    type Err = miniscript::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl Into<String> for ExtendedDescriptor {
    fn into(self) -> String {
        format!("{}", self.internal)
    }
}
