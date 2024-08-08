use alloc::{boxed::Box, collections::BTreeMap};
use bdk_chain::{keychain_txout::DEFAULT_LOOKAHEAD, PersistAsyncWith, PersistWith};
use bitcoin::{BlockHash, Network};
use miniscript::descriptor::KeyMap;

use crate::{
    descriptor::{DescriptorError, ExtendedDescriptor, IntoWalletDescriptor},
    utils::SecpCtx,
    Wallet,
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
pub struct CreateParams<K> {
    pub(crate) descriptors: BTreeMap<K, DescriptorToExtract>,
    pub(crate) keymaps: BTreeMap<K, KeyMap>,
    pub(crate) network: Network,
    pub(crate) genesis_hash: Option<BlockHash>,
    pub(crate) lookahead: u32,
}

impl<K: core::fmt::Debug + Clone + Ord> CreateParams<K> {
    /// Construct parameters with provided `descriptor`, `change_descriptor`.
    ///
    /// Default values: `genesis_hash` = `None`, `lookahead` = [`DEFAULT_LOOKAHEAD`]
    pub fn new() -> Self {
        Self {
            descriptors: BTreeMap::new(),
            keymaps: BTreeMap::new(),
            network: Network::Bitcoin,
            genesis_hash: None,
            lookahead: DEFAULT_LOOKAHEAD,
        }
    }

    pub fn descriptor<D: IntoWalletDescriptor + 'static>(
        mut self,
        keychain: K,
        descriptor: D,
    ) -> Self {
        self.descriptors
            .insert(keychain, make_descriptor_to_extract(descriptor));
        self
    }

    /// Extend the `keymap` of the given `keychain`.
    pub fn keymap(mut self, keychain: K, keymap: KeyMap) -> Self {
        self.keymaps.entry(keychain).or_default().extend(keymap);
        self
    }

    /// Create [`Wallet`] without persistence.
    pub fn create_wallet_no_persist(self) -> Result<Wallet<K>, DescriptorError> {
        Wallet::<K>::create_with_params(self)
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
    ) -> Result<PersistedWallet<K>, <Wallet<K> as PersistWith<Db>>::CreateError>
    where
        Wallet<K>: PersistWith<Db, CreateParams = Self>,
    {
        PersistedWallet::<K>::create(db, self)
    }

    /// Create [`PersistedWallet`] with the given async `Db`.
    pub async fn create_wallet_async<Db>(
        self,
        db: &mut Db,
    ) -> Result<PersistedWallet<K>, <Wallet<K> as PersistAsyncWith<Db>>::CreateError>
    where
        Wallet<K>: PersistAsyncWith<Db, CreateParams = Self>,
    {
        PersistedWallet::<K>::create_async(db, self).await
    }
}

/// Parameters for [`Wallet::load`] or [`PersistedWallet::load`].
#[must_use]
pub struct LoadParams<K> {
    pub(crate) keymaps: BTreeMap<K, KeyMap>,
    pub(crate) lookahead: u32,
    pub(crate) check_network: Option<Network>,
    pub(crate) check_genesis_hash: Option<BlockHash>,
    pub(crate) check_descriptors: BTreeMap<K, DescriptorToExtract>,
}

impl<K: core::fmt::Debug + Clone + Ord> LoadParams<K> {
    /// Construct parameters with default values.
    ///
    /// Default values: `lookahead` = [`DEFAULT_LOOKAHEAD`]
    pub fn new() -> Self {
        Self {
            keymaps: BTreeMap::default(),
            lookahead: DEFAULT_LOOKAHEAD,
            check_network: None,
            check_genesis_hash: None,
            check_descriptors: BTreeMap::default(),
        }
    }

    /// Extend the given `keychain`'s `keymap`.
    pub fn keymap(mut self, keychain: K, keymap: KeyMap) -> Self {
        self.keymaps.entry(keychain).or_default().extend(keymap);
        self
    }

    /// Checks that `descriptor` of `keychain` matches this, and extracts private keys (if
    /// available).
    pub fn descriptor<D>(mut self, keychain: K, descriptor: D) -> Self
    where
        D: IntoWalletDescriptor + 'static,
    {
        self.check_descriptors
            .insert(keychain, make_descriptor_to_extract(descriptor));
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
    ) -> Result<Option<PersistedWallet<K>>, <Wallet<K> as PersistWith<Db>>::LoadError>
    where
        Wallet<K>: PersistWith<Db, LoadParams = Self>,
    {
        PersistedWallet::<K>::load(db, self)
    }

    /// Load [`PersistedWallet`] with the given async `Db`.
    pub async fn load_wallet_async<Db>(
        self,
        db: &mut Db,
    ) -> Result<Option<PersistedWallet<K>>, <Wallet<K> as PersistAsyncWith<Db>>::LoadError>
    where
        Wallet<K>: PersistAsyncWith<Db, LoadParams = Self>,
    {
        PersistedWallet::<K>::load_async(db, self).await
    }

    /// Load [`Wallet`] without persistence.
    pub fn load_wallet_no_persist(
        self,
        changeset: ChangeSet<K>,
    ) -> Result<Option<Wallet<K>>, LoadError<K>> {
        Wallet::<K>::load_with_params(changeset, self)
    }
}

impl<K: core::fmt::Debug + Clone + Ord> Default for LoadParams<K> {
    fn default() -> Self {
        Self::new()
    }
}
