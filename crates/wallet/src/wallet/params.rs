use alloc::boxed::Box;
use bdk_chain::{keychain_txout::DEFAULT_LOOKAHEAD, PersistAsyncWith, PersistWith};
use bitcoin::{BlockHash, Network};
use miniscript::descriptor::KeyMap;

use crate::{
    descriptor::{DescriptorError, ExtendedDescriptor, IntoWalletDescriptor},
    utils::SecpCtx,
    KeychainKind, Wallet,
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
    pub(crate) change_descriptor: DescriptorToExtract,
    pub(crate) change_descriptor_keymap: KeyMap,
    pub(crate) network: Network,
    pub(crate) genesis_hash: Option<BlockHash>,
    pub(crate) lookahead: u32,
}

impl CreateParams {
    /// Construct parameters with provided `descriptor`, `change_descriptor` and `network`.
    ///
    /// Default values: `genesis_hash` = `None`, `lookahead` = [`DEFAULT_LOOKAHEAD`]
    pub fn new<D: IntoWalletDescriptor + 'static>(descriptor: D, change_descriptor: D) -> Self {
        Self {
            descriptor: make_descriptor_to_extract(descriptor),
            descriptor_keymap: KeyMap::default(),
            change_descriptor: make_descriptor_to_extract(change_descriptor),
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

    /// Create [`PersistedWallet`] with the given `Db`.
    pub fn create_wallet<Db>(
        self,
        db: &mut Db,
    ) -> Result<PersistedWallet, <Wallet as PersistWith<Db>>::CreateError>
    where
        Wallet: PersistWith<Db, CreateParams = Self>,
    {
        PersistedWallet::create(db, self)
    }

    /// Create [`PersistedWallet`] with the given async `Db`.
    pub async fn create_wallet_async<Db>(
        self,
        db: &mut Db,
    ) -> Result<PersistedWallet, <Wallet as PersistAsyncWith<Db>>::CreateError>
    where
        Wallet: PersistAsyncWith<Db, CreateParams = Self>,
    {
        PersistedWallet::create_async(db, self).await
    }

    /// Create [`Wallet`] without persistence.
    pub fn create_wallet_no_persist(self) -> Result<Wallet, DescriptorError> {
        Wallet::create_with_params(self)
    }
}

unsafe impl Send for CreateParams {}
unsafe impl Send for LoadParams {}

/// Parameters for [`Wallet::load`] or [`PersistedWallet::load`].
#[must_use]
pub struct LoadParams {
    pub(crate) descriptor_keymap: KeyMap,
    pub(crate) change_descriptor_keymap: KeyMap,
    pub(crate) lookahead: u32,
    pub(crate) check_network: Option<Network>,
    pub(crate) check_genesis_hash: Option<BlockHash>,
    pub(crate) check_descriptor: Option<DescriptorToExtract>,
    pub(crate) check_change_descriptor: Option<DescriptorToExtract>,
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

    /// Checks that `descriptor` of `keychain` matches this, and extracts private keys (if
    /// available).
    pub fn descriptors<D>(mut self, descriptor: D, change_descriptor: D) -> Self
    where
        D: IntoWalletDescriptor + 'static,
    {
        self.check_descriptor = Some(make_descriptor_to_extract(descriptor));
        self.check_change_descriptor = Some(make_descriptor_to_extract(change_descriptor));
        self
    }

    /// Check for `network`.
    pub fn network(mut self, network: Network) -> Self {
        self.check_network = Some(network);
        self
    }

    /// Check for a `genesis_hash`.
    pub fn genesis_hash(mut self, genesis_hash: BlockHash) -> Self {
        self.check_genesis_hash = Some(genesis_hash);
        self
    }

    /// Use custom lookahead value.
    pub fn lookahead(mut self, lookahead: u32) -> Self {
        self.lookahead = lookahead;
        self
    }

    /// Load [`PersistedWallet`] with the given `Db`.
    pub fn load_wallet<Db>(
        self,
        db: &mut Db,
    ) -> Result<Option<PersistedWallet>, <Wallet as PersistWith<Db>>::LoadError>
    where
        Wallet: PersistWith<Db, LoadParams = Self>,
    {
        PersistedWallet::load(db, self)
    }

    /// Load [`PersistedWallet`] with the given async `Db`.
    pub async fn load_wallet_async<Db>(
        self,
        db: &mut Db,
    ) -> Result<Option<PersistedWallet>, <Wallet as PersistAsyncWith<Db>>::LoadError>
    where
        Wallet: PersistAsyncWith<Db, LoadParams = Self>,
    {
        PersistedWallet::load_async(db, self).await
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
