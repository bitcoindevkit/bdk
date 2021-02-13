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
use std::collections::HashSet;
use std::marker::PhantomData;
use std::ops::Deref;
use std::str::FromStr;

use bitcoin::secp256k1::{self, Secp256k1, Signing};

use bitcoin::util::bip32;
use bitcoin::{Network, PrivateKey, PublicKey};

use miniscript::descriptor::{Descriptor, DescriptorXKey, Wildcard};
pub use miniscript::descriptor::{
    DescriptorPublicKey, DescriptorSecretKey, DescriptorSinglePriv, DescriptorSinglePub, KeyMap,
    SortedMultiVec,
};
pub use miniscript::ScriptContext;
use miniscript::{Miniscript, Terminal};

use crate::descriptor::{CheckMiniscript, DescriptorError};
use crate::wallet::utils::SecpCtx;

#[cfg(feature = "keys-bip39")]
#[cfg_attr(docsrs, doc(cfg(feature = "keys-bip39")))]
pub mod bip39;

/// Set of valid networks for a key
pub type ValidNetworks = HashSet<Network>;

/// Create a set containing mainnet, testnet and regtest
pub fn any_network() -> ValidNetworks {
    vec![
        Network::Bitcoin,
        Network::Testnet,
        Network::Regtest,
        Network::Signet,
    ]
    .into_iter()
    .collect()
}
/// Create a set only containing mainnet
pub fn mainnet_network() -> ValidNetworks {
    vec![Network::Bitcoin].into_iter().collect()
}
/// Create a set containing testnet and regtest
pub fn test_networks() -> ValidNetworks {
    vec![Network::Testnet, Network::Regtest, Network::Signet]
        .into_iter()
        .collect()
}
/// Compute the intersection of two sets
pub fn merge_networks(a: &ValidNetworks, b: &ValidNetworks) -> ValidNetworks {
    a.intersection(b).cloned().collect()
}

/// Container for public or secret keys
#[derive(Debug)]
pub enum DescriptorKey<Ctx: ScriptContext> {
    #[doc(hidden)]
    Public(DescriptorPublicKey, ValidNetworks, PhantomData<Ctx>),
    #[doc(hidden)]
    Secret(DescriptorSecretKey, ValidNetworks, PhantomData<Ctx>),
}

impl<Ctx: ScriptContext> DescriptorKey<Ctx> {
    /// Create an instance given a public key and a set of valid networks
    pub fn from_public(public: DescriptorPublicKey, networks: ValidNetworks) -> Self {
        DescriptorKey::Public(public, networks, PhantomData)
    }

    /// Create an instance given a secret key and a set of valid networks
    pub fn from_secret(secret: DescriptorSecretKey, networks: ValidNetworks) -> Self {
        DescriptorKey::Secret(secret, networks, PhantomData)
    }

    /// Override the computed set of valid networks
    pub fn override_valid_networks(self, networks: ValidNetworks) -> Self {
        match self {
            DescriptorKey::Public(key, _, _) => DescriptorKey::Public(key, networks, PhantomData),
            DescriptorKey::Secret(key, _, _) => DescriptorKey::Secret(key, networks, PhantomData),
        }
    }

    // This method is used internally by `bdk::fragment!` and `bdk::descriptor!`. It has to be
    // public because it is effectively called by external crates, once the macros are expanded,
    // but since it is not meant to be part of the public api we hide it from the docs.
    #[doc(hidden)]
    pub fn extract(
        self,
        secp: &SecpCtx,
    ) -> Result<(DescriptorPublicKey, KeyMap, ValidNetworks), KeyError> {
        match self {
            DescriptorKey::Public(public, valid_networks, _) => {
                Ok((public, KeyMap::default(), valid_networks))
            }
            DescriptorKey::Secret(secret, valid_networks, _) => {
                let mut key_map = KeyMap::with_capacity(1);

                let public = secret
                    .as_public(secp)
                    .map_err(|e| miniscript::Error::Unexpected(e.to_string()))?;
                key_map.insert(public.clone(), secret);

                Ok((public, key_map, valid_networks))
            }
        }
    }
}

