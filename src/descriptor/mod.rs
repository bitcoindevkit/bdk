// Magical Bitcoin Library
// Written in 2020 by
//     Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020 Magical Bitcoin
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Descriptors
//!
//! This module contains generic utilities to work with descriptors, plus some re-exported types
//! from [`miniscript`].

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;

use bitcoin::secp256k1::Secp256k1;
use bitcoin::util::bip32::{ChildNumber, DerivationPath, Fingerprint};
use bitcoin::util::psbt;
use bitcoin::{Network, PublicKey, Script, TxOut};

use miniscript::descriptor::{DescriptorPublicKey, DescriptorXKey, InnerXKey};
pub use miniscript::{
    descriptor::KeyMap, Descriptor, Legacy, Miniscript, MiniscriptKey, ScriptContext, Segwitv0,
    Terminal, ToPublicKey,
};

pub mod checksum;
mod dsl;
pub mod error;
pub mod policy;
pub mod template;

pub use self::checksum::get_checksum;
use self::error::Error;
pub use self::policy::Policy;
use crate::keys::{KeyError, ToDescriptorKey, ValidNetworks};
use crate::wallet::signer::SignersContainer;
use crate::wallet::utils::{descriptor_to_pk_ctx, SecpCtx};

/// Alias for a [`Descriptor`] that can contain extended keys using [`DescriptorPublicKey`]
pub type ExtendedDescriptor = Descriptor<DescriptorPublicKey>;

/// Alias for the type of maps that represent derivation paths in a [`psbt::Input`] or
/// [`psbt::Output`]
///
/// [`psbt::Input`]: bitcoin::util::psbt::Input
/// [`psbt::Output`]: bitcoin::util::psbt::Output
pub type HDKeyPaths = BTreeMap<PublicKey, (Fingerprint, DerivationPath)>;

/// Trait for types which can be converted into an [`ExtendedDescriptor`] and a [`KeyMap`] usable by a wallet in a specific [`Network`]
pub trait ToWalletDescriptor {
    fn to_wallet_descriptor(
        self,
        network: Network,
    ) -> Result<(ExtendedDescriptor, KeyMap), KeyError>;
}

impl ToWalletDescriptor for &str {
    fn to_wallet_descriptor(
        self,
        network: Network,
    ) -> Result<(ExtendedDescriptor, KeyMap), KeyError> {
        let descriptor = if self.contains('#') {
            let parts: Vec<&str> = self.splitn(2, '#').collect();
            if !get_checksum(parts[0])
                .ok()
                .map(|computed| computed == parts[1])
                .unwrap_or(false)
            {
                return Err(KeyError::InvalidChecksum);
            }

            parts[0]
        } else {
            self
        };

        ExtendedDescriptor::parse_descriptor(descriptor)?.to_wallet_descriptor(network)
    }
}

impl ToWalletDescriptor for &String {
    fn to_wallet_descriptor(
        self,
        network: Network,
    ) -> Result<(ExtendedDescriptor, KeyMap), KeyError> {
        self.as_str().to_wallet_descriptor(network)
    }
}

impl ToWalletDescriptor for ExtendedDescriptor {
    fn to_wallet_descriptor(
        self,
        network: Network,
    ) -> Result<(ExtendedDescriptor, KeyMap), KeyError> {
        (self, KeyMap::default()).to_wallet_descriptor(network)
    }
}

impl ToWalletDescriptor for (ExtendedDescriptor, KeyMap) {
    fn to_wallet_descriptor(
        self,
        network: Network,
    ) -> Result<(ExtendedDescriptor, KeyMap), KeyError> {
        use crate::keys::DescriptorKey;

        let secp = Secp256k1::new();

        let check_key = |pk: &DescriptorPublicKey| {
            let (pk, _, networks) = if self.0.is_witness() {
                let desciptor_key: DescriptorKey<miniscript::Segwitv0> =
                    pk.clone().to_descriptor_key()?;
                desciptor_key.extract(&secp)?
            } else {
                let desciptor_key: DescriptorKey<miniscript::Legacy> =
                    pk.clone().to_descriptor_key()?;
                desciptor_key.extract(&secp)?
            };

            if networks.contains(&network) {
                Ok(pk)
            } else {
                Err(KeyError::InvalidNetwork)
            }
        };

        // check the network for the keys
        let translated = self.0.translate_pk(check_key, check_key)?;

        Ok((translated, self.1))
    }
}

