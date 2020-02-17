use std::fmt::{self, Display};
use std::str::FromStr;

use bitcoin::hashes::hex::{FromHex, ToHex};
use bitcoin::secp256k1;
use bitcoin::util::base58;
use bitcoin::util::bip32::{
    ChildNumber, DerivationPath, ExtendedPrivKey, ExtendedPubKey, Fingerprint,
};
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
    pub master_fingerprint: Option<Fingerprint>,
    pub master_derivation: Option<DerivationPath>,
    pub pubkey: ExtendedPubKey,
    pub secret: Option<ExtendedPrivKey>,
    pub path: DerivationPath,
    pub final_index: DerivationIndex,
}

impl DescriptorExtendedKey {
    pub fn full_path(&self, index: u32) -> DerivationPath {
        let mut final_path: Vec<ChildNumber> = Vec::new();
        if let Some(path) = &self.master_derivation {
            let path_as_vec: Vec<ChildNumber> = path.clone().into();
            final_path.extend_from_slice(&path_as_vec);
        }
        let our_path: Vec<ChildNumber> = self.path_with_index(index).into();
        final_path.extend_from_slice(&our_path);

        final_path.into()
    }

    pub fn path_with_index(&self, index: u32) -> DerivationPath {
        let mut final_path: Vec<ChildNumber> = Vec::new();
        let our_path: Vec<ChildNumber> = self.path.clone().into();
        final_path.extend_from_slice(&our_path);
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
            let derive_priv = xprv.derive_priv(ctx, &self.path_with_index(index))?;
            Ok(ExtendedPubKey::from_private(ctx, &derive_priv))
        } else {
            Ok(self.pubkey.derive_pub(ctx, &self.path_with_index(index))?)
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
            write!(f, "[{}", fingerprint.to_hex())?;
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
                if inp.len() < 9 {
                    return Err(super::Error::MalformedInput);
                }

                let master_fingerprint = &inp[1..9];
                let close_bracket_index =
                    &inp[9..].find("]").ok_or(super::Error::MalformedInput)?;
                let path = if *close_bracket_index > 0 {
                    Some(DerivationPath::from_str(&format!(
                        "m{}",
                        &inp[9..9 + *close_bracket_index]
                    ))?)
                } else {
                    None
                };

                (
                    Some(Fingerprint::from_hex(master_fingerprint)?),
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

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use bitcoin::hashes::hex::FromHex;
    use bitcoin::util::bip32::{ChildNumber, DerivationPath};

    use crate::descriptor::*;

    macro_rules! hex_fingerprint {
        ($hex:expr) => {
            Fingerprint::from_hex($hex).unwrap()
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
    fn test_derivation_index_fixed() {
        let index = DerivationIndex::Fixed;
        assert_eq!(index.as_path(1337), DerivationPath::from(vec![]));
        assert_eq!(format!("{}", index), "");
    }

    #[test]
    fn test_derivation_index_normal() {
        let index = DerivationIndex::Normal;
        assert_eq!(
            index.as_path(1337),
            DerivationPath::from(vec![ChildNumber::Normal { index: 1337 }])
        );
        assert_eq!(format!("{}", index), "/*");
    }

    #[test]
    fn test_derivation_index_hardened() {
        let index = DerivationIndex::Hardened;
        assert_eq!(
            index.as_path(1337),
            DerivationPath::from(vec![ChildNumber::Hardened { index: 1337 }])
        );
        assert_eq!(format!("{}", index), "/*'");
    }

    #[test]
    fn test_parse_xpub_no_path_fixed() {
        let key = "xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL";
        let ek = DescriptorExtendedKey::from_str(key).unwrap();
        assert_eq!(ek.pubkey.fingerprint(), hex_fingerprint!("31a507b8"));
        assert_eq!(ek.path, deriv_path!());
        assert_eq!(ek.final_index, DerivationIndex::Fixed);
    }

    #[test]
    fn test_parse_xpub_with_path_fixed() {
        let key = "xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL/1/2/3";
        let ek = DescriptorExtendedKey::from_str(key).unwrap();
        assert_eq!(ek.pubkey.fingerprint(), hex_fingerprint!("31a507b8"));
        assert_eq!(ek.path, deriv_path!("m/1/2/3"));
        assert_eq!(ek.final_index, DerivationIndex::Fixed);
    }

    #[test]
    fn test_parse_xpub_with_path_normal() {
        let key = "xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL/1/2/3/*";
        let ek = DescriptorExtendedKey::from_str(key).unwrap();
        assert_eq!(ek.pubkey.fingerprint(), hex_fingerprint!("31a507b8"));
        assert_eq!(ek.path, deriv_path!("m/1/2/3"));
        assert_eq!(ek.final_index, DerivationIndex::Normal);
    }

    #[test]
    #[should_panic(expected = "HardenedDerivationOnXpub")]
    fn test_parse_xpub_with_path_hardened() {
        let key = "xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL/*'";
        let ek = DescriptorExtendedKey::from_str(key).unwrap();
        assert_eq!(ek.pubkey.fingerprint(), hex_fingerprint!("31a507b8"));
        assert_eq!(ek.path, deriv_path!("m/1/2/3"));
        assert_eq!(ek.final_index, DerivationIndex::Fixed);
    }

    #[test]
    fn test_parse_tprv_with_path_hardened() {
        let key = "tprv8ZgxMBicQKsPduL5QnGihpprdHyypMGi4DhimjtzYemu7se5YQNcZfAPLqXRuGHb5ZX2eTQj62oNqMnyxJ7B7wz54Uzswqw8fFqMVdcmVF7/1/2/3/*'";
        let ek = DescriptorExtendedKey::from_str(key).unwrap();
        assert!(ek.secret.is_some());
        assert_eq!(ek.pubkey.fingerprint(), hex_fingerprint!("5ea4190e"));
        assert_eq!(ek.path, deriv_path!("m/1/2/3"));
        assert_eq!(ek.final_index, DerivationIndex::Hardened);
    }

    #[test]
    fn test_parse_xpub_master_details() {
        let key = "[d34db33f/44'/0'/0']xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL";
        let ek = DescriptorExtendedKey::from_str(key).unwrap();
        assert_eq!(ek.master_fingerprint, Some(hex_fingerprint!("d34db33f")));
        assert_eq!(ek.master_derivation, Some(deriv_path!("m/44'/0'/0'")));
    }

    #[test]
    fn test_parse_xpub_master_details_empty_derivation() {
        let key = "[d34db33f]xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL";
        let ek = DescriptorExtendedKey::from_str(key).unwrap();
        assert_eq!(ek.master_fingerprint, Some(hex_fingerprint!("d34db33f")));
        assert_eq!(ek.master_derivation, None);
    }

    #[test]
    #[should_panic(expected = "MalformedInput")]
    fn test_parse_xpub_short_input() {
        let key = "[d34d";
        DescriptorExtendedKey::from_str(key).unwrap();
    }

    #[test]
    #[should_panic(expected = "MalformedInput")]
    fn test_parse_xpub_missing_closing_bracket() {
        let key = "[d34db33fxpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL";
        DescriptorExtendedKey::from_str(key).unwrap();
    }

    #[test]
    #[should_panic(expected = "InvalidChar")]
    fn test_parse_xpub_invalid_fingerprint() {
        let key = "[d34db33z]xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL";
        DescriptorExtendedKey::from_str(key).unwrap();
    }

    #[test]
    fn test_xpub_normal_full_path() {
        let key = "xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL/1/2/*";
        let ek = DescriptorExtendedKey::from_str(key).unwrap();
        assert_eq!(ek.full_path(42), deriv_path!("m/1/2/42"));
    }

    #[test]
    fn test_xpub_fixed_full_path() {
        let key = "xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL/1/2";
        let ek = DescriptorExtendedKey::from_str(key).unwrap();
        assert_eq!(ek.full_path(42), deriv_path!("m/1/2"));
        assert_eq!(ek.full_path(1337), deriv_path!("m/1/2"));
    }
}