/// Enum representation of the known valid [`ScriptContext`]s
#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum ScriptContextEnum {
    /// Legacy scripts
    Legacy,
    /// Segwitv0 scripts
    Segwitv0,
}

impl ScriptContextEnum {
    /// Returns whether the script context is [`ScriptContextEnum::Legacy`]
    pub fn is_legacy(&self) -> bool {
        self == &ScriptContextEnum::Legacy
    }

    /// Returns whether the script context is [`ScriptContextEnum::Segwitv0`]
    pub fn is_segwit_v0(&self) -> bool {
        self == &ScriptContextEnum::Segwitv0
    }
}

/// Trait that adds extra useful methods to [`ScriptContext`]s
pub trait ExtScriptContext: ScriptContext {
    /// Returns the [`ScriptContext`] as a [`ScriptContextEnum`]
    fn as_enum() -> ScriptContextEnum;

    /// Returns whether the script context is [`Legacy`](miniscript::Legacy)
    fn is_legacy() -> bool {
        Self::as_enum().is_legacy()
    }

    /// Returns whether the script context is [`Segwitv0`](miniscript::Segwitv0)
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
/// For key types that can do this check statically (because they can only work within a
/// single `Ctx`), the "specialized" trait can be implemented to make the compiler handle the type
/// checking.
///
/// Keys also have control over the networks they support: constructing the return object with
/// [`DescriptorKey::from_public`] or [`DescriptorKey::from_secret`] allows to specify a set of
/// [`ValidNetworks`].
///
/// ## Examples
///
/// Key type valid in any context:
///
/// ```
/// use bdk::bitcoin::PublicKey;
///
/// use bdk::keys::{DescriptorKey, KeyError, ScriptContext, IntoDescriptorKey};
///
/// pub struct MyKeyType {
///     pubkey: PublicKey,
/// }
///
/// impl<Ctx: ScriptContext> IntoDescriptorKey<Ctx> for MyKeyType {
///     fn into_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
///         self.pubkey.into_descriptor_key()
///     }
/// }
/// ```
///
/// Key type that is only valid on mainnet:
///
/// ```
/// use bdk::bitcoin::PublicKey;
///
/// use bdk::keys::{
///     mainnet_network, DescriptorKey, DescriptorPublicKey, DescriptorSinglePub, KeyError,
///     ScriptContext, IntoDescriptorKey,
/// };
///
/// pub struct MyKeyType {
///     pubkey: PublicKey,
/// }
///
/// impl<Ctx: ScriptContext> IntoDescriptorKey<Ctx> for MyKeyType {
///     fn into_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
///         Ok(DescriptorKey::from_public(
///             DescriptorPublicKey::SinglePub(DescriptorSinglePub {
///                 origin: None,
///                 key: self.pubkey,
///             }),
///             mainnet_network(),
///         ))
///     }
/// }
/// ```
///
/// Key type that internally encodes in which context it's valid. The context is checked at runtime:
///
/// ```
/// use bdk::bitcoin::PublicKey;
///
/// use bdk::keys::{DescriptorKey, ExtScriptContext, KeyError, ScriptContext, IntoDescriptorKey};
///
/// pub struct MyKeyType {
///     is_legacy: bool,
///     pubkey: PublicKey,
/// }
///
/// impl<Ctx: ScriptContext + 'static> IntoDescriptorKey<Ctx> for MyKeyType {
///     fn into_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
///         if Ctx::is_legacy() == self.is_legacy {
///             self.pubkey.into_descriptor_key()
///         } else {
///             Err(KeyError::InvalidScriptContext)
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
/// use bdk::bitcoin::PublicKey;
/// use std::str::FromStr;
///
/// use bdk::keys::{DescriptorKey, KeyError, IntoDescriptorKey};
///
/// pub struct MySegwitOnlyKeyType {
///     pubkey: PublicKey,
/// }
///
/// impl IntoDescriptorKey<bdk::miniscript::Segwitv0> for MySegwitOnlyKeyType {
///     fn into_descriptor_key(self) -> Result<DescriptorKey<bdk::miniscript::Segwitv0>, KeyError> {
///         self.pubkey.into_descriptor_key()
///     }
/// }
///
/// let key = MySegwitOnlyKeyType {
///     pubkey: PublicKey::from_str("...")?,
/// };
/// let (descriptor, _, _) = bdk::descriptor!(pkh(key))?;
/// //                                       ^^^^^ changing this to `wpkh` would make it compile
///
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub trait IntoDescriptorKey<Ctx: ScriptContext>: Sized {
    /// Turn the key into a [`DescriptorKey`] within the requested [`ScriptContext`]
    fn into_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError>;
}

/// Enum for extended keys that can be either `xprv` or `xpub`
///
/// An instance of [`ExtendedKey`] can be constructed from an [`ExtendedPrivKey`](bip32::ExtendedPrivKey)
/// or an [`ExtendedPubKey`](bip32::ExtendedPubKey) by using the `From` trait.
///
/// Defaults to the [`Legacy`](miniscript::Legacy) context.
pub enum ExtendedKey<Ctx: ScriptContext = miniscript::Legacy> {
    /// A private extended key, aka an `xprv`
    Private((bip32::ExtendedPrivKey, PhantomData<Ctx>)),
    /// A public extended key, aka an `xpub`
    Public((bip32::ExtendedPubKey, PhantomData<Ctx>)),
}

impl<Ctx: ScriptContext> ExtendedKey<Ctx> {
    /// Return whether or not the key contains the private data
    pub fn has_secret(&self) -> bool {
        match self {
            ExtendedKey::Private(_) => true,
            ExtendedKey::Public(_) => false,
        }
    }

