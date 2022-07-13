///! SpendableCollection
///
/// This module defines the [`SpendableCollection`] trait.
use std::{cell::RefCell, sync::Arc};

use bitcoin::{Network, OutPoint, Script, Transaction, TxOut, Txid};
use miniscript::DescriptorTrait;

use crate::{
    address_validator::AddressValidator,
    database::{BatchDatabase, DatabaseUtils},
    descriptor::{AsDerived, ExtendedDescriptor},
    signer::{SignersContainer, TransactionSigner},
    BlockTime, Error, KeychainKind, LocalUtxo, Wallet,
};

use super::{utils::SecpCtx, AddressInfo};

/// Contains the "required" methods of [`SpendableCollection`], where all other methods could be
/// deried from (albiet probably not in an optimised manner).
pub trait SpendableCollectionInner {
    /// Iterates though descriptors with associated keychain kind.
    type DescIter: Iterator<Item = (ExtendedDescriptor, KeychainKind)> + ExactSizeIterator;

    /// Iterates though signers.
    type SignerIter: Iterator<Item = Arc<dyn TransactionSigner>>;

    /// Iterates through UTXOs with associated weight.
    type UtxoIter: Iterator<Item = (LocalUtxo, usize)>;

    /// Returns an iterator that ranges through all owned descriptors.
    fn iter_descriptors(&self) -> Result<Self::DescIter, Error>;

    /// Returns an iterator that ranges through all owned signers.
    fn iter_signers(&self) -> Result<Self::SignerIter, Error>;

    /// Obtains owned UTXOs alongside their satisfaction weights.
    ///     Warning: It is possible that the returned list contains a UTXO currently used as an
    ///     unconfirmed tx input.
    fn iter_utxos(&self) -> Result<Self::UtxoIter, Error>;

    /// Obtains transaction (and details) of given txid.
    ///
    /// Internally, we should include atleast all transactions containing owned and spendable UTXOs
    /// of the given descriptors (internal and external).
    /// However, having extra and unnecessary transactions should not hurt (TODO @evanlinjin:
    /// confirm this).
    ///
    /// TODO @evanlinjin: Maybe this should be in it's own trait?
    fn get_tx(&self, txid: &Txid) -> Result<Option<(Transaction, Option<BlockTime>)>, Error>;

    /// Returns a fresh/unused address derived from given descriptor. This is currently used for
    /// obtaining a change/drain address.
    #[deprecated(note = "Change selection should be in it's separate step")]
    fn new_address(&self, desc: &ExtendedDescriptor) -> Result<AddressInfo, Error>;
}

/// Represents a collection of owned and spendable `UTXO`s, `ExtendedDescriptor`s and associated
/// `Transaction`s.
pub trait SpendableCollection: SpendableCollectionInner {
    /// Obtains parent [`ExtendedDescriptor`] and child index of the provided `ScriptPubKey`.
    /// Note that if the script is not owned, None shoud be returned.
    fn get_path_of_script_pubkey(
        &self,
        script: &Script,
    ) -> Result<Option<(ExtendedDescriptor, u32)>, Error> {
        let descriptors = self
            .iter_descriptors()?
            .map(|(desc, _)| desc)
            .collect::<Vec<_>>();

        let secp = SecpCtx::new();

        for index in 0..2100_u32 {
            for desc in &descriptors {
                let derived_script = desc.as_derived(index, &secp).script_pubkey();
                if script == &derived_script {
                    return Ok(Some((desc.clone(), index)));
                }
            }
        }

        Ok(None)
    }

    /// Obtain a local UTXO given the provided outpoint.
    /// The default implementation is very inefficient and should be overloaded.
    fn get_utxo(&self, outpoint: &OutPoint) -> Result<Option<LocalUtxo>, Error> {
        Ok(self.iter_utxos()?.find_map(|(utxo, _)| {
            if &utxo.outpoint == outpoint {
                Some(utxo)
            } else {
                None
            }
        }))
    }

    /// Returns whether given script is owned by us, or not.
    fn is_mine(&self, script: &Script) -> Result<bool, Error> {
        self.get_path_of_script_pubkey(script)
            .map(|path| path.is_some())
    }

