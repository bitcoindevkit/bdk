use std::fmt::{self, Display};
use std::str::FromStr;

use bitcoin::secp256k1;
use bitcoin::util::base58;
use bitcoin::util::bip32::{ChildNumber, DerivationPath, ExtendedPrivKey, ExtendedPubKey};
use bitcoin::PublicKey;

#[allow(unused_imports)]
use log::{debug, error, info, trace};

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum DerivationIndex {
    Fixed,
    Normal,
    Hardened,
}

impl DerivationIndex {
    fn as_path(&self, index: u32) -> DerivationPath {
        match self {
            DerivationIndex::Fixed => vec![],
            DerivationIndex::Normal => vec![ChildNumber::Normal { index }],
            DerivationIndex::Hardened => vec![ChildNumber::Hardened { index }],
        }
        .into()
    }
}

impl Display for DerivationIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let chars = match *self {
            Self::Fixed => "",
            Self::Normal => "/*",
            Self::Hardened => "/*'",
        };

        write!(f, "{}", chars)
    }
}

#[derive(Clone, Debug)]
pub struct DescriptorExtendedKey {
    pub master_fingerprint: Option<String>,
    pub master_derivation: Option<DerivationPath>,
    pub pubkey: ExtendedPubKey,
    pub secret: Option<ExtendedPrivKey>,
    pub path: DerivationPath,
    pub final_index: DerivationIndex,
}

impl DescriptorExtendedKey {
    pub fn full_path(&self, index: u32) -> DerivationPath {
        let mut final_path: Vec<ChildNumber> = self.path.clone().into();
        let other_path: Vec<ChildNumber> = self.final_index.as_path(index).into();
        final_path.extend_from_slice(&other_path);

        final_path.into()
    }

    pub fn derive<C: secp256k1::Verification + secp256k1::Signing>(
        &self,
        ctx: &secp256k1::Secp256k1<C>,
        index: u32,
    ) -> Result<PublicKey, super::Error> {
        Ok(self.derive_xpub(ctx, index)?.public_key)
    }

    pub fn derive_xpub<C: secp256k1::Verification + secp256k1::Signing>(
        &self,
        ctx: &secp256k1::Secp256k1<C>,
        index: u32,
    ) -> Result<ExtendedPubKey, super::Error> {
        if let Some(xprv) = self.secret {
            let derive_priv = xprv.derive_priv(ctx, &self.full_path(index))?;
            Ok(ExtendedPubKey::from_private(ctx, &derive_priv))
        } else {
            Ok(self.pubkey.derive_pub(ctx, &self.full_path(index))?)
        }
    }

    pub fn root_xpub<C: secp256k1::Verification + secp256k1::Signing>(
        &self,
        ctx: &secp256k1::Secp256k1<C>,
    ) -> ExtendedPubKey {
        if let Some(ref xprv) = self.secret {
            ExtendedPubKey::from_private(ctx, xprv)
        } else {
            self.pubkey
        }
    }
}

impl Display for DescriptorExtendedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref fingerprint) = self.master_fingerprint {
            write!(f, "[{}", fingerprint)?;
            if let Some(ref path) = self.master_derivation {
                write!(f, "{}", &path.to_string()[1..])?;
            }
            write!(f, "]")?;
        }

        if let Some(xprv) = self.secret {
            write!(f, "{}", xprv)?
        } else {
            write!(f, "{}", self.pubkey)?
        }

        write!(f, "{}{}", &self.path.to_string()[1..], self.final_index)
    }
}

impl FromStr for DescriptorExtendedKey {
    type Err = super::Error;

    fn from_str(inp: &str) -> Result<DescriptorExtendedKey, Self::Err> {
        let len = inp.len();

        let (master_fingerprint, master_derivation, offset) = match inp.starts_with("[") {
            false => (None, None, 0),
            true => {
                let master_fingerprint = &inp[1..9];
                let close_bracket_index = &inp[9..]
                    .find("]")
                    .ok_or(super::Error::MissingClosingBracket)?;
                let path = if *close_bracket_index > 0 {
                    Some(DerivationPath::from_str(&format!(
                        "m{}",
                        &inp[9..9 + *close_bracket_index]
                    ))?)
                } else {
                    None
                };

                (
                    Some(master_fingerprint.into()),
                    path,
                    9 + *close_bracket_index + 1,
                )
            }
        };

        let (key_range, offset) = match &inp[offset..].find("/") {
            Some(index) => (offset..offset + *index, offset + *index),
            None => (offset..len, len),
        };
        let data = base58::from_check(&inp[key_range.clone()])?;
        let secp = secp256k1::Secp256k1::new();
        let (pubkey, secret) = match &data[0..4] {
            [0x04u8, 0x88, 0xB2, 0x1E] | [0x04u8, 0x35, 0x87, 0xCF] => {
                (ExtendedPubKey::from_str(&inp[key_range])?, None)
            }
            [0x04u8, 0x88, 0xAD, 0xE4] | [0x04u8, 0x35, 0x83, 0x94] => {
                let private = ExtendedPrivKey::from_str(&inp[key_range])?;
                (ExtendedPubKey::from_private(&secp, &private), Some(private))
            }
            data => return Err(super::Error::InvalidPrefix(data.into())),
        };

        let (path, final_index, _) = match &inp[offset..].starts_with("/") {
            false => (DerivationPath::from(vec![]), DerivationIndex::Fixed, offset),
            true => {
                let (all, skip) = match &inp[len - 2..len] {
                    "/*" => (DerivationIndex::Normal, 2),
                    "*'" | "*h" => (DerivationIndex::Hardened, 3),
                    _ => (DerivationIndex::Fixed, 0),
                };

                if all == DerivationIndex::Hardened && secret.is_none() {
                    return Err(super::Error::HardenedDerivationOnXpub);
                }

                (
                    DerivationPath::from_str(&format!("m{}", &inp[offset..len - skip]))?,
                    all,
                    len,
                )
            }
        };

        Ok(DescriptorExtendedKey {
            master_fingerprint,
            master_derivation,
            pubkey,
            secret,
            path,
            final_index,
        })
    }
}