    /// Transform the [`ExtendedKey`] into an [`ExtendedPrivKey`](bip32::ExtendedPrivKey) for the
    /// given [`Network`], if the key contains the private data
    pub fn into_xprv(self, network: Network) -> Option<bip32::ExtendedPrivKey> {
        match self {
            ExtendedKey::Private((mut xprv, _)) => {
                xprv.network = network;
                Some(xprv)
            }
            ExtendedKey::Public(_) => None,
        }
    }

    /// Transform the [`ExtendedKey`] into an [`ExtendedPubKey`](bip32::ExtendedPubKey) for the
    /// given [`Network`]
    pub fn into_xpub<C: Signing>(
        self,
        network: bitcoin::Network,
        secp: &Secp256k1<C>,
    ) -> bip32::ExtendedPubKey {
        let mut xpub = match self {
            ExtendedKey::Private((xprv, _)) => bip32::ExtendedPubKey::from_private(secp, &xprv),
            ExtendedKey::Public((xpub, _)) => xpub,
        };

        xpub.network = network;
        xpub
    }
}

impl<Ctx: ScriptContext> From<bip32::ExtendedPubKey> for ExtendedKey<Ctx> {
    fn from(xpub: bip32::ExtendedPubKey) -> Self {
        ExtendedKey::Public((xpub, PhantomData))
    }
}

impl<Ctx: ScriptContext> From<bip32::ExtendedPrivKey> for ExtendedKey<Ctx> {
    fn from(xprv: bip32::ExtendedPrivKey) -> Self {
        ExtendedKey::Private((xprv, PhantomData))
    }
}

/// Trait for keys that can be derived.
///
/// When extra metadata are provided, a [`DerivableKey`] can be transofrmed into a
/// [`DescriptorKey`]: the trait [`IntoDescriptorKey`] is automatically implemented
/// for `(DerivableKey, DerivationPath)` and
/// `(DerivableKey, KeySource, DerivationPath)` tuples.
///
/// For key types that don't encode any indication about the path to use (like bip39), it's
/// generally recommended to implemented this trait instead of [`IntoDescriptorKey`]. The same
/// rules regarding script context and valid networks apply.
///
/// ## Examples
///
/// Key types that can be directly converted into an [`ExtendedPrivKey`] or
/// an [`ExtendedPubKey`] can implement only the required `into_extended_key()` method.
///
/// ```
/// use bdk::bitcoin;
/// use bdk::bitcoin::util::bip32;
/// use bdk::keys::{DerivableKey, ExtendedKey, KeyError, ScriptContext};
///
/// struct MyCustomKeyType {
///     key_data: bitcoin::PrivateKey,
///     chain_code: Vec<u8>,
///     network: bitcoin::Network,
/// }
///
/// impl<Ctx: ScriptContext> DerivableKey<Ctx> for MyCustomKeyType {
///     fn into_extended_key(self) -> Result<ExtendedKey<Ctx>, KeyError> {
///         let xprv = bip32::ExtendedPrivKey {
///             network: self.network,
///             depth: 0,
///             parent_fingerprint: bip32::Fingerprint::default(),
///             private_key: self.key_data,
///             chain_code: bip32::ChainCode::from(self.chain_code.as_ref()),
///             child_number: bip32::ChildNumber::Normal { index: 0 },
///         };
///
///         xprv.into_extended_key()
///     }
/// }
/// ```
///
/// Types that don't internally encode the [`Network`](bitcoin::Network) in which they are valid need some extra
/// steps to override the set of valid networks, otherwise only the network specified in the
/// [`ExtendedPrivKey`] or [`ExtendedPubKey`] will be considered valid.
///
/// ```
/// use bdk::bitcoin;
/// use bdk::bitcoin::util::bip32;
/// use bdk::keys::{
///     any_network, DerivableKey, DescriptorKey, ExtendedKey, KeyError, ScriptContext,
/// };
///
/// struct MyCustomKeyType {
///     key_data: bitcoin::PrivateKey,
///     chain_code: Vec<u8>,
/// }
///
/// impl<Ctx: ScriptContext> DerivableKey<Ctx> for MyCustomKeyType {
///     fn into_extended_key(self) -> Result<ExtendedKey<Ctx>, KeyError> {
///         let xprv = bip32::ExtendedPrivKey {
///             network: bitcoin::Network::Bitcoin, // pick an arbitrary network here
///             depth: 0,
///             parent_fingerprint: bip32::Fingerprint::default(),
///             private_key: self.key_data,
///             chain_code: bip32::ChainCode::from(self.chain_code.as_ref()),
///             child_number: bip32::ChildNumber::Normal { index: 0 },
///         };
///
///         xprv.into_extended_key()
///     }
///
///     fn into_descriptor_key(
///         self,
///         source: Option<bip32::KeySource>,
///         derivation_path: bip32::DerivationPath,
///     ) -> Result<DescriptorKey<Ctx>, KeyError> {
///         let descriptor_key = self
///             .into_extended_key()?
///             .into_descriptor_key(source, derivation_path)?;
///
///         // Override the set of valid networks here
///         Ok(descriptor_key.override_valid_networks(any_network()))
///     }
/// }
/// ```
///
/// [`DerivationPath`]: (bip32::DerivationPath)
/// [`ExtendedPrivKey`]: (bip32::ExtendedPrivKey)
/// [`ExtendedPubKey`]: (bip32::ExtendedPubKey)
pub trait DerivableKey<Ctx: ScriptContext = miniscript::Legacy>: Sized {
    /// Consume `self` and turn it into an [`ExtendedKey`]
    ///
    /// This can be used to get direct access to `xprv`s and `xpub`s for types that implement this trait,
    /// like [`Mnemonic`](bip39::Mnemonic) when the `keys-bip39` feature is enabled.
    #[cfg_attr(
        feature = "keys-bip39",
        doc = r##"
```rust
use bdk::bitcoin::Network;
use bdk::keys::{DerivableKey, ExtendedKey};
use bdk::keys::bip39::{Mnemonic, Language};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let xkey: ExtendedKey =
    Mnemonic::from_phrase(
        "jelly crash boy whisper mouse ecology tuna soccer memory million news short",
        Language::English
    )?
    .into_extended_key()?;
let xprv = xkey.into_xprv(Network::Bitcoin).unwrap();
# Ok(()) }
```
"##
    )]
    fn into_extended_key(self) -> Result<ExtendedKey<Ctx>, KeyError>;

    /// Consume `self` and turn it into a [`DescriptorKey`] by adding the extra metadata, such as
    /// key origin and derivation path
    fn into_descriptor_key(
        self,
        origin: Option<bip32::KeySource>,
        derivation_path: bip32::DerivationPath,
    ) -> Result<DescriptorKey<Ctx>, KeyError> {
        match self.into_extended_key()? {
            ExtendedKey::Private((xprv, _)) => DescriptorSecretKey::XPrv(DescriptorXKey {
                origin,
                xkey: xprv,
                derivation_path,
                wildcard: Wildcard::Unhardened,
            })
            .into_descriptor_key(),
            ExtendedKey::Public((xpub, _)) => DescriptorPublicKey::XPub(DescriptorXKey {
                origin,
                xkey: xpub,
                derivation_path,
                wildcard: Wildcard::Unhardened,
            })
            .into_descriptor_key(),
        }
    }
}