impl ToWalletDescriptor for (ExtendedDescriptor, KeyMap, ValidNetworks) {
    fn to_wallet_descriptor(
        self,
        network: Network,
    ) -> Result<(ExtendedDescriptor, KeyMap), KeyError> {
        let valid_networks = &self.2;

        let fix_key = |pk: &DescriptorPublicKey| {
            if valid_networks.contains(&network) {
                // workaround for xpubs generated by other key types, like bip39: since when the
                // conversion is made one network has to be chosen, what we generally choose
                // "mainnet", but then override the set of valid networks to specify that all of
                // them are valid. here we reset the network to make sure the wallet struct gets a
                // descriptor with the right network everywhere.
                let pk = match pk {
                    DescriptorPublicKey::XPub(ref xpub) => {
                        let mut xpub = xpub.clone();
                        xpub.xkey.network = network;

                        DescriptorPublicKey::XPub(xpub)
                    }
                    other => other.clone(),
                };

                Ok(pk)
            } else {
                Err(KeyError::InvalidNetwork)
            }
        };

        // fixup the network for keys that need it
        let translated = self.0.translate_pk(fix_key, fix_key)?;

        Ok((translated, self.1))
    }
}

/// Trait implemented on [`Descriptor`]s to add a method to extract the spending [`policy`]
pub trait ExtractPolicy {
    fn extract_policy(
        &self,
        signers: Arc<SignersContainer>,
        secp: &SecpCtx,
    ) -> Result<Option<Policy>, Error>;
}

pub(crate) trait XKeyUtils {
    fn full_path(&self, append: &[ChildNumber]) -> DerivationPath;
    fn root_fingerprint(&self, secp: &SecpCtx) -> Fingerprint;
}

impl<K: InnerXKey> XKeyUtils for DescriptorXKey<K> {
    fn full_path(&self, append: &[ChildNumber]) -> DerivationPath {
        let full_path = match self.origin {
            Some((_, ref path)) => path
                .into_iter()
                .chain(self.derivation_path.into_iter())
                .cloned()
                .collect(),
            None => self.derivation_path.clone(),
        };

        if self.is_wildcard {
            full_path
                .into_iter()
                .chain(append.iter())
                .cloned()
                .collect()
        } else {
            full_path
        }
    }

    fn root_fingerprint(&self, secp: &SecpCtx) -> Fingerprint {
        match self.origin {
            Some((fingerprint, _)) => fingerprint,
            None => self.xkey.xkey_fingerprint(secp),
        }
    }
}

pub(crate) trait DescriptorMeta: Sized {
    fn is_witness(&self) -> bool;
    fn get_hd_keypaths(&self, index: u32, secp: &SecpCtx) -> Result<HDKeyPaths, Error>;
    fn is_fixed(&self) -> bool;
    fn derive_from_hd_keypaths(&self, hd_keypaths: &HDKeyPaths, secp: &SecpCtx) -> Option<Self>;
    fn derive_from_psbt_input(
        &self,
        psbt_input: &psbt::Input,
        utxo: Option<TxOut>,
        secp: &SecpCtx,
    ) -> Option<Self>;
}

pub(crate) trait DescriptorScripts {
    fn psbt_redeem_script(&self, secp: &SecpCtx) -> Option<Script>;
    fn psbt_witness_script(&self, secp: &SecpCtx) -> Option<Script>;
}

