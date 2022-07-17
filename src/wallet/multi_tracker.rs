// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2022 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

///! MultiTracker
///
/// This module defines the [`MultiTracker`] trait.
use std::{cell::RefCell, collections::HashMap, sync::Arc};

use bitcoin::{Network, OutPoint, Script, Transaction, TxOut, Txid};
use miniscript::DescriptorTrait;

#[allow(deprecated)]
use crate::{
    address_validator::AddressValidator,
    database::{BatchDatabase, DatabaseUtils},
    descriptor::{AsDerived, ExtendedDescriptor},
    signer::{SignersContainer, TransactionSigner},
    BlockTime, Error, KeychainKind, LocalUtxo, Wallet,
};

use super::{utils::SecpCtx, AddressInfo};

/// Contains a descriptor and associated data
#[derive(Debug, Clone)]
pub struct DescriptorItem {
    /// Wallet descriptor
    pub descriptor: ExtendedDescriptor,
    /// Keychain kind of descriptor - external/internal
    pub keychain: KeychainKind,
    /// Signers of descriptor
    pub signers: Arc<SignersContainer>,
}

/// Contains a transaction and it's associated data
#[derive(Debug, Clone)]
pub struct TransactionItem {
    /// Raw transaction
    pub raw: Transaction,
    /// Confirmed time (if any) of the transaction
    pub confirmed: Option<BlockTime>,
    /// Fees of the transaction (if avaliable)
    pub fees: Option<u64>,
}

/// Contains a local tx output and it's associated data
/// TODO @evanlinjin: Figure out if this is useful
#[derive(Debug, Clone)]
pub struct LocalOutputItem {
    /// References this output
    pub outpoint: OutPoint,
    /// The actual tx output
    pub txout: TxOut,
    /// Confirmation time (if any) of the tx that contains this output
    pub confirmed: Option<BlockTime>,
    /// Path for `scriptPubKey` creation (descriptor_item, child_index)
    pub script_path: (DescriptorItem, u32),
    /// Satisfaction weight for the `scriptPubKey` (required weight of corresponding
    /// `scriptSig`/`witness` data)
    pub satisfaction_weight: u32,
}

/// Contains the "required" methods of [`MultiTracker`], where all other methods could be
/// deried from (albiet probably not in an optimised manner).
///
/// TODO @evanlinjin: Should we have an `iter_txout` method as defined in `bdk_core`?
/// <https://github.com/LLFourn/bdk_core_staging/blob/67d1e5499b0eb1de59f652c433c88942646593b3/bdk_core/src/descriptor_tracker.rs#L873>
pub trait MultiTrackerInner {
    /// Returns an iterator that ranges through all owned descriptors.
    fn iter_descriptors(&self) -> Result<Box<dyn Iterator<Item = DescriptorItem> + '_>, Error>;

    /// Returns an iterator that ranges through all owned signers.
    fn iter_signers(
        &self,
    ) -> Result<Box<dyn Iterator<Item = Arc<dyn TransactionSigner>> + '_>, Error>;

    /// Obtains owned UTXOs alongside their satisfaction weights.
    ///
    /// TODO @evanlinjin: We should have our own structure for Unspents.
    /// Fields that make sense =>
    /// * Confirmed height.
    /// * Descriptor + Child index.
    /// * Satisfaction weight of scriptPubKey (weight of corresponding scriptSig/witness data).
    fn iter_unspent(&self) -> Result<Box<dyn Iterator<Item = (LocalUtxo, usize)> + '_>, Error>;

    /// Obtains transaction (and details) of given txid.
    ///
    /// Internally, we should include atleast all transactions containing owned and spendable UTXOs
    /// of the given descriptors (internal and external).
    fn get_tx(&self, txid: &Txid) -> Result<Option<TransactionItem>, Error>;

    /// Obtains the latest block height of internal trackers.
    fn latest_blockheight(&self) -> Result<Option<BlockTime>, Error>;

    /// Returns a fresh/unused address derived from given descriptor. This is currently used for
    /// obtaining a change/drain address.
    #[deprecated(note = "Change selection should be in it's separate step")]
    fn new_address(&self, desc: &ExtendedDescriptor) -> Result<AddressInfo, Error>;
}