/// Identity conversion
impl<Ctx: ScriptContext> DerivableKey<Ctx> for ExtendedKey<Ctx> {
    fn into_extended_key(self) -> Result<ExtendedKey<Ctx>, KeyError> {
        Ok(self)
    }
}

impl<Ctx: ScriptContext> DerivableKey<Ctx> for bip32::ExtendedPubKey {
    fn into_extended_key(self) -> Result<ExtendedKey<Ctx>, KeyError> {
        Ok(self.into())
    }
}

impl<Ctx: ScriptContext> DerivableKey<Ctx> for bip32::ExtendedPrivKey {
    fn into_extended_key(self) -> Result<ExtendedKey<Ctx>, KeyError> {
        Ok(self.into())
    }
}

/// Output of a [`GeneratableKey`] key generation
pub struct GeneratedKey<K, Ctx: ScriptContext> {
    key: K,
    valid_networks: ValidNetworks,
    phantom: PhantomData<Ctx>,
}

impl<K, Ctx: ScriptContext> GeneratedKey<K, Ctx> {
    fn new(key: K, valid_networks: ValidNetworks) -> Self {
        GeneratedKey {
            key,
            valid_networks,
            phantom: PhantomData,
        }
    }

    /// Consumes `self` and returns the key
    pub fn into_key(self) -> K {
        self.key
    }
}