impl DescriptorScripts for Descriptor<DescriptorPublicKey> {
    fn psbt_redeem_script(&self, secp: &SecpCtx) -> Option<Script> {
        let deriv_ctx = descriptor_to_pk_ctx(secp);

        match self {
            Descriptor::ShWpkh(_) => Some(self.witness_script(deriv_ctx)),
            Descriptor::ShWsh(ref script) => Some(script.encode(deriv_ctx).to_v0_p2wsh()),
            Descriptor::Sh(ref script) => Some(script.encode(deriv_ctx)),
            Descriptor::Bare(ref script) => Some(script.encode(deriv_ctx)),
            Descriptor::ShSortedMulti(ref keys) => Some(keys.encode(deriv_ctx)),
            _ => None,
        }
    }

    fn psbt_witness_script(&self, secp: &SecpCtx) -> Option<Script> {
        let deriv_ctx = descriptor_to_pk_ctx(secp);

        match self {
            Descriptor::Wsh(ref script) => Some(script.encode(deriv_ctx)),
            Descriptor::ShWsh(ref script) => Some(script.encode(deriv_ctx)),
            Descriptor::WshSortedMulti(ref keys) | Descriptor::ShWshSortedMulti(ref keys) => {
                Some(keys.encode(deriv_ctx))
            }
            _ => None,
        }
    }
}

impl DescriptorMeta for Descriptor<DescriptorPublicKey> {
    fn is_witness(&self) -> bool {
        match self {
            Descriptor::Bare(_)
            | Descriptor::Pk(_)
            | Descriptor::Pkh(_)
            | Descriptor::Sh(_)
            | Descriptor::ShSortedMulti(_) => false,
            Descriptor::Wpkh(_)
            | Descriptor::ShWpkh(_)
            | Descriptor::Wsh(_)
            | Descriptor::ShWsh(_)
            | Descriptor::ShWshSortedMulti(_)
            | Descriptor::WshSortedMulti(_) => true,
        }
    }

    fn get_hd_keypaths(&self, index: u32, secp: &SecpCtx) -> Result<HDKeyPaths, Error> {
        let translate_key = |key: &DescriptorPublicKey,
                             index: u32,
                             paths: &mut HDKeyPaths|
         -> Result<DummyKey, Error> {
            match key {
                DescriptorPublicKey::SinglePub(_) => {}
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

                    paths.insert(
                        derived_pubkey.public_key,
                        (
                            xpub.root_fingerprint(secp),
                            xpub.full_path(&[ChildNumber::from_normal_idx(index)?]),
                        ),
                    );
                }
            }

            Ok(DummyKey::default())
        };

        let mut answer_pk = BTreeMap::new();
        let mut answer_pkh = BTreeMap::new();

        self.translate_pk(
            |pk| translate_key(pk, index, &mut answer_pk),
            |pkh| translate_key(pkh, index, &mut answer_pkh),
        )?;

        answer_pk.append(&mut answer_pkh);