    /// Obtains a new change address without a descriptor provided explicitly
    #[deprecated]
    fn new_auto_address(&self) -> Result<AddressInfo, Error> {
        let (mut internal, mut external): (Vec<_>, Vec<_>) = self
            .iter_descriptors()?
            .partition(|(_, kc)| kc == &KeychainKind::Internal);

        internal.append(&mut external);

        let (desc, _) = internal.iter().chain(external.iter()).next().unwrap();

        #[allow(deprecated)]
        self.new_address(desc)
    }
}

/// Addons to [`SpendableCollection`] to allow for usage with fee-bump logic.
///
/// TODO @evanlinjin: decouple fee-bump logic from Wallet.
pub trait FeeBumpCollection: SpendableCollection {
    /// Previous output may be from a confirmed transaction.
    fn get_previous_output(&self, outpoint: &OutPoint) -> Result<Option<TxOut>, Error>;
}

/// Implements [`SpendableCollection`] with one external descriptor and an optional internal descriptor.
// #[derive(Clone)]
pub struct SpendableDatabase<'a, D> {
    descriptor: &'a ExtendedDescriptor,
    change_descriptor: Option<&'a ExtendedDescriptor>,
    network: Network,
    secp: SecpCtx,

    pub(crate) db: &'a RefCell<D>,
    pub(crate) signers: Vec<Arc<SignersContainer>>, // [external, internal]
    pub(crate) address_validators: Vec<Arc<dyn AddressValidator>>,
}

impl<'a, D: BatchDatabase> Clone for SpendableDatabase<'a, D> {
    fn clone(&self) -> Self {
        Self {
            descriptor: self.descriptor,
            change_descriptor: self.change_descriptor,
            network: self.network,
            secp: self.secp.clone(),
            db: self.db,
            signers: self.signers.clone(),
            address_validators: self.address_validators.clone(),
        }
    }
}

/// [`SpendableCollection`] `::DescIter` implementation for [`SpendableDatabase`].
pub struct DatabaseDescIter<'a, D>(SpendableDatabase<'a, D>, usize);

impl<'a, D> Iterator for DatabaseDescIter<'a, D> {
    type Item = (ExtendedDescriptor, KeychainKind);

    fn next(&mut self) -> Option<Self::Item> {
        self.1 += 1;

        match self.1 {
            1 => Some((self.0.descriptor.clone(), KeychainKind::External)),
            2 => self
                .0
                .change_descriptor
                .map(|change_desc| (change_desc.clone(), KeychainKind::Internal)),
            _ => None,
        }
    }
}

impl<'a, D> ExactSizeIterator for DatabaseDescIter<'a, D> {
    fn len(&self) -> usize {
        let start_count = if self.0.change_descriptor.is_some() {
            2
        } else {
            1
        };
        start_count - self.1
    }
}

impl<'a, D: BatchDatabase> SpendableDatabase<'a, D> {
    /// Creates a new [`SpendableDatabase`].
    pub fn new(
        descriptor: &'a ExtendedDescriptor,
        change_descriptor: Option<&'a ExtendedDescriptor>,
        network: Network,
        db: &'a RefCell<D>,
        signers: Vec<Arc<SignersContainer>>,
        address_validators: Vec<Arc<dyn AddressValidator>>,
    ) -> Self {
        let secp = SecpCtx::new();
        Self {
            descriptor,
            change_descriptor,
            network,
            secp,
            db,
            signers,
            address_validators,
        }
    }
}

impl<'a, D: BatchDatabase> SpendableCollectionInner for SpendableDatabase<'a, D> {
    type DescIter = DatabaseDescIter<'a, D>;

    type SignerIter = std::vec::IntoIter<Arc<dyn TransactionSigner>>;

    type UtxoIter = std::vec::IntoIter<(LocalUtxo, usize)>;