impl<K, Ctx: ScriptContext> Deref for GeneratedKey<K, Ctx> {
    type Target = K;

    fn deref(&self) -> &Self::Target {
        &self.key
    }
}

// Make generated "derivable" keys themselves "derivable". Also make sure they are assigned the
// right `valid_networks`.
impl<Ctx, K> DerivableKey<Ctx> for GeneratedKey<K, Ctx>
where
    Ctx: ScriptContext,
    K: DerivableKey<Ctx>,
{
    fn into_extended_key(self) -> Result<ExtendedKey<Ctx>, KeyError> {
        self.key.into_extended_key()
    }

    fn into_descriptor_key(
        self,
        origin: Option<bip32::KeySource>,
        derivation_path: bip32::DerivationPath,
    ) -> Result<DescriptorKey<Ctx>, KeyError> {
        let descriptor_key = self.key.into_descriptor_key(origin, derivation_path)?;
        Ok(descriptor_key.override_valid_networks(self.valid_networks))
    }
}

// Make generated keys directly usable in descriptors, and make sure they get assigned the right
// `valid_networks`.
impl<Ctx, K> IntoDescriptorKey<Ctx> for GeneratedKey<K, Ctx>
where
    Ctx: ScriptContext,
    K: IntoDescriptorKey<Ctx>,
{
    fn into_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
        let desc_key = self.key.into_descriptor_key()?;
        Ok(desc_key.override_valid_networks(self.valid_networks))
    }
}

