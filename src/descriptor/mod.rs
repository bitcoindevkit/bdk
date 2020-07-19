use std::cell::RefCell;
use std::collections::BTreeMap;
use std::convert::{Into, TryFrom};
use std::fmt;
use std::str::FromStr;

use bitcoin::hashes::{hash160, Hash};
use bitcoin::secp256k1::{All, Secp256k1};
use bitcoin::util::bip32::{DerivationPath, ExtendedPrivKey, Fingerprint};
use bitcoin::util::psbt::PartiallySignedTransaction as PSBT;
use bitcoin::{PrivateKey, PublicKey, Script};

pub use miniscript::{
    Descriptor, Legacy, Miniscript, MiniscriptKey, ScriptContext, Segwitv0, Terminal,
};

use serde::{Deserialize, Serialize};

use crate::psbt::utils::PSBTUtils;

pub mod checksum;
pub mod error;
pub mod extended_key;
mod keys;
pub mod policy;

pub use self::checksum::get_checksum;
use self::error::Error;
pub use self::extended_key::{DerivationIndex, DescriptorExtendedKey};
pub use self::policy::Policy;

use self::keys::Key;

trait MiniscriptExtractPolicy {
    fn extract_policy(
        &self,
        lookup_map: &BTreeMap<String, Box<dyn Key>>,
    ) -> Result<Option<Policy>, Error>;
}

pub trait ExtractPolicy {
    fn extract_policy(&self) -> Result<Option<Policy>, Error>;
}

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
            Descriptor::ShWpkh(_) => Some(self.witness_script()),
            Descriptor::ShWsh(ref script) => Some(script.encode().to_v0_p2wsh()),
            Descriptor::Sh(ref script) => Some(script.encode()),
            _ => None,
        }
    }

    fn psbt_witness_script(&self) -> Option<Script> {
        match self {
            Descriptor::Wsh(ref script) => Some(script.encode()),
            Descriptor::ShWsh(ref script) => Some(script.encode()),
            _ => None,
        }
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

impl fmt::Display for ExtendedDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.internal)
    }
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

