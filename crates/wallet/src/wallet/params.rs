use alloc::boxed::Box;
use bdk_chain::keychain_txout::DEFAULT_LOOKAHEAD;
use bitcoin::{BlockHash, Network};
use miniscript::descriptor::KeyMap;

use crate::{
    descriptor::{DescriptorError, ExtendedDescriptor, IntoWalletDescriptor},
    utils::SecpCtx,
    AsyncWalletPersister, CreateWithPersistError, KeychainKind, LoadWithPersistError, Wallet,
    WalletPersister,
};

use super::{ChangeSet, LoadError, PersistedWallet};

/// This atrocity is to avoid having type parameters on [`CreateParams`] and [`LoadParams`].
///
/// The better option would be to do `Box<dyn IntoWalletDescriptor>`, but we cannot due to Rust's
/// [object safety rules](https://doc.rust-lang.org/reference/items/traits.html#object-safety).
type DescriptorToExtract = Box<
    dyn FnOnce(&SecpCtx, Network) -> Result<(ExtendedDescriptor, KeyMap), DescriptorError>
        + 'static,
>;

fn make_descriptor_to_extract<D>(descriptor: D) -> DescriptorToExtract
where
    D: IntoWalletDescriptor + 'static,
{
    Box::new(|secp, network| descriptor.into_wallet_descriptor(secp, network))
}

/// Parameters for [`Wallet::create`] or [`PersistedWallet::create`].
#[must_use]
pub struct CreateParams {
    pub(crate) descriptor: DescriptorToExtract,
    pub(crate) descriptor_keymap: KeyMap,
    pub(crate) change_descriptor: Option<DescriptorToExtract>,
    pub(crate) change_descriptor_keymap: KeyMap,
    pub(crate) network: Network,
    pub(crate) genesis_hash: Option<BlockHash>,
    pub(crate) lookahead: u32,
}

impl CreateParams {
    /// Construct parameters with provided `descriptor`.
    ///
    /// Default values:
    /// * `change_descriptor` = `None`
    /// * `network` = [`Network::Bitcoin`]
    /// * `genesis_hash` = `None`
    /// * `lookahead` = [`DEFAULT_LOOKAHEAD`]
    ///
    /// Use this method only when building a wallet with a single descriptor. See
    /// also [`Wallet::create_single`].
    pub fn new_single<D: IntoWalletDescriptor + 'static>(descriptor: D) -> Self {
        Self {
            descriptor: make_descriptor_to_extract(descriptor),
            descriptor_keymap: KeyMap::default(),
            change_descriptor: None,
            change_descriptor_keymap: KeyMap::default(),
            network: Network::Bitcoin,
            genesis_hash: None,
            lookahead: DEFAULT_LOOKAHEAD,
        }
    }

    /// Construct parameters with provided `descriptor` and `change_descriptor`.
    ///
    /// Default values:
    /// * `network` = [`Network::Bitcoin`]
    /// * `genesis_hash` = `None`
    /// * `lookahead` = [`DEFAULT_LOOKAHEAD`]
    pub fn new<D: IntoWalletDescriptor + 'static>(descriptor: D, change_descriptor: D) -> Self {
        Self {
            descriptor: make_descriptor_to_extract(descriptor),
            descriptor_keymap: KeyMap::default(),
            change_descriptor: Some(make_descriptor_to_extract(change_descriptor)),
            change_descriptor_keymap: KeyMap::default(),
            network: Network::Bitcoin,
            genesis_hash: None,
            lookahead: DEFAULT_LOOKAHEAD,
        }
    }

    /// Extend the given `keychain`'s `keymap`.
    pub fn keymap(mut self, keychain: KeychainKind, keymap: KeyMap) -> Self {
        match keychain {
            KeychainKind::External => &mut self.descriptor_keymap,
            KeychainKind::Internal => &mut self.change_descriptor_keymap,
        }
        .extend(keymap);
        self
    }

    /// Set `network`.
    pub fn network(mut self, network: Network) -> Self {
        self.network = network;
        self
    }

    /// Use a custom `genesis_hash`.
    pub fn genesis_hash(mut self, genesis_hash: BlockHash) -> Self {
        self.genesis_hash = Some(genesis_hash);
        self
    }

    /// Use custom lookahead value.
    pub fn lookahead(mut self, lookahead: u32) -> Self {
        self.lookahead = lookahead;
        self
    }

    /// Create [`PersistedWallet`] with the given [`WalletPersister`].
    pub fn create_wallet<P>(
        self,
        persister: &mut P,
    ) -> Result<PersistedWallet<P>, CreateWithPersistError<P::Error>>
    where
        P: WalletPersister,
    {
        PersistedWallet::create(persister, self)
    }

    /// Create [`PersistedWallet`] with the given [`AsyncWalletPersister`].
    pub async fn create_wallet_async<P>(
        self,
        persister: &mut P,
    ) -> Result<PersistedWallet<P>, CreateWithPersistError<P::Error>>
    where
        P: AsyncWalletPersister,
    {
        PersistedWallet::create_async(persister, self).await
    }

    /// Create [`Wallet`] without persistence.
    pub fn create_wallet_no_persist(self) -> Result<Wallet, DescriptorError> {
        Wallet::create_with_params(self)
    }
}