/// Trait for keys that can be generated
///
/// The same rules about [`ScriptContext`] and [`ValidNetworks`] from [`IntoDescriptorKey`] apply.
///
/// This trait is particularly useful when combined with [`DerivableKey`]: if `Self`
/// implements it, the returned [`GeneratedKey`] will also implement it. The same is true for
/// [`IntoDescriptorKey`]: the generated keys can be directly used in descriptors if `Self` is also
/// [`IntoDescriptorKey`].
pub trait GeneratableKey<Ctx: ScriptContext>: Sized {
    /// Type specifying the amount of entropy required e.g. `[u8;32]`
    type Entropy: AsMut<[u8]> + Default;

    /// Extra options required by the `generate_with_entropy`
    type Options;
    /// Returned error in case of failure
    type Error: std::fmt::Debug;

    /// Generate a key given the extra options and the entropy
    fn generate_with_entropy(
        options: Self::Options,
        entropy: Self::Entropy,
    ) -> Result<GeneratedKey<Self, Ctx>, Self::Error>;

    /// Generate a key given the options with a random entropy
    fn generate(options: Self::Options) -> Result<GeneratedKey<Self, Ctx>, Self::Error> {
        use rand::{thread_rng, Rng};

        let mut entropy = Self::Entropy::default();
        thread_rng().fill(entropy.as_mut());
        Self::generate_with_entropy(options, entropy)
    }
}

/// Trait that allows generating a key with the default options
///
/// This trait is automatically implemented if the [`GeneratableKey::Options`] implements [`Default`].
pub trait GeneratableDefaultOptions<Ctx>: GeneratableKey<Ctx>
where
    Ctx: ScriptContext,
    <Self as GeneratableKey<Ctx>>::Options: Default,
{
    /// Generate a key with the default options and a given entropy
    fn generate_with_entropy_default(
        entropy: Self::Entropy,
    ) -> Result<GeneratedKey<Self, Ctx>, Self::Error> {
        Self::generate_with_entropy(Default::default(), entropy)
    }

    /// Generate a key with the default options and a random entropy
    fn generate_default() -> Result<GeneratedKey<Self, Ctx>, Self::Error> {
        Self::generate(Default::default())
    }
}

/// Automatic implementation of [`GeneratableDefaultOptions`] for [`GeneratableKey`]s where
/// `Options` implements `Default`
impl<Ctx, K> GeneratableDefaultOptions<Ctx> for K
where
    Ctx: ScriptContext,
    K: GeneratableKey<Ctx>,
    <K as GeneratableKey<Ctx>>::Options: Default,
{
}

impl<Ctx: ScriptContext> GeneratableKey<Ctx> for bip32::ExtendedPrivKey {
    type Entropy = [u8; 32];

    type Options = ();
    type Error = bip32::Error;

    fn generate_with_entropy(
        _: Self::Options,
        entropy: Self::Entropy,
    ) -> Result<GeneratedKey<Self, Ctx>, Self::Error> {
        // pick a arbitrary network here, but say that we support all of them
        let xprv = bip32::ExtendedPrivKey::new_master(Network::Bitcoin, entropy.as_ref())?;
        Ok(GeneratedKey::new(xprv, any_network()))
    }
}

/// Options for generating a [`PrivateKey`]
///
/// Defaults to creating compressed keys, which save on-chain bytes and fees
#[derive(Debug, Copy, Clone)]
pub struct PrivateKeyGenerateOptions {
    /// Whether the generated key should be "compressed" or not
    pub compressed: bool,
}

impl Default for PrivateKeyGenerateOptions {
    fn default() -> Self {
        PrivateKeyGenerateOptions { compressed: true }
    }
}

impl<Ctx: ScriptContext> GeneratableKey<Ctx> for PrivateKey {
    type Entropy = [u8; secp256k1::constants::SECRET_KEY_SIZE];

    type Options = PrivateKeyGenerateOptions;
    type Error = bip32::Error;

    fn generate_with_entropy(
        options: Self::Options,
        entropy: Self::Entropy,
    ) -> Result<GeneratedKey<Self, Ctx>, Self::Error> {
        // pick a arbitrary network here, but say that we support all of them
        let key = secp256k1::SecretKey::from_slice(&entropy)?;
        let private_key = PrivateKey {
            compressed: options.compressed,
            network: Network::Bitcoin,
            key,
        };

        Ok(GeneratedKey::new(private_key, any_network()))
    }
}