/// Represents a collection of owned and spendable `UTXO`s, `ExtendedDescriptor`s and associated
/// `Transaction`s.
pub trait MultiTracker: MultiTrackerInner {
    /// Obtains parent [`ExtendedDescriptor`] and child index of the provided `ScriptPubKey`.
    /// Note that if the script is not owned, None shoud be returned.
    ///
    /// The default implementation of this method is very inefficient and should be overloaded.
    fn get_path_of_script_pubkey(
        &self,
        script: &Script,
    ) -> Result<Option<(DescriptorItem, u32)>, Error> {
        let descriptors = self.iter_descriptors()?.collect::<Vec<_>>();

        let secp = SecpCtx::new();

        for index in 0..200_u32 {
            for item in &descriptors {
                let desc = item.descriptor.clone();
                let derived_script = desc.as_derived(index, &secp).script_pubkey();
                if script == &derived_script {
                    return Ok(Some((item.clone(), index)));
                }
            }
        }

        Ok(None)
    }

    /// Obtain a local UTXO given the provided outpoint.
    /// The default implementation is very inefficient and should be overloaded.
    fn get_utxo(&self, outpoint: &OutPoint) -> Result<Option<LocalUtxo>, Error> {
        Ok(self.iter_unspent()?.find_map(|(utxo, _)| {
            if &utxo.outpoint == outpoint {
                Some(utxo)
            } else {
                None
            }
        }))
    }

    /// Obtain output of the provided outpoint.
    /// Output may not be owned by us, just be part of a transaction that we are keeping track of.
    fn get_output(&self, outpoint: &OutPoint) -> Result<Option<TxOut>, Error> {
        match self.get_tx(&outpoint.txid)? {
            // return error if vout is invalid
            Some(tx) => tx.raw.output.get(outpoint.vout as usize).map_or_else(
                || Err(Error::InvalidOutpoint(*outpoint)),
                |tx_out| Ok(Some(tx_out.clone())),
            ),
            None => Ok(None),
        }
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
            .partition(|item| item.keychain == KeychainKind::Internal);

        internal.append(&mut external);

        let item = internal.iter().chain(external.iter()).next().unwrap();

        #[allow(deprecated)]
        self.new_address(&item.descriptor)
    }
}

/// Implements [`MultiTracker`] with one external descriptor and an optional internal descriptor.
// #[derive(Clone)]
pub struct LegacyTracker<'a, D> {
    descriptor: &'a ExtendedDescriptor,
    change_descriptor: Option<&'a ExtendedDescriptor>,
    network: Network,
    secp: SecpCtx,

    pub(crate) db: &'a RefCell<D>,
    pub(crate) signers: Vec<Arc<SignersContainer>>, // [external, internal]
    #[allow(deprecated)]
    pub(crate) address_validators: Vec<Arc<dyn AddressValidator>>,
}

impl<'a, D: BatchDatabase> Clone for LegacyTracker<'a, D> {
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

/// [`MultiTracker`] descriptor iterator implementation for [`LegacyTracker`].
pub struct LegacyDescIter<'a, D>(LegacyTracker<'a, D>, usize);

impl<'a, D> Iterator for LegacyDescIter<'a, D> {
    type Item = DescriptorItem;

    fn next(&mut self) -> Option<Self::Item> {
        self.1 += 1;

        match self.1 {
            1 => Some(DescriptorItem {
                descriptor: self.0.descriptor.clone(),
                keychain: KeychainKind::External,
                signers: Arc::clone(&self.0.signers[0]),
            }),
            2 => self.0.change_descriptor.map(|change_desc| DescriptorItem {
                descriptor: change_desc.clone(),
                keychain: KeychainKind::Internal,
                signers: Arc::clone(&self.0.signers[1]),
            }),
            _ => None,
        }
    }
}

impl<'a, D> ExactSizeIterator for LegacyDescIter<'a, D> {
    fn len(&self) -> usize {
        let start_count = if self.0.change_descriptor.is_some() {
            2
        } else {
            1
        };
        start_count - self.1
    }
}