impl std::convert::AsRef<StringDescriptor> for ExtendedDescriptor {
    fn as_ref(&self) -> &StringDescriptor {
        &self.internal
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

    pub fn derive_with_miniscript_legacy(
        &self,
        miniscript: Miniscript<PublicKey, Legacy>,
    ) -> Result<DerivedDescriptor, Error> {
        let derived_desc = match self.internal {
            Descriptor::Bare(_) => Descriptor::Bare(miniscript),
            Descriptor::Sh(_) => Descriptor::Sh(miniscript),
            _ => return Err(Error::CantDeriveWithMiniscript),
        };

        // if !self.same_structure(&derived_desc) {
        //     Err(Error::CantDeriveWithMiniscript)
        // } else {
        Ok(derived_desc)
        // }
    }

    pub fn derive_with_miniscript_segwit_v0(
        &self,
        miniscript: Miniscript<PublicKey, Segwitv0>,
    ) -> Result<DerivedDescriptor, Error> {
        let derived_desc = match self.internal {
            Descriptor::Wsh(_) => Descriptor::Wsh(miniscript),
            Descriptor::ShWsh(_) => Descriptor::ShWsh(miniscript),
            _ => return Err(Error::CantDeriveWithMiniscript),
        };

        // if !self.same_structure(&derived_desc) {
        //     Err(Error::CantDeriveWithMiniscript)
        // } else {
        Ok(derived_desc)
        // }
    }

    pub fn derive_from_psbt_input(
        &self,
        psbt: &PSBT,
        input_index: usize,
    ) -> Result<DerivedDescriptor, Error> {
        let get_pk_from_partial_sigs = || {
            // here we need the public key.. since it's a single sig, there are only two
            // options: we can either find it in the `partial_sigs`, or we can't. if we
            // can't, it means that we can't even satisfy the input, so we can exit knowing
            // that we did our best to try to find it.
            psbt.inputs[input_index]
                .partial_sigs
                .keys()
                .nth(0)
                .ok_or(Error::MissingPublicKey)
        };

        if let Some(wit_script) = &psbt.inputs[input_index].witness_script {
            self.derive_with_miniscript_segwit_v0(Miniscript::parse(wit_script)?)
        } else if let Some(p2sh_script) = &psbt.inputs[input_index].redeem_script {
            if p2sh_script.is_v0_p2wpkh() {
                // wrapped p2wpkh
                get_pk_from_partial_sigs().map(|pk| Descriptor::ShWpkh(*pk))
            } else {
                self.derive_with_miniscript_legacy(Miniscript::parse(p2sh_script)?)
            }
        } else if let Some(utxo) = psbt.get_utxo_for(input_index) {
            if utxo.script_pubkey.is_p2pkh() {
                get_pk_from_partial_sigs().map(|pk| Descriptor::Pkh(*pk))
            } else if utxo.script_pubkey.is_p2pk() {
                get_pk_from_partial_sigs().map(|pk| Descriptor::Pk(*pk))
            } else if utxo.script_pubkey.is_v0_p2wpkh() {
                get_pk_from_partial_sigs().map(|pk| Descriptor::Wpkh(*pk))
            } else {
                // try as bare script
                self.derive_with_miniscript_legacy(Miniscript::parse(&utxo.script_pubkey)?)
            }
        } else {
            Err(Error::MissingDetails)
        }
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

    pub fn get_xprv(&self) -> impl IntoIterator<Item = ExtendedPrivKey> + '_ {
        self.keys
            .iter()
            .filter(|(_, v)| v.xprv().is_some())
            .map(|(_, v)| v.xprv().unwrap())
    }

    pub fn get_secret_keys(&self) -> impl IntoIterator<Item = PrivateKey> + '_ {
        self.keys
            .iter()
            .filter(|(_, v)| v.as_secret_key().is_some())
            .map(|(_, v)| v.as_secret_key().unwrap())
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

    pub fn is_fixed(&self) -> bool {
        self.keys.iter().all(|(_, key)| key.is_fixed())
    }

    pub fn same_structure<K: MiniscriptKey>(&self, other: &Descriptor<K>) -> bool {
        // Translate all the public keys to () and then check if the two descriptors are equal.
        // TODO: translate hashes to their default value before checking for ==

        let func_string = |_string: &String| -> Result<_, Error> { Ok(DummyKey::default()) };

        let func_generic_pk = |_data: &K| -> Result<_, Error> { Ok(DummyKey::default()) };
        let func_generic_pkh =
            |_data: &<K as MiniscriptKey>::Hash| -> Result<_, Error> { Ok(DummyKey::default()) };

        let translated_a = self.internal.translate_pk(func_string, func_string);
        let translated_b = other.translate_pk(func_generic_pk, func_generic_pkh);

        match (translated_a, translated_b) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }

    pub fn as_public_version(&self) -> Result<ExtendedDescriptor, Error> {
        let keys: RefCell<BTreeMap<String, Box<dyn Key>>> = RefCell::new(BTreeMap::new());

        let translatefpk = |string: &String| -> Result<_, Error> {
            let public = self.keys.get(string).unwrap().public(&self.ctx)?;

            let result = format!("{}", public);
            keys.borrow_mut().insert(string.clone(), public);

            Ok(result)
        };
        let translatefpkh = |string: &String| -> Result<_, Error> {
            let public = self.keys.get(string).unwrap().public(&self.ctx)?;

            let result = format!("{}", public);
            keys.borrow_mut().insert(string.clone(), public);

            Ok(result)
        };

        let internal = self.internal.translate_pk(translatefpk, translatefpkh)?;

        Ok(ExtendedDescriptor {
            internal,
            keys: keys.into_inner(),
            ctx: self.ctx.clone(),
        })
    }
}

impl ExtractPolicy for ExtendedDescriptor {
    fn extract_policy(&self) -> Result<Option<Policy>, Error> {
        self.internal.extract_policy(&self.keys)
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

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use bitcoin::hashes::hex::FromHex;
    use bitcoin::{Network, PublicKey};

    use crate::descriptor::*;

    macro_rules! hex_fingerprint {
        ($hex:expr) => {
            Fingerprint::from_hex($hex).unwrap()
        };
    }

    macro_rules! hex_pubkey {
        ($hex:expr) => {
            PublicKey::from_str($hex).unwrap()
        };
    }

    macro_rules! deriv_path {
        ($str:expr) => {
            DerivationPath::from_str($str).unwrap()
        };

        () => {
            DerivationPath::from(vec![])
        };
    }

    #[test]
    fn test_descriptor_parse_wif() {
        let string = "pkh(cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy)";
        let desc = ExtendedDescriptor::from_str(string).unwrap();
        assert!(desc.is_fixed());
        assert_eq!(
            desc.derive(0)
                .unwrap()
                .address(Network::Testnet)
                .unwrap()
                .to_string(),
            "mqwpxxvfv3QbM8PU8uBx2jaNt9btQqvQNx"
        );
        assert_eq!(
            desc.derive(42)
                .unwrap()
                .address(Network::Testnet)
                .unwrap()
                .to_string(),
            "mqwpxxvfv3QbM8PU8uBx2jaNt9btQqvQNx"
        );
        assert_eq!(
            desc.get_secret_keys().into_iter().collect::<Vec<_>>().len(),
            1
        );
    }

    #[test]
    fn test_descriptor_parse_pubkey() {
        let string = "pkh(039b6347398505f5ec93826dc61c19f47c66c0283ee9be980e29ce325a0f4679ef)";
        let desc = ExtendedDescriptor::from_str(string).unwrap();
        assert!(desc.is_fixed());
        assert_eq!(
            desc.derive(0)
                .unwrap()
                .address(Network::Testnet)
                .unwrap()
                .to_string(),
            "mqwpxxvfv3QbM8PU8uBx2jaNt9btQqvQNx"
        );
        assert_eq!(
            desc.derive(42)
                .unwrap()
                .address(Network::Testnet)
                .unwrap()
                .to_string(),
            "mqwpxxvfv3QbM8PU8uBx2jaNt9btQqvQNx"
        );
        assert_eq!(
            desc.get_secret_keys().into_iter().collect::<Vec<_>>().len(),
            0
        );
    }

    #[test]
    fn test_descriptor_parse_xpub() {
        let string = "pkh(tpubDEnoLuPdBep9bzw5LoGYpsxUQYheRQ9gcgrJhJEcdKFB9cWQRyYmkCyRoTqeD4tJYiVVgt6A3rN6rWn9RYhR9sBsGxji29LYWHuKKbdb1ev/*)";
        let desc = ExtendedDescriptor::from_str(string).unwrap();
        assert!(!desc.is_fixed());
        assert_eq!(
            desc.derive(0)
                .unwrap()
                .address(Network::Testnet)
                .unwrap()
                .to_string(),
            "mxbXpnVkwARGtYXk5yeGYf59bGWuPpdE4X"
        );
        assert_eq!(
            desc.derive(42)
                .unwrap()
                .address(Network::Testnet)
                .unwrap()
                .to_string(),
            "mhtuS1QaEV4HPcK4bWk4Wvpd64SUjiC5Zt"
        );
        assert_eq!(desc.get_xprv().into_iter().collect::<Vec<_>>().len(), 0);
    }

    #[test]
    #[should_panic(expected = "KeyParsingError")]
    fn test_descriptor_parse_fail() {
        let string = "pkh(this_is_not_a_valid_key)";
        ExtendedDescriptor::from_str(string).unwrap();
    }

    #[test]
    fn test_descriptor_hd_keypaths() {
        let string = "pkh(tpubDEnoLuPdBep9bzw5LoGYpsxUQYheRQ9gcgrJhJEcdKFB9cWQRyYmkCyRoTqeD4tJYiVVgt6A3rN6rWn9RYhR9sBsGxji29LYWHuKKbdb1ev/*)";
        let desc = ExtendedDescriptor::from_str(string).unwrap();
        let keypaths = desc.get_hd_keypaths(0).unwrap();
        assert!(keypaths.contains_key(&hex_pubkey!(
            "025d5fc65ebb8d44a5274b53bac21ff8307fec2334a32df05553459f8b1f7fe1b6"
        )));
        assert_eq!(
            keypaths.get(&hex_pubkey!(
                "025d5fc65ebb8d44a5274b53bac21ff8307fec2334a32df05553459f8b1f7fe1b6"
            )),
            Some(&(hex_fingerprint!("31a507b8"), deriv_path!("m/0")))
        )
    }
}