impl<Ctx: ScriptContext, T: DerivableKey<Ctx>> IntoDescriptorKey<Ctx>
    for (T, bip32::DerivationPath)
{
    fn into_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
        self.0.into_descriptor_key(None, self.1)
    }
}

impl<Ctx: ScriptContext, T: DerivableKey<Ctx>> IntoDescriptorKey<Ctx>
    for (T, bip32::KeySource, bip32::DerivationPath)
{
    fn into_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
        self.0.into_descriptor_key(Some(self.1), self.2)
    }
}

fn expand_multi_keys<Pk: IntoDescriptorKey<Ctx>, Ctx: ScriptContext>(
    pks: Vec<Pk>,
    secp: &SecpCtx,
) -> Result<(Vec<DescriptorPublicKey>, KeyMap, ValidNetworks), KeyError> {
    let (pks, key_maps_networks): (Vec<_>, Vec<_>) = pks
        .into_iter()
        .map(|key| Ok::<_, KeyError>(key.into_descriptor_key()?.extract(secp)?))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(a, b, c)| (a, (b, c)))
        .unzip();

    let (key_map, valid_networks) = key_maps_networks.into_iter().fold(
        (KeyMap::default(), any_network()),
        |(mut keys_acc, net_acc), (key, net)| {
            keys_acc.extend(key.into_iter());
            let net_acc = merge_networks(&net_acc, &net);

            (keys_acc, net_acc)
        },
    );

    Ok((pks, key_map, valid_networks))
}

// Used internally by `bdk::fragment!` to build `pk_k()` fragments
#[doc(hidden)]
pub fn make_pk<Pk: IntoDescriptorKey<Ctx>, Ctx: ScriptContext>(
    descriptor_key: Pk,
    secp: &SecpCtx,
) -> Result<(Miniscript<DescriptorPublicKey, Ctx>, KeyMap, ValidNetworks), DescriptorError> {
    let (key, key_map, valid_networks) = descriptor_key.into_descriptor_key()?.extract(secp)?;
    let minisc = Miniscript::from_ast(Terminal::PkK(key))?;

    minisc.check_minsicript()?;

    Ok((minisc, key_map, valid_networks))
}

// Used internally by `bdk::fragment!` to build `multi()` fragments
#[doc(hidden)]
pub fn make_multi<Pk: IntoDescriptorKey<Ctx>, Ctx: ScriptContext>(
    thresh: usize,
    pks: Vec<Pk>,
    secp: &SecpCtx,
) -> Result<(Miniscript<DescriptorPublicKey, Ctx>, KeyMap, ValidNetworks), DescriptorError> {
    let (pks, key_map, valid_networks) = expand_multi_keys(pks, secp)?;
    let minisc = Miniscript::from_ast(Terminal::Multi(thresh, pks))?;

    minisc.check_minsicript()?;

    Ok((minisc, key_map, valid_networks))
}

// Used internally by `bdk::descriptor!` to build `sortedmulti()` fragments
#[doc(hidden)]
pub fn make_sortedmulti<Pk, Ctx, F>(
    thresh: usize,
    pks: Vec<Pk>,
    build_desc: F,
    secp: &SecpCtx,
) -> Result<(Descriptor<DescriptorPublicKey>, KeyMap, ValidNetworks), DescriptorError>
where
    Pk: IntoDescriptorKey<Ctx>,
    Ctx: ScriptContext,
    F: Fn(
        usize,
        Vec<DescriptorPublicKey>,
    ) -> Result<(Descriptor<DescriptorPublicKey>, PhantomData<Ctx>), DescriptorError>,
{
    let (pks, key_map, valid_networks) = expand_multi_keys(pks, secp)?;
    let descriptor = build_desc(thresh, pks)?.0;

    Ok((descriptor, key_map, valid_networks))
}