impl<'a, D: BatchDatabase> LegacyTracker<'a, D> {
    /// Creates a new [`LegacyTracker`].
    #[allow(deprecated)]
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

impl<'a, D: BatchDatabase> MultiTrackerInner for LegacyTracker<'a, D> {
    fn iter_descriptors(&self) -> Result<Box<dyn Iterator<Item = DescriptorItem> + '_>, Error> {
        Ok(Box::new(LegacyDescIter::<'_>(self.clone(), 0)))
    }

    fn iter_signers(
        &self,
    ) -> Result<Box<dyn Iterator<Item = Arc<dyn TransactionSigner>> + '_>, Error> {
        let signers = self
            .signers
            .iter()
            .flat_map(|cont| cont.signers())
            .map(Arc::clone);

        Ok(Box::new(signers))
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
    fn iter_unspent(&self) -> Result<Box<dyn Iterator<Item = (LocalUtxo, usize)> + '_>, Error> {
        let utxos = self
            .db
            .borrow()
            .iter_utxos()?
            .into_iter()
            .filter(|utxo| !utxo.is_spent)
            .filter_map(move |utxo| {
                let (item, _) = self
                    // @evanlinjin: Will panic with default implementation on timeout.
                    .get_path_of_script_pubkey(&utxo.txout.script_pubkey)
                    .unwrap()?;
                let weight = item.descriptor.max_satisfaction_weight().unwrap();
                Some((utxo, weight))
            });

        Ok(Box::new(utxos))
    }

    fn get_tx(&self, txid: &Txid) -> Result<Option<TransactionItem>, Error> {
        Ok(self
            .db
            .borrow()
            .get_tx(txid, true)?
            .map(|details| TransactionItem {
                raw: details.transaction.unwrap(),
                confirmed: details.confirmation_time,
                fees: details.fee,
            }))
    }

    fn latest_blockheight(&self) -> Result<Option<BlockTime>, Error> {
        self.db
            .borrow()
            .get_sync_time()
            .map(|opt| opt.map(|sync_time| sync_time.block_time))
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

impl<'a, D: BatchDatabase> MultiTracker for LegacyTracker<'a, D> {
    fn get_utxo(&self, outpoint: &OutPoint) -> Result<Option<LocalUtxo>, Error> {
        self.db.borrow().get_utxo(outpoint)
    }

    fn get_path_of_script_pubkey(
        &self,
        script: &Script,
    ) -> Result<Option<(DescriptorItem, u32)>, Error> {
        // TODO: Add as struct field
        let desc_map = self
            .iter_descriptors()?
            .map(|desc| (desc.descriptor.to_string(), desc))
            .collect::<HashMap<_, _>>();

        // check internal cache for relation
        let cached_res =
            self.db
                .borrow()
                .get_path_from_script_pubkey(script)?
                .map(|(keychain, child_ind)| {
                    let desc = match keychain {
                        KeychainKind::External => self.descriptor,
                        KeychainKind::Internal => self.change_descriptor.unwrap(),
                    };
                    let item = desc_map.get(&desc.to_string()).unwrap();
                    (item.clone(), child_ind)
                });

        if cached_res.is_some() {
            return Ok(cached_res);
        }

        // try brute-force method
        let db = self.db.borrow();
        let last_ext = db.get_last_index(KeychainKind::External)?.unwrap_or(0_u32);
        let last_int = db.get_last_index(KeychainKind::Internal)?.unwrap_or(0_u32);
        let start = std::cmp::min(last_ext, last_int);

        let descriptors = self.iter_descriptors()?.collect::<Vec<_>>();

        for index in start..start + 200_u32 {
            for item in &descriptors {
                let derived_script = item
                    .descriptor
                    .as_derived(index, &self.secp)
                    .script_pubkey();
                if script == &derived_script {
                    return Ok(Some((item.clone(), index)));
                }
            }
        }

        Ok(None)
    }

    fn is_mine(&self, script: &Script) -> Result<bool, Error> {
        self.db.borrow().is_mine(script)
    }
}