/// Parameters for [`Wallet::load`] or [`PersistedWallet::load`].
#[must_use]
pub struct LoadParams {
    pub(crate) descriptor_keymap: KeyMap,
    pub(crate) change_descriptor_keymap: KeyMap,
    pub(crate) lookahead: u32,
    pub(crate) check_network: Option<Network>,
    pub(crate) check_genesis_hash: Option<BlockHash>,
    pub(crate) check_descriptor: Option<Option<DescriptorToExtract>>,
    pub(crate) check_change_descriptor: Option<Option<DescriptorToExtract>>,
    pub(crate) extract_keys: bool,
}

impl LoadParams {
    /// Construct parameters with default values.
    ///
    /// Default values: `lookahead` = [`DEFAULT_LOOKAHEAD`]
    pub fn new() -> Self {
        Self {
            descriptor_keymap: KeyMap::default(),
            change_descriptor_keymap: KeyMap::default(),
            lookahead: DEFAULT_LOOKAHEAD,
            check_network: None,
            check_genesis_hash: None,
            check_descriptor: None,
            check_change_descriptor: None,
            extract_keys: false,
        }
    }

    /// Extend the given `keychain`'s `keymap`.
    pub fn keymap(mut self, keychain: KeychainKind, keymap: KeyMap) -> Self {
        match keychain {
            KeychainKind::External => &mut self.descriptor_keymap,
            KeychainKind::Internal => &mut self.change_descriptor_keymap,
        }
        .extend(keymap);
        self
    }

    /// Checks the `expected_descriptor` matches exactly what is loaded for `keychain`.
    ///
    /// # Note
    ///
    /// You must also specify [`extract_keys`](Self::extract_keys) if you wish to add a signer
    /// for an expected descriptor containing secrets.
    pub fn descriptor<D>(mut self, keychain: KeychainKind, expected_descriptor: Option<D>) -> Self
    where
        D: IntoWalletDescriptor + 'static,
    {
        let expected = expected_descriptor.map(|d| make_descriptor_to_extract(d));
        match keychain {
            KeychainKind::External => self.check_descriptor = Some(expected),
            KeychainKind::Internal => self.check_change_descriptor = Some(expected),
        }
        self
    }

    /// Checks that the given network matches the one loaded from persistence.
    pub fn check_network(mut self, network: Network) -> Self {
        self.check_network = Some(network);
        self
    }

    /// Checks that the given `genesis_hash` matches the one loaded from persistence.
    pub fn check_genesis_hash(mut self, genesis_hash: BlockHash) -> Self {
        self.check_genesis_hash = Some(genesis_hash);
        self
    }

    /// Use custom lookahead value.
    pub fn lookahead(mut self, lookahead: u32) -> Self {
        self.lookahead = lookahead;
        self
    }

    /// Whether to try extracting private keys from the *provided descriptors* upon loading.
    /// See also [`LoadParams::descriptor`].
    pub fn extract_keys(mut self) -> Self {
        self.extract_keys = true;
        self
    }

    /// Load [`PersistedWallet`] with the given [`WalletPersister`].
    pub fn load_wallet<P>(
        self,
        persister: &mut P,
    ) -> Result<Option<PersistedWallet<P>>, LoadWithPersistError<P::Error>>
    where
        P: WalletPersister,
    {
        PersistedWallet::load(persister, self)
    }

    /// Load [`PersistedWallet`] with the given [`AsyncWalletPersister`].
    pub async fn load_wallet_async<P>(
        self,
        persister: &mut P,
    ) -> Result<Option<PersistedWallet<P>>, LoadWithPersistError<P::Error>>
    where
        P: AsyncWalletPersister,
    {
        PersistedWallet::load_async(persister, self).await
    }

    /// Load [`Wallet`] without persistence.
    pub fn load_wallet_no_persist(self, changeset: ChangeSet) -> Result<Option<Wallet>, LoadError> {
        Wallet::load_with_params(changeset, self)
    }
}

impl Default for LoadParams {
    fn default() -> Self {
        Self::new()
    }
}
