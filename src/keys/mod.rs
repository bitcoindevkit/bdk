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

use std::any::TypeId;
use std::marker::PhantomData;

use bitcoin::util::bip32;
use bitcoin::{PrivateKey, PublicKey};

use miniscript::descriptor::{DescriptorPublicKey, DescriptorSecretKey, DescriptorXKey, KeyMap};
pub use miniscript::ScriptContext;
use miniscript::{Miniscript, Terminal};

use crate::Error;

#[cfg(feature = "keys-bip39")]
#[cfg_attr(docsrs, doc(cfg(feature = "keys-bip39")))]
pub mod bip39;

/// Container for public or secret keys
pub enum DescriptorKey<Ctx: ScriptContext> {
    Public(DescriptorPublicKey, PhantomData<Ctx>),
    Secret(DescriptorSecretKey, PhantomData<Ctx>),
}

impl<Ctx: ScriptContext> DescriptorKey<Ctx> {
    // This method is used internally by `bdk::fragment!` and `bdk::descriptor!`. It has to be
    // public because it is effectively called by external crates, once the macros are expanded,
    // but since it is not meant to be part of the public api we hide it from the docs.
    #[doc(hidden)]
    pub fn into_key_and_secret(self) -> Result<(DescriptorPublicKey, KeyMap), Error> {
        match self {
            DescriptorKey::Public(public, _) => Ok((public, KeyMap::default())),
            DescriptorKey::Secret(secret, _) => {
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

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum ScriptContextEnum {
    Legacy,
    Segwitv0,
}

impl ScriptContextEnum {
    pub fn is_legacy(&self) -> bool {
        self == &ScriptContextEnum::Legacy
    }

    pub fn is_segwit_v0(&self) -> bool {
        self == &ScriptContextEnum::Segwitv0
    }
}

pub trait ExtScriptContext: ScriptContext {
    fn as_enum() -> ScriptContextEnum;

    fn is_legacy() -> bool {
        Self::as_enum().is_legacy()
    }

    fn is_segwit_v0() -> bool {
        Self::as_enum().is_segwit_v0()
    }
}

impl<Ctx: ScriptContext + 'static> ExtScriptContext for Ctx {
    fn as_enum() -> ScriptContextEnum {
        match TypeId::of::<Ctx>() {
            t if t == TypeId::of::<miniscript::Legacy>() => ScriptContextEnum::Legacy,
            t if t == TypeId::of::<miniscript::Segwitv0>() => ScriptContextEnum::Segwitv0,
            _ => unimplemented!("Unknown ScriptContext type"),
        }
    }
}

/// Trait for objects that can be turned into a public or secret [`DescriptorKey`]
///
/// The generic type `Ctx` is used to define the context in which the key is valid: some key
/// formats, like the mnemonics used by Electrum wallets, encode internally whether the wallet is
/// legacy or segwit. Thus, trying to turn a valid legacy mnemonic into a `DescriptorKey`
/// that would become part of a segwit descriptor should fail.
///
/// For key types that do care about this, the [`ExtScriptContext`] trait provides some useful
/// methods that can be used to check at runtime which `Ctx` is being used.
///
/// For key types that that do not need to check this at runtime (because they can only work within a
/// single `Ctx`), the "specialized" trait can be implemented to make the compiler handle the type
/// checking.
///
/// ## Examples
///
/// Key type valid in any context:
///
/// ```
/// use bdk::bitcoin::PublicKey;
///
/// use bdk::keys::{ScriptContext, ToDescriptorKey, DescriptorKey};
/// use bdk::Error;
///
/// pub struct MyKeyType {
///     pubkey: PublicKey,
/// }
///
/// impl<Ctx: ScriptContext> ToDescriptorKey<Ctx> for MyKeyType {
///     fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, Error> {
///         self.pubkey.to_descriptor_key()
///     }
/// }
/// ```
///
/// Key type that internally encodes in which context it's valid. The context is checked at runtime:
///
/// ```
/// use bdk::bitcoin::PublicKey;
///
/// use bdk::keys::{ExtScriptContext, ScriptContext, ToDescriptorKey, DescriptorKey};
/// use bdk::Error;
///
/// pub struct MyKeyType {
///     is_legacy: bool,
///     pubkey: PublicKey,
/// }
///
/// impl<Ctx: ScriptContext + 'static> ToDescriptorKey<Ctx> for MyKeyType {
///     fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, Error> {
///         if Ctx::is_legacy() == self.is_legacy {
///             self.pubkey.to_descriptor_key()
///         } else {
///             Err(Error::Generic("Invalid key context".into()))
///         }
///     }
/// }
/// ```
///
/// Key type that can only work within [`miniscript::Segwitv0`] context. Only the specialized version
/// of the trait is implemented.
///
/// This example deliberately fails to compile, to demonstrate how the compiler can catch when keys
/// are misused. In this case, the "segwit-only" key is used to build a `pkh()` descriptor, which
/// makes the compiler (correctly) fail.
///
/// ```compile_fail
/// use std::str::FromStr;
/// use bdk::bitcoin::PublicKey;
///
/// use bdk::keys::{ToDescriptorKey, DescriptorKey};
/// use bdk::Error;
///
/// pub struct MySegwitOnlyKeyType {
///     pubkey: PublicKey,
/// }
///
/// impl ToDescriptorKey<bdk::miniscript::Segwitv0> for MySegwitOnlyKeyType {
///     fn to_descriptor_key(self) -> Result<DescriptorKey<bdk::miniscript::Segwitv0>, Error> {
///         self.pubkey.to_descriptor_key()
///     }
/// }
///
/// let key = MySegwitOnlyKeyType {
///     pubkey: PublicKey::from_str("...")?,
/// };
/// let (descriptor, _) = bdk::descriptor!(pkh ( key ) )?;
/// //                                    ^^^^^ changing this to `wpkh` would make it compile
///
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub trait ToDescriptorKey<Ctx: ScriptContext>: Sized {
    /// Turn the key into a [`DescriptorKey`] within the requested [`ScriptContext`]
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, Error>;

    // Used internally by `bdk::fragment!` to build `pk_k()` fragments
    #[doc(hidden)]
    fn into_miniscript_and_secret(
        self,
    ) -> Result<(Miniscript<DescriptorPublicKey, Ctx>, KeyMap), Error> {
        let descriptor_key = self.to_descriptor_key()?;
        let (key, key_map) = descriptor_key.into_key_and_secret()?;

        Ok((Miniscript::from_ast(Terminal::PkK(key))?, key_map))
    }
}

// Used internally by `bdk::fragment!` to build `multi()` fragments
#[doc(hidden)]
pub fn make_multi<Pk: ToDescriptorKey<Ctx>, Ctx: ScriptContext>(
    thresh: usize,
    pks: Vec<Pk>,
) -> Result<(Miniscript<DescriptorPublicKey, Ctx>, KeyMap), Error> {
    let (pks, key_maps): (Vec<_>, Vec<_>) = pks
        .into_iter()
        .map(|key| {
            key.to_descriptor_key()
                .and_then(DescriptorKey::into_key_and_secret)
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .unzip();

    let key_map = key_maps
        .into_iter()
        .fold(KeyMap::default(), |mut acc, map| {
            acc.extend(map.into_iter());
            acc
        });

    Ok((Miniscript::from_ast(Terminal::Multi(thresh, pks))?, key_map))
}

/// The "identity" conversion is used internally by some `bdk::fragment`s
impl<Ctx: ScriptContext> ToDescriptorKey<Ctx> for DescriptorKey<Ctx> {
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, Error> {
        Ok(self)
    }
}

impl<Ctx: ScriptContext> ToDescriptorKey<Ctx> for DescriptorPublicKey {
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, Error> {
        Ok(DescriptorKey::Public(self, PhantomData))
    }
}

impl<Ctx: ScriptContext> ToDescriptorKey<Ctx> for PublicKey {
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, Error> {
        Ok(DescriptorKey::Public(
            DescriptorPublicKey::PubKey(self),
            PhantomData,
        ))
    }
}

/// This assumes that "is_wildcard" is true, since this is generally the way extended keys are used
impl<Ctx: ScriptContext> ToDescriptorKey<Ctx> for (bip32::ExtendedPubKey, bip32::DerivationPath) {
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, Error> {
        Ok(DescriptorKey::Public(
            DescriptorPublicKey::XPub(DescriptorXKey {
                source: None,
                xkey: self.0,
                derivation_path: self.1,
                is_wildcard: true,
            }),
            PhantomData,
        ))
    }
}

impl<Ctx: ScriptContext> ToDescriptorKey<Ctx> for DescriptorSecretKey {
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, Error> {
        Ok(DescriptorKey::Secret(self, PhantomData))
    }
}

impl<Ctx: ScriptContext> ToDescriptorKey<Ctx> for PrivateKey {
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, Error> {
        Ok(DescriptorKey::Secret(
            DescriptorSecretKey::PrivKey(self),
            PhantomData,
        ))
    }
}

/// This assumes that "is_wildcard" is true, since this is generally the way extended keys are used
impl<Ctx: ScriptContext> ToDescriptorKey<Ctx> for (bip32::ExtendedPrivKey, bip32::DerivationPath) {
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, Error> {
        Ok(DescriptorKey::Secret(
            DescriptorSecretKey::XPrv(DescriptorXKey {
                source: None,
                xkey: self.0,
                derivation_path: self.1,
                is_wildcard: true,
            }),
            PhantomData,
        ))
    }
}
