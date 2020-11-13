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

use bitcoin::secp256k1;

use bitcoin::util::bip32;
use bitcoin::{Network, PrivateKey, PublicKey};

pub use miniscript::descriptor::{
    DescriptorPublicKey, DescriptorSecretKey, DescriptorSinglePriv, DescriptorSinglePub,
};
use miniscript::descriptor::{DescriptorXKey, KeyMap};
pub use miniscript::ScriptContext;
use miniscript::{Miniscript, Terminal};

#[cfg(feature = "keys-bip39")]
#[cfg_attr(docsrs, doc(cfg(feature = "keys-bip39")))]
pub mod bip39;

/// Set of valid networks for a key
pub type ValidNetworks = HashSet<Network>;

/// Create a set containing mainnet, testnet and regtest
pub fn any_network() -> ValidNetworks {
    vec![Network::Bitcoin, Network::Testnet, Network::Regtest]
        .into_iter()
        .collect()
}
/// Create a set only containing mainnet
pub fn mainnet_network() -> ValidNetworks {
    vec![Network::Bitcoin].into_iter().collect()
}
/// Create a set containing testnet and regtest
pub fn test_networks() -> ValidNetworks {
    vec![Network::Testnet, Network::Regtest]
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
    pub fn extract(self) -> Result<(DescriptorPublicKey, KeyMap, ValidNetworks), KeyError> {
        match self {
            DescriptorKey::Public(public, valid_networks, _) => {
                Ok((public, KeyMap::default(), valid_networks))
            }
            DescriptorKey::Secret(secret, valid_networks, _) => {
                let mut key_map = KeyMap::with_capacity(1);

                let public = secret
                    .as_public()
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

/// Trait that adds extra useful methods to [`ScriptContext`]s
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
/// use bdk::keys::{ScriptContext, ToDescriptorKey, DescriptorKey, KeyError};
///
/// pub struct MyKeyType {
///     pubkey: PublicKey,
/// }
///
/// impl<Ctx: ScriptContext> ToDescriptorKey<Ctx> for MyKeyType {
///     fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
///         self.pubkey.to_descriptor_key()
///     }
/// }
/// ```
///
/// Key type that is only valid on mainnet:
///
/// ```
/// use bdk::bitcoin::PublicKey;
///
/// use bdk::keys::{mainnet_network, ScriptContext, ToDescriptorKey, DescriptorKey, DescriptorPublicKey, DescriptorSinglePub, KeyError};
///
/// pub struct MyKeyType {
///     pubkey: PublicKey,
/// }
///
/// impl<Ctx: ScriptContext> ToDescriptorKey<Ctx> for MyKeyType {
///     fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
///         Ok(DescriptorKey::from_public(DescriptorPublicKey::SinglePub(DescriptorSinglePub {
///             origin: None,
///             key: self.pubkey
///         }), mainnet_network()))
///     }
/// }
/// ```
///
/// Key type that internally encodes in which context it's valid. The context is checked at runtime:
///
/// ```
/// use bdk::bitcoin::PublicKey;
///
/// use bdk::keys::{ExtScriptContext, ScriptContext, ToDescriptorKey, DescriptorKey, KeyError};
///
/// pub struct MyKeyType {
///     is_legacy: bool,
///     pubkey: PublicKey,
/// }
///
/// impl<Ctx: ScriptContext + 'static> ToDescriptorKey<Ctx> for MyKeyType {
///     fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
///         if Ctx::is_legacy() == self.is_legacy {
///             self.pubkey.to_descriptor_key()
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
/// use std::str::FromStr;
/// use bdk::bitcoin::PublicKey;
///
/// use bdk::keys::{ToDescriptorKey, DescriptorKey, KeyError};
///
/// pub struct MySegwitOnlyKeyType {
///     pubkey: PublicKey,
/// }
///
/// impl ToDescriptorKey<bdk::miniscript::Segwitv0> for MySegwitOnlyKeyType {
///     fn to_descriptor_key(self) -> Result<DescriptorKey<bdk::miniscript::Segwitv0>, KeyError> {
///         self.pubkey.to_descriptor_key()
///     }
/// }
///
/// let key = MySegwitOnlyKeyType {
///     pubkey: PublicKey::from_str("...")?,
/// };
/// let (descriptor, _, _) = bdk::descriptor!(pkh ( key ) )?;
/// //                                       ^^^^^ changing this to `wpkh` would make it compile
///
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
pub trait ToDescriptorKey<Ctx: ScriptContext>: Sized {
    /// Turn the key into a [`DescriptorKey`] within the requested [`ScriptContext`]
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError>;
}

/// Trait for keys that can be derived.
///
/// When extra metadata are provided, a [`DerivableKey`] can be transofrmed into a
/// [`DescriptorKey`]: the trait [`ToDescriptorKey`] is automatically implemented
/// for `(DerivableKey, DerivationPath)` and
/// `(DerivableKey, (Fingerprint, DerivationPath), DerivationPath)` tuples.
///
/// For key types that don't encode any indication about the path to use (like bip39), it's
/// generally recommended to implemented this trait instead of [`ToDescriptorKey`]. The same
/// rules regarding script context and valid networks apply.
///
/// [`DerivationPath`]: (bip32::DerivationPath)
pub trait DerivableKey<Ctx: ScriptContext> {
    /// Add a extra metadata, consume `self` and turn it into a [`DescriptorKey`]
    fn add_metadata(
        self,
        origin: Option<(bip32::Fingerprint, bip32::DerivationPath)>,
        derivation_path: bip32::DerivationPath,
    ) -> Result<DescriptorKey<Ctx>, KeyError>;
}

impl<Ctx: ScriptContext> DerivableKey<Ctx> for bip32::ExtendedPubKey {
    fn add_metadata(
        self,
        origin: Option<(bip32::Fingerprint, bip32::DerivationPath)>,
        derivation_path: bip32::DerivationPath,
    ) -> Result<DescriptorKey<Ctx>, KeyError> {
        DescriptorPublicKey::XPub(DescriptorXKey {
            origin,
            xkey: self,
            derivation_path,
            is_wildcard: true,
        })
        .to_descriptor_key()
    }
}

impl<Ctx: ScriptContext> DerivableKey<Ctx> for bip32::ExtendedPrivKey {
    fn add_metadata(
        self,
        origin: Option<(bip32::Fingerprint, bip32::DerivationPath)>,
        derivation_path: bip32::DerivationPath,
    ) -> Result<DescriptorKey<Ctx>, KeyError> {
        DescriptorSecretKey::XPrv(DescriptorXKey {
            origin,
            xkey: self,
            derivation_path,
            is_wildcard: true,
        })
        .to_descriptor_key()
    }
}

/// Output of a [`GeneratableKey`] key generation
pub struct GeneratedKey<K, Ctx: ScriptContext> {
    key: K,
    valid_networks: ValidNetworks,
    phantom: PhantomData<Ctx>,
}

impl<K, Ctx: ScriptContext> GeneratedKey<K, Ctx> {
    pub fn new(key: K, valid_networks: ValidNetworks) -> Self {
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
    fn add_metadata(
        self,
        origin: Option<(bip32::Fingerprint, bip32::DerivationPath)>,
        derivation_path: bip32::DerivationPath,
    ) -> Result<DescriptorKey<Ctx>, KeyError> {
        let descriptor_key = self.key.add_metadata(origin, derivation_path)?;
        Ok(descriptor_key.override_valid_networks(self.valid_networks))
    }
}

// Make generated keys directly usable in descriptors, and make sure they get assigned the right
// `valid_networks`.
impl<Ctx, K> ToDescriptorKey<Ctx> for GeneratedKey<K, Ctx>
where
    Ctx: ScriptContext,
    K: ToDescriptorKey<Ctx>,
{
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
        let desc_key = self.key.to_descriptor_key()?;
        Ok(desc_key.override_valid_networks(self.valid_networks))
    }
}

/// Trait for keys that can be generated
///
/// The same rules about [`ScriptContext`] and [`ValidNetworks`] from [`ToDescriptorKey`] apply.
///
/// This trait is particularly useful when combined with [`DerivableKey`]: if `Self`
/// implements it, the returned [`GeneratedKey`] will also implement it. The same is true for
/// [`ToDescriptorKey`]: the generated keys can be directly used in descriptors if `Self` is also
/// [`ToDescriptorKey`].
pub trait GeneratableKey<Ctx: ScriptContext>: Sized {
    /// Type specifying the amount of entropy required e.g. [u8;32]
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

impl<Ctx: ScriptContext, T: DerivableKey<Ctx>> ToDescriptorKey<Ctx> for (T, bip32::DerivationPath) {
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
        self.0.add_metadata(None, self.1)
    }
}

impl<Ctx: ScriptContext, T: DerivableKey<Ctx>> ToDescriptorKey<Ctx>
    for (
        T,
        (bip32::Fingerprint, bip32::DerivationPath),
        bip32::DerivationPath,
    )
{
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
        self.0.add_metadata(Some(self.1), self.2)
    }
}

// Used internally by `bdk::fragment!` to build `pk_k()` fragments
#[doc(hidden)]
pub fn make_pk<Pk: ToDescriptorKey<Ctx>, Ctx: ScriptContext>(
    descriptor_key: Pk,
) -> Result<(Miniscript<DescriptorPublicKey, Ctx>, KeyMap, ValidNetworks), KeyError> {
    let (key, key_map, valid_networks) = descriptor_key.to_descriptor_key()?.extract()?;

    Ok((
        Miniscript::from_ast(Terminal::PkK(key))?,
        key_map,
        valid_networks,
    ))
}

// Used internally by `bdk::fragment!` to build `multi()` fragments
#[doc(hidden)]
pub fn make_multi<Pk: ToDescriptorKey<Ctx>, Ctx: ScriptContext>(
    thresh: usize,
    pks: Vec<Pk>,
) -> Result<(Miniscript<DescriptorPublicKey, Ctx>, KeyMap, ValidNetworks), KeyError> {
    let (pks, key_maps_networks): (Vec<_>, Vec<_>) = pks
        .into_iter()
        .map(|key| Ok::<_, KeyError>(key.to_descriptor_key()?.extract()?))
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

    Ok((
        Miniscript::from_ast(Terminal::Multi(thresh, pks))?,
        key_map,
        valid_networks,
    ))
}

/// The "identity" conversion is used internally by some `bdk::fragment`s
impl<Ctx: ScriptContext> ToDescriptorKey<Ctx> for DescriptorKey<Ctx> {
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
        Ok(self)
    }
}

impl<Ctx: ScriptContext> ToDescriptorKey<Ctx> for DescriptorPublicKey {
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
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

impl<Ctx: ScriptContext> ToDescriptorKey<Ctx> for PublicKey {
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
        DescriptorPublicKey::SinglePub(DescriptorSinglePub {
            key: self,
            origin: None,
        })
        .to_descriptor_key()
    }
}

impl<Ctx: ScriptContext> ToDescriptorKey<Ctx> for DescriptorSecretKey {
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
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

impl<Ctx: ScriptContext> ToDescriptorKey<Ctx> for PrivateKey {
    fn to_descriptor_key(self) -> Result<DescriptorKey<Ctx>, KeyError> {
        DescriptorSecretKey::SinglePriv(DescriptorSinglePriv {
            key: self,
            origin: None,
        })
        .to_descriptor_key()
    }
}

/// Errors thrown while working with [`keys`](crate::keys)
#[derive(Debug)]
pub enum KeyError {
    InvalidScriptContext,
    InvalidNetwork,
    InvalidChecksum,
    Message(String),

    BIP32(bitcoin::util::bip32::Error),
    Miniscript(miniscript::Error),
}

impl From<miniscript::Error> for KeyError {
    fn from(inner: miniscript::Error) -> Self {
        KeyError::Miniscript(inner)
    }
}

impl From<bitcoin::util::bip32::Error> for KeyError {
    fn from(inner: bitcoin::util::bip32::Error) -> Self {
        KeyError::BIP32(inner)
    }
}

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