    fn iter_descriptors(&self) -> Result<Self::DescIter, Error> {
        Ok(DatabaseDescIter::<'_>(self.clone(), 0))
    }

    fn iter_signers(&self) -> Result<Self::SignerIter, Error> {
        #[allow(clippy::needless_collect)]
        let signers = self
            .signers
            .iter()
            .flat_map(|cont| cont.signers())
            .map(Arc::clone)
            .collect::<Vec<_>>();

        Ok(signers.into_iter())
    }

    /// TODO @evanlinjin:
    /// With the old implementation, we can call `get_descriptor_from_keychain` to obtain the
    /// parent descriptor without relying on a "cache" of relationships between `ScriptPubKey`s
    /// and "paths".
    /// However, we cannot continue to use this approach since we need to generalize everything
    /// to support multiple descriptors.
    /// A possible solution would be to replace usage of [`KeychainKind`] with
    /// `(KeychainKind, u32)`, so descriptors are referenced with an additional index.
    ///
    /// For now, we need to ensure the aforementioned relationship is sufficiently cached to
    /// avoid "missing" available UTXOs.
    fn iter_utxos(&self) -> Result<Self::UtxoIter, Error> {
        #[allow(clippy::needless_collect)]
        let utxos = self
            .db
            .borrow()
            .iter_utxos()?
            .into_iter()
            .filter(|utxo| !utxo.is_spent)
            .filter_map(|utxo| {
                let (parent_desc, _) = self
                    // @evanlinjin: Will panic with default implementation on timeout.
                    .get_path_of_script_pubkey(&utxo.txout.script_pubkey)
                    .unwrap()?;
                let weight = parent_desc.max_satisfaction_weight().unwrap();
                Some((utxo, weight))
            })
            .collect::<Vec<_>>();

        Ok(utxos.into_iter())
    }

    fn get_tx(&self, txid: &Txid) -> Result<Option<(Transaction, Option<BlockTime>)>, Error> {
        Ok(self.db.borrow().get_tx(txid, true)?.map(|tx_details| {
            (
                tx_details.transaction.unwrap(),
                tx_details.confirmation_time,
            )
        }))
    }

    fn new_address(&self, desc: &ExtendedDescriptor) -> Result<AddressInfo, Error> {
        let keychain = if desc == self.descriptor {
            KeychainKind::External
        } else if self.change_descriptor == Some(desc) {
            KeychainKind::Internal
        } else {
            return Err(Error::Generic("descriptor does not exist".to_string()));
        };

        Wallet::_get_new_address(
            self.db,
            &self.secp,
            self.address_validators.clone(),
            desc,
            keychain,
            self.network,
        )
    }
}

impl<'a, D: BatchDatabase> SpendableCollection for SpendableDatabase<'a, D> {
    fn get_utxo(&self, outpoint: &OutPoint) -> Result<Option<LocalUtxo>, Error> {
        self.db.borrow().get_utxo(outpoint)
    }

    fn get_path_of_script_pubkey(
        &self,
        script: &Script,
    ) -> Result<Option<(ExtendedDescriptor, u32)>, Error> {
        let cached_res =
            self.db
                .borrow()
                .get_path_from_script_pubkey(script)?
                .map(|(keychain, child_ind)| {
                    let desc = match keychain {
                        KeychainKind::External => self.descriptor,
                        KeychainKind::Internal => self.change_descriptor.unwrap(),
                    };

                    (desc.clone(), child_ind)
                });

        if cached_res.is_some() {
            return Ok(cached_res);
        }

        // // try brute-force method...
        let last_ext = self
            .db
            .borrow()
            .get_last_index(KeychainKind::External)?
            .unwrap_or(0_u32);
        let last_int = self
            .db
            .borrow()
            .get_last_index(KeychainKind::Internal)?
            .unwrap_or(0_u32);
        let start = std::cmp::min(last_ext, last_int);

        let descriptors = self
            .iter_descriptors()?
            .map(|(desc, _)| desc)
            .collect::<Vec<_>>();

        let secp = SecpCtx::new();

        for index in start..start + 2100_u32 {
            for desc in &descriptors {
                let derived_script = desc.as_derived(index, &secp).script_pubkey();
                if script == &derived_script {
                    return Ok(Some((desc.clone(), index)));
                }
            }
        }

        Ok(None)
    }

    fn is_mine(&self, script: &Script) -> Result<bool, Error> {
        self.db.borrow().is_mine(script)
    }
}
