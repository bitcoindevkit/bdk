use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;

use bitcoin::hashes::hash160;
use bitcoin::secp256k1::Secp256k1;
use bitcoin::util::bip32::{ChildNumber, DerivationPath, Fingerprint};
use bitcoin::util::psbt;
use bitcoin::{PublicKey, Script, TxOut};

use miniscript::descriptor::{DescriptorPublicKey, DescriptorXKey, InnerXKey};
pub use miniscript::{
    Descriptor, Legacy, Miniscript, MiniscriptKey, ScriptContext, Segwitv0, Terminal, ToPublicKey,
};

pub mod checksum;
pub mod error;
pub mod policy;

// use crate::wallet::utils::AddressType;
use crate::wallet::signer::SignersContainer;

pub use self::checksum::get_checksum;
use self::error::Error;
pub use self::policy::Policy;

pub type ExtendedDescriptor = Descriptor<DescriptorPublicKey>;
type HDKeyPaths = BTreeMap<PublicKey, (Fingerprint, DerivationPath)>;

pub trait ExtractPolicy {
    fn extract_policy(
        &self,
        signers: Arc<SignersContainer<DescriptorPublicKey>>,
    ) -> Result<Option<Policy>, Error>;
}

pub trait XKeyUtils {
    fn full_path(&self, append: &[ChildNumber]) -> DerivationPath;
    fn root_fingerprint(&self) -> Fingerprint;
}

impl<K: InnerXKey> XKeyUtils for DescriptorXKey<K> {
    fn full_path(&self, append: &[ChildNumber]) -> DerivationPath {
        let full_path = match &self.source {
            &Some((_, ref path)) => path
                .into_iter()
                .chain(self.derivation_path.into_iter())
                .cloned()
                .collect(),
            &None => self.derivation_path.clone(),
        };

        if self.is_wildcard {
            full_path
                .into_iter()
                .chain(append.into_iter())
                .cloned()
                .collect()
        } else {
            full_path
        }
    }

    fn root_fingerprint(&self) -> Fingerprint {
        match &self.source {
            &Some((fingerprint, _)) => fingerprint.clone(),
            &None => self.xkey.xkey_fingerprint(),
        }
    }
}

pub trait DescriptorMeta: Sized {
    fn is_witness(&self) -> bool;
    fn get_hd_keypaths(&self, index: u32) -> Result<HDKeyPaths, Error>;
    fn is_fixed(&self) -> bool;
    fn derive_from_hd_keypaths(&self, hd_keypaths: &HDKeyPaths) -> Option<Self>;
    fn derive_from_psbt_input(&self, psbt_input: &psbt::Input, utxo: Option<TxOut>)
        -> Option<Self>;
    // fn address_type(&self) -> Option<AddressType>;
}

pub trait DescriptorScripts {
    fn psbt_redeem_script(&self) -> Option<Script>;
    fn psbt_witness_script(&self) -> Option<Script>;
}

