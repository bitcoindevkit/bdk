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

pub use self::checksum::get_checksum;
use self::error::Error;
pub use self::policy::Policy;
use crate::wallet::signer::SignersContainer;

pub type ExtendedDescriptor = Descriptor<DescriptorPublicKey>;
pub type HDKeyPaths = BTreeMap<PublicKey, (Fingerprint, DerivationPath)>;

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
    fn test_derive_from_psbt_input_wpkh_wif() {
        let descriptor = Descriptor::<DescriptorPublicKey>::from_str(
            "wpkh(02b4632d08485ff1df2db55b9dafd23347d1c47a457072a1e87be26896549a8737)",
        )
        .unwrap();
        let psbt: psbt::PartiallySignedTransaction = deserialize(
            &Vec::<u8>::from_hex(
                "70736274ff010052010000000162307be8e431fbaff807cdf9cdc3fde44d7402\
                 11bc8342c31ffd6ec11fe35bcc0100000000ffffffff01328601000000000016\
                 001493ce48570b55c42c2af816aeaba06cfee1224fae000000000001011fa086\
                 01000000000016001493ce48570b55c42c2af816aeaba06cfee1224fae010304\
                 010000000000",
            )
            .unwrap(),
        )
        .unwrap();

        assert!(descriptor
            .derive_from_psbt_input(&psbt.inputs[0], psbt.get_utxo_for(0))
            .is_some());
    }

    #[test]
    fn test_derive_from_psbt_input_pkh_tpub() {
        let descriptor = Descriptor::<DescriptorPublicKey>::from_str(
            "pkh([0f056943/44h/0h/0h]tpubDDpWvmUrPZrhSPmUzCMBHffvC3HyMAPnWDSAQNBTnj1iZeJa7BZQEttFiP4DS4GCcXQHezdXhn86Hj6LHX5EDstXPWrMaSneRWM8yUf6NFd/10/*)",
        )
        .unwrap();
        let psbt: psbt::PartiallySignedTransaction = deserialize(
            &Vec::<u8>::from_hex(
                "70736274ff010053010000000145843b86be54a3cd8c9e38444e1162676c00df\
                 e7964122a70df491ea12fd67090100000000ffffffff01c19598000000000017\
                 a91432bb94283282f72b2e034709e348c44d5a4db0ef8700000000000100f902\
                 0000000001010167e99c0eb67640f3a1b6805f2d8be8238c947f8aaf49eb0a9c\
                 bee6a42c984200000000171600142b29a22019cca05b9c2b2d283a4c4489e1cf\
                 9f8ffeffffff02a01dced06100000017a914e2abf033cadbd74f0f4c74946201\
                 decd20d5c43c8780969800000000001976a9148b0fce5fb1264e599a65387313\
                 3c95478b902eb288ac02473044022015d9211576163fa5b001e84dfa3d44efd9\
                 86b8f3a0d3d2174369288b2b750906022048dacc0e5d73ae42512fd2b97e2071\
                 a8d0bce443b390b1fe0b8128fe70ec919e01210232dad1c5a67dcb0116d407e2\
                 52584228ab7ec00e8b9779d0c3ffe8114fc1a7d2c80600000103040100000022\
                 0603433b83583f8c4879b329dd08bbc7da935e4cc02f637ff746e05f0466ffb2\
                 a6a2180f0569432c00008000000080000000800a000000000000000000",
            )
            .unwrap(),
        )
        .unwrap();

        assert!(descriptor
            .derive_from_psbt_input(&psbt.inputs[0], psbt.get_utxo_for(0))
            .is_some());
    }

    #[test]
    fn test_derive_from_psbt_input_wsh() {
        let descriptor = Descriptor::<DescriptorPublicKey>::from_str(
            "wsh(and_v(v:pk(03b6633fef2397a0a9de9d7b6f23aef8368a6e362b0581f0f0af70d5ecfd254b14),older(6)))",
        )
        .unwrap();
        let psbt: psbt::PartiallySignedTransaction = deserialize(
            &Vec::<u8>::from_hex(
                "70736274ff01005302000000011c8116eea34408ab6529223c9a176606742207\
                 67a1ff1d46a6e3c4a88243ea6e01000000000600000001109698000000000017\
                 a914ad105f61102e0d01d7af40d06d6a5c3ae2f7fde387000000000001012b80\
                 969800000000002200203ca72f106a72234754890ca7640c43f65d2174e44d33\
                 336030f9059345091044010304010000000105252103b6633fef2397a0a9de9d\
                 7b6f23aef8368a6e362b0581f0f0af70d5ecfd254b14ad56b20000",
            )
            .unwrap(),
        )
        .unwrap();

        assert!(descriptor
            .derive_from_psbt_input(&psbt.inputs[0], psbt.get_utxo_for(0))
            .is_some());
    }

    #[test]
    fn test_derive_from_psbt_input_sh() {
        let descriptor = Descriptor::<DescriptorPublicKey>::from_str(
            "sh(and_v(v:pk(021403881a5587297818fcaf17d239cefca22fce84a45b3b1d23e836c4af671dbb),after(630000)))",
        )
        .unwrap();
        let psbt: psbt::PartiallySignedTransaction = deserialize(
            &Vec::<u8>::from_hex(
                "70736274ff0100530100000001bc8c13df445dfadcc42afa6dc841f85d22b01d\
                 a6270ebf981740f4b7b1d800390000000000feffffff01ba9598000000000017\
                 a91457b148ba4d3e5fa8608a8657875124e3d1c9390887f09c0900000100e002\
                 0000000001016ba1bbe05cc93574a0d611ec7d93ad0ab6685b28d0cd80e8a82d\
                 debb326643c90100000000feffffff02809698000000000017a914d9a6e8c455\
                 8e16c8253afe53ce37ad61cf4c38c487403504cf6100000017a9144044fb6e0b\
                 757dfc1b34886b6a95aef4d3db137e870247304402202a9b72d939bcde8ba2a1\
                 e0980597e47af4f5c152a78499143c3d0a78ac2286a602207a45b1df9e93b8c9\
                 6f09f5c025fe3e413ca4b905fe65ee55d32a3276439a9b8f012102dc1fcc2636\
                 4da1aa718f03d8d9bd6f2ff410ed2cf1245a168aa3bcc995ac18e0a806000001\
                 03040100000001042821021403881a5587297818fcaf17d239cefca22fce84a4\
                 5b3b1d23e836c4af671dbbad03f09c09b10000",
            )
            .unwrap(),
        )
        .unwrap();

        assert!(descriptor
            .derive_from_psbt_input(&psbt.inputs[0], psbt.get_utxo_for(0))
            .is_some());
    }
}
