use bdk_chain::PersistBackend;
use bitcoin::constants::genesis_block;
use bitcoin::{BlockHash, Network};

use crate::descriptor::{DescriptorError, IntoWalletDescriptor};
use crate::wallet::{ChangeSet, Wallet};

use super::{InitError, InitOrLoadError};

/// Helper structure to initialize a fresh [`Wallet`] instance.
pub struct WalletBuilder<E> {
    pub(super) descriptor: E,
    pub(super) change_descriptor: Option<E>,
    pub(super) network: Network,
    pub(super) custom_genesis_hash: Option<BlockHash>,
}

impl<E> WalletBuilder<E> {
    /// Start building a fresh [`Wallet`] instance.
    pub(super) fn new(descriptor: E) -> Self {
        Self {
            descriptor,
            change_descriptor: None,
            network: Network::Bitcoin,
            custom_genesis_hash: None,
        }
    }

    /// Set the internal (change) keychain.
    ///
    /// The internal keychain will be used to derive change addresses for change outputs.
    ///
    /// If no internal keychain is set, the wallet will use the external keychain for deriving
    /// change addresses.
    pub fn with_change_descriptor(mut self, change_descriptor: E) -> Self {
        self.change_descriptor = Some(change_descriptor);
        self
    }

    /// Set the [`Network`] type for the wallet.
    ///
    /// This changes the format of the derived wallet, as well as the internal genesis block hash.
    /// The internal genesis block hash can be overriden by [`with_genesis_hash`].
    ///
    /// By default, [`Network::Bitcoin`] is used.
    ///
    /// [`with_genesis_hash`]: Self::with_genesis_hash
    pub fn with_network(mut self, network: Network) -> Self {
        self.network = network;
        self
    }

    /// Overrides the genesis hash implied by [`with_network`].
    ///
    /// [`with_network`]: Self::with_network
    pub fn with_genesis_hash(mut self, genesis_hash: BlockHash) -> Self {
        self.custom_genesis_hash = Some(genesis_hash);
        self
    }

    pub(super) fn determine_genesis_hash(&self) -> BlockHash {
        self.custom_genesis_hash
            .unwrap_or_else(|| genesis_block(self.network).block_hash())
    }
}

impl<E: IntoWalletDescriptor> WalletBuilder<E> {
    /// Initializes a fresh wallet and persists it in `db`.
    pub fn init<D>(self, db: D) -> Result<Wallet<D>, InitError<D::WriteError>>
    where
        D: PersistBackend<ChangeSet>,
    {
        Wallet::init(self, db)
    }

    /// Either loads [`Wallet`] from persistence, or initializes a fresh wallet if it does not
    /// exist.
    ///
    /// This method will fail if the persistence is non-empty with parameters that are different to
    /// those specified by [`WalletBuilder`].
    pub fn init_or_load<D>(
        self,
        db: D,
    ) -> Result<Wallet<D>, InitOrLoadError<D::WriteError, D::LoadError>>
    where
        D: PersistBackend<ChangeSet>,
    {
        Wallet::init_or_load(self, db)
    }

    /// Initializes a fresh wallet with no persistence.
    pub fn init_without_persistence(self) -> Result<Wallet<()>, DescriptorError> {
        Wallet::init(self, ()).map_err(|err| match err {
            InitError::Descriptor(err) => err,
            InitError::Write(_) => panic!("there is no db to write to"),
        })
    }
}