impl<T> DescriptorScripts for Descriptor<T>
where
    T: miniscript::MiniscriptKey + miniscript::ToPublicKey,
{
    fn psbt_redeem_script(&self) -> Option<Script> {
        match self {
            Descriptor::ShWpkh(_) => Some(self.witness_script()),
            Descriptor::ShWsh(ref script) => Some(script.encode().to_v0_p2wsh()),
            Descriptor::Sh(ref script) => Some(script.encode()),
            Descriptor::Bare(ref script) => Some(script.encode()),
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

impl DescriptorMeta for Descriptor<DescriptorPublicKey> {
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

    fn get_hd_keypaths(&self, index: u32) -> Result<HDKeyPaths, Error> {
        let mut answer = BTreeMap::new();

        let translatefpk = |key: &DescriptorPublicKey| -> Result<_, Error> {
            match key {
                DescriptorPublicKey::PubKey(_) => {}
                DescriptorPublicKey::XPub(xpub) => {
                    let derive_path = if xpub.is_wildcard {
                        xpub.derivation_path
                            .into_iter()
                            .chain([ChildNumber::from_normal_idx(index)?].iter())
                            .cloned()
                            .collect()
                    } else {
                        xpub.derivation_path.clone()
                    };
                    let derived_pubkey = xpub
                        .xkey
                        .derive_pub(&Secp256k1::verification_only(), &derive_path)?;

                    answer.insert(
                        derived_pubkey.public_key,
                        (
                            xpub.root_fingerprint(),
                            xpub.full_path(&[ChildNumber::from_normal_idx(index)?]),
                        ),
                    );
                }
            }

            Ok(DummyKey::default())
        };
        let translatefpkh = |_: &hash160::Hash| -> Result<_, Error> { Ok(DummyKey::default()) };

        self.translate_pk(translatefpk, translatefpkh)?;

        Ok(answer)
    }

    fn is_fixed(&self) -> bool {
        let mut found_wildcard = false;

        let translatefpk = |key: &DescriptorPublicKey| -> Result<_, Error> {
            match key {
                DescriptorPublicKey::PubKey(_) => {}
                DescriptorPublicKey::XPub(xpub) => {
                    if xpub.is_wildcard {
                        found_wildcard = true;
                    }
                }
            }

            Ok(DummyKey::default())
        };
        let translatefpkh = |_: &hash160::Hash| -> Result<_, Error> { Ok(DummyKey::default()) };

        self.translate_pk(translatefpk, translatefpkh).unwrap();

        !found_wildcard
    }

    fn derive_from_hd_keypaths(&self, hd_keypaths: &HDKeyPaths) -> Option<Self> {
        let index: HashMap<_, _> = hd_keypaths.values().cloned().collect();

        let mut derive_path = None::<DerivationPath>;
        let translatefpk = |key: &DescriptorPublicKey| -> Result<_, Error> {
            if derive_path.is_some() {
                // already found a matching path, we are done
                return Ok(DummyKey::default());
            }

            if let DescriptorPublicKey::XPub(xpub) = key {
                // Check if the key matches one entry in our `index`. If it does, `matches()` will
                // return the "prefix" that matched, so we remove that prefix from the full path
                // found in `index` and save it in `derive_path`
                let root_fingerprint = xpub.root_fingerprint();
                derive_path = index
                    .get_key_value(&root_fingerprint)
                    .and_then(|(fingerprint, path)| xpub.matches(*fingerprint, path))
                    .map(|prefix_path| prefix_path.into_iter().cloned().collect::<Vec<_>>())
                    .map(|prefix| {
                        index
                            .get(&xpub.root_fingerprint())
                            .unwrap()
                            .into_iter()
                            .skip(prefix.len())
                            .cloned()
                            .collect()
                    });
            }

            Ok(DummyKey::default())
        };
        let translatefpkh = |_: &hash160::Hash| -> Result<_, Error> { Ok(DummyKey::default()) };

        self.translate_pk(translatefpk, translatefpkh).unwrap();

        derive_path.map(|path| self.derive(path.as_ref()))
    }

    fn derive_from_psbt_input(
        &self,
        psbt_input: &psbt::Input,
        utxo: Option<TxOut>,
    ) -> Option<Self> {
        if let Some(derived) = self.derive_from_hd_keypaths(&psbt_input.hd_keypaths) {
            return Some(derived);
        } else if !self.is_fixed() {
            // If the descriptor is not fixed we can't brute-force the derivation address, so just
            // exit here
            return None;
        }

        match self {
            Descriptor::Pk(_)
            | Descriptor::Pkh(_)
            | Descriptor::Wpkh(_)
            | Descriptor::ShWpkh(_)
                if utxo.is_some()
                    && self.script_pubkey() == utxo.as_ref().unwrap().script_pubkey =>
            {
                Some(self.clone())
            }
            Descriptor::Bare(ms) | Descriptor::Sh(ms)
                if psbt_input.redeem_script.is_some()
                    && &ms.encode() == psbt_input.redeem_script.as_ref().unwrap() =>
            {
                Some(self.clone())
            }
            Descriptor::Wsh(ms) | Descriptor::ShWsh(ms)
                if psbt_input.witness_script.is_some()
                    && &ms.encode() == psbt_input.witness_script.as_ref().unwrap() =>
            {
                Some(self.clone())
            }
            _ => None,
        }
    }

    // fn address_type(&self) -> Option<AddressType> {
    //     match self {
    //         Descriptor::Pkh(_) => Some(AddressType::Pkh),
    //         Descriptor::Wpkh(_) => Some(AddressType::Wpkh),
    //         Descriptor::ShWpkh(_) => Some(AddressType::ShWpkh),
    //         Descriptor::Sh(_) => Some(AddressType::Sh),
    //         Descriptor::Wsh(_) => Some(AddressType::Wsh),
    //         Descriptor::ShWsh(_) => Some(AddressType::ShWsh),
    //         _ => None,
    //     }
    // }
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

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use bitcoin::consensus::encode::deserialize;
    use bitcoin::hashes::hex::FromHex;
    use bitcoin::util::psbt;

    use super::*;
    use crate::psbt::PSBTUtils;

    #[test]
    fn test_derive_from_psbt_input_wpkh() {
        let psbt: psbt::PartiallySignedTransaction = deserialize(&Vec::<u8>::from_hex("70736274ff010052010000000162307be8e431fbaff807cdf9cdc3fde44d740211bc8342c31ffd6ec11fe35bcc0100000000ffffffff01328601000000000016001493ce48570b55c42c2af816aeaba06cfee1224fae000000000001011fa08601000000000016001493ce48570b55c42c2af816aeaba06cfee1224fae010304010000000000").unwrap()).unwrap();

        let descriptor = Descriptor::<DescriptorPublicKey>::from_str(
            "wpkh(02b4632d08485ff1df2db55b9dafd23347d1c47a457072a1e87be26896549a8737)",
        )
        .unwrap();

        let result = descriptor.derive_from_psbt_input(&psbt.inputs[0], psbt.get_utxo_for(0));
        println!("{:?}", result);
    }
}