        Ok(answer_pk)
    }

    fn is_fixed(&self) -> bool {
        fn check_key(key: &DescriptorPublicKey, flag: &mut bool) -> Result<DummyKey, Error> {
            match key {
                DescriptorPublicKey::SinglePub(_) => {}
                DescriptorPublicKey::XPub(xpub) => {
                    if xpub.is_wildcard {
                        *flag = true;
                    }
                }
            }

            Ok(DummyKey::default())
        }

        let mut found_wildcard_pk = false;
        let mut found_wildcard_pkh = false;

        self.translate_pk(
            |pk| check_key(pk, &mut found_wildcard_pk),
            |pkh| check_key(pkh, &mut found_wildcard_pkh),
        )
        .unwrap();

        !found_wildcard_pk && !found_wildcard_pkh
    }

    fn derive_from_hd_keypaths(&self, hd_keypaths: &HDKeyPaths, secp: &SecpCtx) -> Option<Self> {
        let try_key = |key: &DescriptorPublicKey,
                       index: &HashMap<Fingerprint, DerivationPath>,
                       found_path: &mut Option<ChildNumber>|
         -> Result<DummyKey, Error> {
            if found_path.is_some() {
                // already found a matching path, we are done
                return Ok(DummyKey::default());
            }

            if let DescriptorPublicKey::XPub(xpub) = key {
                // Check if the key matches one entry in our `index`. If it does, `matches()` will
                // return the "prefix" that matched, so we remove that prefix from the full path
                // found in `index` and save it in `derive_path`. We expect this to be a derivation
                // path of length 1 if the key `is_wildcard` and an empty path otherwise.
                let root_fingerprint = xpub.root_fingerprint(secp);
                let derivation_path: Option<Vec<ChildNumber>> = index
                    .get_key_value(&root_fingerprint)
                    .and_then(|(fingerprint, path)| {
                        xpub.matches(&(*fingerprint, path.clone()), secp)
                    })
                    .map(|prefix| {
                        index
                            .get(&xpub.root_fingerprint(secp))
                            .unwrap()
                            .into_iter()
                            .skip(prefix.into_iter().count())
                            .cloned()
                            .collect()
                    });

                match derivation_path {
                    Some(path) if xpub.is_wildcard && path.len() == 1 => {
                        *found_path = Some(path[0])
                    }
                    Some(path) if !xpub.is_wildcard && path.is_empty() => {
                        *found_path = Some(ChildNumber::Normal { index: 0 })
                    }
                    Some(_) => return Err(Error::InvalidHDKeyPath),
                    _ => {}
                }
            }

            Ok(DummyKey::default())
        };

        let index: HashMap<_, _> = hd_keypaths.values().cloned().collect();

        let mut found_path_pk = None;
        let mut found_path_pkh = None;

        if self
            .translate_pk(
                |pk| try_key(pk, &index, &mut found_path_pk),
                |pkh| try_key(pkh, &index, &mut found_path_pkh),
            )
            .is_err()
        {
            return None;
        }

        // if we have found a path for both `found_path_pk` and `found_path_pkh` but they are
        // different we consider this an error and return None. we only return a path either if
        // they are equal or if only one of them is Some(_)
        let merged_path = match (found_path_pk, found_path_pkh) {
            (Some(a), Some(b)) if a != b => return None,
            (a, b) => a.or(b),
        };

        merged_path.map(|path| self.derive(path))
    }

    fn derive_from_psbt_input(
        &self,
        psbt_input: &psbt::Input,
        utxo: Option<TxOut>,
        secp: &SecpCtx,
    ) -> Option<Self> {
        if let Some(derived) = self.derive_from_hd_keypaths(&psbt_input.hd_keypaths, secp) {
            return Some(derived);
        } else if !self.is_fixed() {
            // If the descriptor is not fixed we can't brute-force the derivation address, so just
            // exit here
            return None;
        }

        let deriv_ctx = descriptor_to_pk_ctx(secp);
        match self {
            Descriptor::Pk(_)
            | Descriptor::Pkh(_)
            | Descriptor::Wpkh(_)
            | Descriptor::ShWpkh(_)
                if utxo.is_some()
                    && self.script_pubkey(deriv_ctx) == utxo.as_ref().unwrap().script_pubkey =>
            {
                Some(self.clone())
            }
            Descriptor::Bare(ms) | Descriptor::Sh(ms)
                if psbt_input.redeem_script.is_some()
                    && &ms.encode(deriv_ctx) == psbt_input.redeem_script.as_ref().unwrap() =>
            {
                Some(self.clone())
            }
            Descriptor::Wsh(ms) | Descriptor::ShWsh(ms)
                if psbt_input.witness_script.is_some()
                    && &ms.encode(deriv_ctx) == psbt_input.witness_script.as_ref().unwrap() =>
            {
                Some(self.clone())
            }
            Descriptor::ShSortedMulti(keys)
                if psbt_input.redeem_script.is_some()
                    && &keys.encode(deriv_ctx) == psbt_input.redeem_script.as_ref().unwrap() =>
            {
                Some(self.clone())
            }
            Descriptor::WshSortedMulti(keys) | Descriptor::ShWshSortedMulti(keys)
                if psbt_input.witness_script.is_some()
                    && &keys.encode(deriv_ctx) == psbt_input.witness_script.as_ref().unwrap() =>
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
    use bitcoin::secp256k1::Secp256k1;
    use bitcoin::util::{bip32, psbt};

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
            .derive_from_psbt_input(&psbt.inputs[0], psbt.get_utxo_for(0), &Secp256k1::new())
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
            .derive_from_psbt_input(&psbt.inputs[0], psbt.get_utxo_for(0), &Secp256k1::new())
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
            .derive_from_psbt_input(&psbt.inputs[0], psbt.get_utxo_for(0), &Secp256k1::new())
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
            .derive_from_psbt_input(&psbt.inputs[0], psbt.get_utxo_for(0), &Secp256k1::new())
            .is_some());
    }

    #[test]
    fn test_to_wallet_descriptor_fixup_networks() {
        use crate::keys::{any_network, ToDescriptorKey};

        let xpub = bip32::ExtendedPubKey::from_str("xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL").unwrap();
        let path = bip32::DerivationPath::from_str("m/0").unwrap();

        // here `to_descriptor_key` will set the valid networks for the key to only mainnet, since
        // we are using an "xpub"
        let key = (xpub, path).to_descriptor_key().unwrap();
        // override it with any. this happens in some key conversions, like bip39
        let key = key.override_valid_networks(any_network());

        // make a descriptor out of it
        let desc = crate::descriptor!(wpkh(key)).unwrap();
        // this should conver the key that supports "any_network" to the right network (testnet)
        let (wallet_desc, _) = desc.to_wallet_descriptor(Network::Testnet).unwrap();

        assert_eq!(wallet_desc.to_string(), "wpkh(tpubDEnoLuPdBep9bzw5LoGYpsxUQYheRQ9gcgrJhJEcdKFB9cWQRyYmkCyRoTqeD4tJYiVVgt6A3rN6rWn9RYhR9sBsGxji29LYWHuKKbdb1ev/0/*)");
    }

    // test ToWalletDescriptor trait from &str with and without checksum appended
    #[test]
    fn test_descriptor_from_str_with_checksum() {
        let desc = "wpkh(tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcLNfjdi5qUvw3VDfgYiH5mNsj5izuiu2N/1/2/*)#tqz0nc62"
            .to_wallet_descriptor(Network::Testnet);
        assert!(desc.is_ok());

        let desc = "wpkh(tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcLNfjdi5qUvw3VDfgYiH5mNsj5izuiu2N/1/2/*)"
            .to_wallet_descriptor(Network::Testnet);
        assert!(desc.is_ok());

        let desc = "wpkh(tpubD6NzVbkrYhZ4XHndKkuB8FifXm8r5FQHwrN6oZuWCz13qb93rtgKvD4PQsqC4HP4yhV3tA2fqr2RbY5mNXfM7RxXUoeABoDtsFUq2zJq6YK/1/2/*)#67ju93jw"
            .to_wallet_descriptor(Network::Testnet);
        assert!(desc.is_ok());

        let desc = "wpkh(tpubD6NzVbkrYhZ4XHndKkuB8FifXm8r5FQHwrN6oZuWCz13qb93rtgKvD4PQsqC4HP4yhV3tA2fqr2RbY5mNXfM7RxXUoeABoDtsFUq2zJq6YK/1/2/*)"
            .to_wallet_descriptor(Network::Testnet);
        assert!(desc.is_ok());

        let desc = "wpkh(tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcLNfjdi5qUvw3VDfgYiH5mNsj5izuiu2N/1/2/*)#67ju93jw"
            .to_wallet_descriptor(Network::Testnet);
        assert!(matches!(desc.err(), Some(KeyError::InvalidChecksum)));

        let desc = "wpkh(tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcLNfjdi5qUvw3VDfgYiH5mNsj5izuiu2N/1/2/*)#67ju93jw"
            .to_wallet_descriptor(Network::Testnet);
        assert!(matches!(desc.err(), Some(KeyError::InvalidChecksum)));
    }

    // test ToWalletDescriptor trait from &str with keys from right and wrong network
    #[test]
    fn test_descriptor_from_str_with_keys_network() {
        let desc = "wpkh(tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcLNfjdi5qUvw3VDfgYiH5mNsj5izuiu2N/1/2/*)"
            .to_wallet_descriptor(Network::Testnet);
        assert!(desc.is_ok());

        let desc = "wpkh(tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcLNfjdi5qUvw3VDfgYiH5mNsj5izuiu2N/1/2/*)"
            .to_wallet_descriptor(Network::Regtest);
        assert!(desc.is_ok());

        let desc = "wpkh(tpubD6NzVbkrYhZ4XHndKkuB8FifXm8r5FQHwrN6oZuWCz13qb93rtgKvD4PQsqC4HP4yhV3tA2fqr2RbY5mNXfM7RxXUoeABoDtsFUq2zJq6YK/1/2/*)"
            .to_wallet_descriptor(Network::Testnet);
        assert!(desc.is_ok());

        let desc = "wpkh(tpubD6NzVbkrYhZ4XHndKkuB8FifXm8r5FQHwrN6oZuWCz13qb93rtgKvD4PQsqC4HP4yhV3tA2fqr2RbY5mNXfM7RxXUoeABoDtsFUq2zJq6YK/1/2/*)"
            .to_wallet_descriptor(Network::Regtest);
        assert!(desc.is_ok());

        let desc = "sh(wpkh(02864bb4ad00cefa806098a69e192bbda937494e69eb452b87bb3f20f6283baedb))"
            .to_wallet_descriptor(Network::Testnet);
        assert!(desc.is_ok());

        let desc = "sh(wpkh(02864bb4ad00cefa806098a69e192bbda937494e69eb452b87bb3f20f6283baedb))"
            .to_wallet_descriptor(Network::Bitcoin);
        assert!(desc.is_ok());

        let desc = "wpkh(tprv8ZgxMBicQKsPdpkqS7Eair4YxjcuuvDPNYmKX3sCniCf16tHEVrjjiSXEkFRnUH77yXc6ZcwHHcLNfjdi5qUvw3VDfgYiH5mNsj5izuiu2N/1/2/*)"
            .to_wallet_descriptor(Network::Bitcoin);
        assert!(matches!(desc.err(), Some(KeyError::InvalidNetwork)));

        let desc = "wpkh(tpubD6NzVbkrYhZ4XHndKkuB8FifXm8r5FQHwrN6oZuWCz13qb93rtgKvD4PQsqC4HP4yhV3tA2fqr2RbY5mNXfM7RxXUoeABoDtsFUq2zJq6YK/1/2/*)"
            .to_wallet_descriptor(Network::Bitcoin);
        assert!(matches!(desc.err(), Some(KeyError::InvalidNetwork)));
    }

    // test ToWalletDescriptor trait from the output of the descriptor!() macro
    #[test]
    fn test_descriptor_from_str_from_output_of_macro() {
        let tpub = bip32::ExtendedPubKey::from_str("tpubD6NzVbkrYhZ4XHndKkuB8FifXm8r5FQHwrN6oZuWCz13qb93rtgKvD4PQsqC4HP4yhV3tA2fqr2RbY5mNXfM7RxXUoeABoDtsFUq2zJq6YK").unwrap();
        let path = bip32::DerivationPath::from_str("m/1/2").unwrap();
        let key = (tpub, path).to_descriptor_key().unwrap();

        // make a descriptor out of it
        let desc = crate::descriptor!(wpkh(key)).unwrap();

        let (wallet_desc, _) = desc.to_wallet_descriptor(Network::Testnet).unwrap();
        let wallet_desc_str = wallet_desc.to_string();
        assert_eq!(wallet_desc_str, "wpkh(tpubD6NzVbkrYhZ4XHndKkuB8FifXm8r5FQHwrN6oZuWCz13qb93rtgKvD4PQsqC4HP4yhV3tA2fqr2RbY5mNXfM7RxXUoeABoDtsFUq2zJq6YK/1/2/*)");

        let (wallet_desc2, _) = wallet_desc_str
            .to_wallet_descriptor(Network::Testnet)
            .unwrap();
        assert_eq!(wallet_desc, wallet_desc2)
    }
}
