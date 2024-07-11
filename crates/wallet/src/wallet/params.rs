use bdk_chain::{keychain_txout::DEFAULT_LOOKAHEAD, PersistAsyncWith, PersistWith};
use bitcoin::{BlockHash, Network};
use miniscript::descriptor::KeyMap;

use crate::{
    descriptor::{DescriptorError, ExtendedDescriptor, IntoWalletDescriptor},
    KeychainKind, Wallet,
};

use super::{utils::SecpCtx, ChangeSet, LoadError, PersistedWallet};

/// Parameters for [`Wallet::create`] or [`PersistedWallet::create`].
#[derive(Debug, Clone)]
#[must_use]
pub struct CreateParams {
    pub(crate) descriptor: ExtendedDescriptor,
    pub(crate) descriptor_keymap: KeyMap,
    pub(crate) change_descriptor: ExtendedDescriptor,
    pub(crate) change_descriptor_keymap: KeyMap,
    pub(crate) network: Network,
    pub(crate) genesis_hash: Option<BlockHash>,
    pub(crate) lookahead: u32,
    pub(crate) secp: SecpCtx,
}

impl CreateParams {
    /// Construct parameters with provided `descriptor`, `change_descriptor` and `network`.
    ///
    /// Default values: `genesis_hash` = `None`, `lookahead` = [`DEFAULT_LOOKAHEAD`]
    pub fn new<E: IntoWalletDescriptor>(
        descriptor: E,
        change_descriptor: E,
        network: Network,
    ) -> Result<Self, DescriptorError> {
        let secp = SecpCtx::default();

        let (descriptor, descriptor_keymap) = descriptor.into_wallet_descriptor(&secp, network)?;
        let (change_descriptor, change_descriptor_keymap) =
            change_descriptor.into_wallet_descriptor(&secp, network)?;

        Ok(Self {
            descriptor,
            descriptor_keymap,
            change_descriptor,
            change_descriptor_keymap,
            network,
            genesis_hash: None,
            lookahead: DEFAULT_LOOKAHEAD,
            secp,
        })
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

/// Parameters for [`Wallet::load`] or [`PersistedWallet::load`].
#[must_use]
#[derive(Debug, Clone)]
pub struct LoadParams {
    pub(crate) descriptor_keymap: KeyMap,
    pub(crate) change_descriptor_keymap: KeyMap,
    pub(crate) lookahead: u32,
    pub(crate) check_network: Option<Network>,
    pub(crate) check_genesis_hash: Option<BlockHash>,
    pub(crate) check_descriptor: Option<ExtendedDescriptor>,
    pub(crate) check_change_descriptor: Option<ExtendedDescriptor>,
    pub(crate) secp: SecpCtx,
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
            secp: SecpCtx::new(),
        }
    }

    /// Construct parameters with `network` check.
    pub fn with_network(network: Network) -> Self {
        Self {
            check_network: Some(network),
            ..Default::default()
        }
    }

    /// Construct parameters with descriptor checks.
    pub fn with_descriptors<E: IntoWalletDescriptor>(
        descriptor: E,
        change_descriptor: E,
        network: Network,
    ) -> Result<Self, DescriptorError> {
        let mut params = Self::with_network(network);
        let secp = &params.secp;

        let (descriptor, descriptor_keymap) = descriptor.into_wallet_descriptor(secp, network)?;
        params.check_descriptor = Some(descriptor);
        params.descriptor_keymap = descriptor_keymap;

        let (change_descriptor, change_descriptor_keymap) =
            change_descriptor.into_wallet_descriptor(secp, network)?;
        params.check_change_descriptor = Some(change_descriptor);
        params.change_descriptor_keymap = change_descriptor_keymap;

        Ok(params)
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