/// The "identity" conversion is used internally by some `bdk::fragment`s
impl<Ctx: ScriptContext> IntoDescriptorKey<Ctx> for DescriptorKey<Ctx> {
    fn into_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
        Ok(self)
    }
}

impl<Ctx: ScriptContext> IntoDescriptorKey<Ctx> for DescriptorPublicKey {
    fn into_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
        let networks = match self {
            DescriptorPublicKey::SinglePub(_) => any_network(),
            DescriptorPublicKey::XPub(DescriptorXKey { xkey, .. })
                if xkey.network == Network::Bitcoin =>
            {
                mainnet_network()
            }
            _ => test_networks(),
        };

        Ok(DescriptorKey::from_public(self, networks))
    }
}

impl<Ctx: ScriptContext> IntoDescriptorKey<Ctx> for PublicKey {
    fn into_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
        DescriptorPublicKey::SinglePub(DescriptorSinglePub {
            key: self,
            origin: None,
        })
        .into_descriptor_key()
    }
}

impl<Ctx: ScriptContext> IntoDescriptorKey<Ctx> for DescriptorSecretKey {
    fn into_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
        let networks = match &self {
            DescriptorSecretKey::SinglePriv(sk) if sk.key.network == Network::Bitcoin => {
                mainnet_network()
            }
            DescriptorSecretKey::XPrv(DescriptorXKey { xkey, .. })
                if xkey.network == Network::Bitcoin =>
            {
                mainnet_network()
            }
            _ => test_networks(),
        };

        Ok(DescriptorKey::from_secret(self, networks))
    }
}

impl<Ctx: ScriptContext> IntoDescriptorKey<Ctx> for &'_ str {
    fn into_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
        DescriptorSecretKey::from_str(self)
            .map_err(|e| KeyError::Message(e.to_string()))?
            .into_descriptor_key()
    }
}

impl<Ctx: ScriptContext> IntoDescriptorKey<Ctx> for PrivateKey {
    fn into_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
        DescriptorSecretKey::SinglePriv(DescriptorSinglePriv {
            key: self,
            origin: None,
        })
        .into_descriptor_key()
    }
}

/// Errors thrown while working with [`keys`](crate::keys)
#[derive(Debug)]
pub enum KeyError {
    /// The key cannot exist in the given script context
    InvalidScriptContext,
    /// The key is not valid for the given network
    InvalidNetwork,
    /// The key has an invalid checksum
    InvalidChecksum,

    /// Custom error message
    Message(String),

    /// BIP32 error
    BIP32(bitcoin::util::bip32::Error),
    /// Miniscript error
    Miniscript(miniscript::Error),
}

impl_error!(miniscript::Error, Miniscript, KeyError);
impl_error!(bitcoin::util::bip32::Error, BIP32, KeyError);

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for KeyError {}

#[cfg(test)]
pub mod test {
    use bitcoin::util::bip32;

    use super::*;

    pub const TEST_ENTROPY: [u8; 32] = [0xAA; 32];

    #[test]
    fn test_keys_generate_xprv() {
        let generated_xprv: GeneratedKey<_, miniscript::Segwitv0> =
            bip32::ExtendedPrivKey::generate_with_entropy_default(TEST_ENTROPY).unwrap();

        assert_eq!(generated_xprv.valid_networks, any_network());
        assert_eq!(generated_xprv.to_string(), "xprv9s21ZrQH143K4Xr1cJyqTvuL2FWR8eicgY9boWqMBv8MDVUZ65AXHnzBrK1nyomu6wdcabRgmGTaAKawvhAno1V5FowGpTLVx3jxzE5uk3Q");
    }

    #[test]
    fn test_keys_generate_wif() {
        let generated_wif: GeneratedKey<_, miniscript::Segwitv0> =
            bitcoin::PrivateKey::generate_with_entropy_default(TEST_ENTROPY).unwrap();

        assert_eq!(generated_wif.valid_networks, any_network());
        assert_eq!(
            generated_wif.to_string(),
            "L2wTu6hQrnDMiFNWA5na6jB12ErGQqtXwqpSL7aWquJaZG8Ai3ch"
        );
    }
}
