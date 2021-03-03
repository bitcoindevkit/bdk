// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

//! Derived descriptor keys

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;

use bitcoin::hashes::hash160;
use bitcoin::PublicKey;

pub use miniscript::{
    descriptor::KeyMap, descriptor::Wildcard, Descriptor, DescriptorPublicKey, Legacy, Miniscript,
    ScriptContext, Segwitv0,
};
use miniscript::{MiniscriptKey, ToPublicKey, TranslatePk};

use crate::wallet::utils::SecpCtx;

/// Extended [`DescriptorPublicKey`] that has been derived
///
/// Derived keys are guaranteed to never contain wildcards of any kind
#[derive(Debug, Clone)]
pub struct DerivedDescriptorKey<'s>(DescriptorPublicKey, &'s SecpCtx);

impl<'s> DerivedDescriptorKey<'s> {
    /// Construct a new derived key
    ///
    /// Panics if the key is wildcard
    pub fn new(key: DescriptorPublicKey, secp: &'s SecpCtx) -> DerivedDescriptorKey<'s> {
        if let DescriptorPublicKey::XPub(xpub) = &key {
            assert!(xpub.wildcard == Wildcard::None)
        }

        DerivedDescriptorKey(key, secp)
    }
}

impl<'s> Deref for DerivedDescriptorKey<'s> {
    type Target = DescriptorPublicKey;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'s> PartialEq for DerivedDescriptorKey<'s> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<'s> Eq for DerivedDescriptorKey<'s> {}

impl<'s> PartialOrd for DerivedDescriptorKey<'s> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.0.partial_cmp(&other.0)
    }
}

impl<'s> Ord for DerivedDescriptorKey<'s> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl<'s> fmt::Display for DerivedDescriptorKey<'s> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'s> Hash for DerivedDescriptorKey<'s> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<'s> MiniscriptKey for DerivedDescriptorKey<'s> {
    type Hash = Self;

    fn to_pubkeyhash(&self) -> Self::Hash {
        DerivedDescriptorKey(self.0.to_pubkeyhash(), self.1)
    }

    fn is_uncompressed(&self) -> bool {
        self.0.is_uncompressed()
    }
    fn serialized_len(&self) -> usize {
        self.0.serialized_len()
    }
}

impl<'s> ToPublicKey for DerivedDescriptorKey<'s> {
    fn to_public_key(&self) -> PublicKey {
        match &self.0 {
            DescriptorPublicKey::SinglePub(ref spub) => spub.key.to_public_key(),
            DescriptorPublicKey::XPub(ref xpub) => {
                xpub.xkey
                    .derive_pub(self.1, &xpub.derivation_path)
                    .expect("Shouldn't fail, only normal derivations")
                    .public_key
            }
        }
    }

    fn hash_to_hash160(hash: &Self::Hash) -> hash160::Hash {
        hash.to_public_key().to_pubkeyhash()
    }
}

pub(crate) trait AsDerived {
    // Derive a descriptor and transform all of its keys to `DerivedDescriptorKey`
    fn as_derived<'s>(&self, index: u32, secp: &'s SecpCtx)
        -> Descriptor<DerivedDescriptorKey<'s>>;

    // Transform the keys into `DerivedDescriptorKey`.
    //
    // Panics if the descriptor is not "fixed", i.e. if it's derivable
    fn as_derived_fixed<'s>(&self, secp: &'s SecpCtx) -> Descriptor<DerivedDescriptorKey<'s>>;
}

impl AsDerived for Descriptor<DescriptorPublicKey> {
    fn as_derived<'s>(
        &self,
        index: u32,
        secp: &'s SecpCtx,
    ) -> Descriptor<DerivedDescriptorKey<'s>> {
        self.derive(index).translate_pk_infallible(
            |key| DerivedDescriptorKey::new(key.clone(), secp),
            |key| DerivedDescriptorKey::new(key.clone(), secp),
        )
    }

    fn as_derived_fixed<'s>(&self, secp: &'s SecpCtx) -> Descriptor<DerivedDescriptorKey<'s>> {
        assert!(!self.is_deriveable());

        self.as_derived(0, secp)
    }
}
