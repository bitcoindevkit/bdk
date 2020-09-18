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

//! Key formats

use bitcoin::util::bip32;
use bitcoin::{PrivateKey, PublicKey};

use miniscript::descriptor::{DescriptorPublicKey, DescriptorSecretKey, DescriptorXKey, KeyMap};

use crate::Error;

#[cfg(feature = "keys-bip39")]
#[cfg_attr(docsrs, doc(cfg(feature = "keys-bip39")))]
pub mod bip39;

/// Container for public or secret keys
pub enum DescriptorKey {
    Public(DescriptorPublicKey),
    Secret(DescriptorSecretKey),
}

impl DescriptorKey {
    #[doc(hidden)]
    pub fn into_key_and_secret(self) -> Result<(DescriptorPublicKey, KeyMap), Error> {
        match self {
            DescriptorKey::Public(public) => Ok((public, KeyMap::default())),
            DescriptorKey::Secret(secret) => {
                let mut key_map = KeyMap::with_capacity(1);

                let public = secret
                    .as_public()
                    .map_err(|e| miniscript::Error::Unexpected(e.to_string()))?;
                key_map.insert(public.clone(), secret);

                Ok((public, key_map))
            }
        }
    }
}

/// Trait for objects that can be turned into a public or secret [`DescriptorKey`]
pub trait ToDescriptorKey {
    fn to_descriptor_key(self) -> Result<DescriptorKey, Error>;
}

/// Identity conversion. This is used internally by [`bdk::fragment`]
impl ToDescriptorKey for DescriptorKey {
    fn to_descriptor_key(self) -> Result<DescriptorKey, Error> {
        Ok(self)
    }
}

impl ToDescriptorKey for DescriptorPublicKey {
    fn to_descriptor_key(self) -> Result<DescriptorKey, Error> {
        Ok(DescriptorKey::Public(self))
    }
}

impl ToDescriptorKey for PublicKey {
    fn to_descriptor_key(self) -> Result<DescriptorKey, Error> {
        Ok(DescriptorKey::Public(DescriptorPublicKey::PubKey(self)))
    }
}

impl ToDescriptorKey for (bip32::ExtendedPubKey, bip32::DerivationPath) {
    fn to_descriptor_key(self) -> Result<DescriptorKey, Error> {
        Ok(DescriptorKey::Public(DescriptorPublicKey::XPub(
            DescriptorXKey {
                source: None,
                xkey: self.0,
                derivation_path: self.1,
                is_wildcard: true,
            },
        )))
    }
}

impl ToDescriptorKey for DescriptorSecretKey {
    fn to_descriptor_key(self) -> Result<DescriptorKey, Error> {
        Ok(DescriptorKey::Secret(self))
    }
}

impl ToDescriptorKey for PrivateKey {
    fn to_descriptor_key(self) -> Result<DescriptorKey, Error> {
        Ok(DescriptorKey::Secret(DescriptorSecretKey::PrivKey(self)))
    }
}

impl ToDescriptorKey for (bip32::ExtendedPrivKey, bip32::DerivationPath) {
    fn to_descriptor_key(self) -> Result<DescriptorKey, Error> {
        Ok(DescriptorKey::Secret(DescriptorSecretKey::XPrv(
            DescriptorXKey {
                source: None,
                xkey: self.0,
                derivation_path: self.1,
                is_wildcard: true,
            },
        )))
    }
}
