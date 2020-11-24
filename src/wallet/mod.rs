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

//! Wallet
//!
//! This module defines the [`Wallet`] structure.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::{BTreeMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use bitcoin::secp256k1::Secp256k1;

use bitcoin::consensus::encode::serialize;
use bitcoin::util::bip32::ChildNumber;
use bitcoin::util::psbt::PartiallySignedTransaction as PSBT;
use bitcoin::{Address, Network, OutPoint, Script, Transaction, TxOut, Txid};

use miniscript::psbt::PsbtInputSatisfier;

#[allow(unused_imports)]
use log::{debug, error, info, trace};

pub mod address_validator;
pub mod coin_selection;
pub mod export;
pub mod signer;
pub mod time;
pub mod tx_builder;
pub(crate) mod utils;

pub use utils::IsDust;

use address_validator::AddressValidator;
use signer::{Signer, SignerId, SignerOrdering, SignersContainer};
use tx_builder::{BumpFee, CreateTx, FeePolicy, TxBuilder, TxBuilderContext};
use utils::{descriptor_to_pk_ctx, After, Older, SecpCtx};

use crate::blockchain::{Blockchain, BlockchainMarker, OfflineBlockchain, Progress};
use crate::database::{BatchDatabase, BatchOperations, DatabaseUtils};
use crate::descriptor::{
    get_checksum, DescriptorMeta, DescriptorScripts, ExtendedDescriptor, ExtractPolicy, Policy,
    ToWalletDescriptor,
};
use crate::error::Error;
use crate::psbt::PSBTUtils;
use crate::types::*;

const CACHE_ADDR_BATCH_SIZE: u32 = 100;

/// Type alias for a [`Wallet`] that uses [`OfflineBlockchain`]
pub type OfflineWallet<D> = Wallet<OfflineBlockchain, D>;

/// A Bitcoin wallet
///
/// A wallet takes descriptors, a [`database`](crate::database) and a
/// [`blockchain`](crate::blockchain) and implements the basic functions that a Bitcoin wallets
/// needs to operate, like [generating addresses](Wallet::get_new_address), [returning the balance](Wallet::get_balance),
/// [creating transactions](Wallet::create_tx), etc.
///
/// A wallet can be either "online" if the [`blockchain`](crate::blockchain) type provided
/// implements [`Blockchain`], or "offline" [`OfflineBlockchain`] is used. Offline wallets only expose
/// methods that don't need any interaction with the blockchain to work.
pub struct Wallet<B, D> {
    descriptor: ExtendedDescriptor,
    change_descriptor: Option<ExtendedDescriptor>,

    signers: Arc<SignersContainer>,
    change_signers: Arc<SignersContainer>,

    address_validators: Vec<Arc<dyn AddressValidator>>,

    network: Network,

    current_height: Option<u32>,

    client: Option<B>,
    database: RefCell<D>,

    secp: SecpCtx,
}

// offline actions, always available
impl<B, D> Wallet<B, D>
where
    B: BlockchainMarker,
    D: BatchDatabase,
{
    /// Create a new "offline" wallet
    pub fn new_offline<E: ToWalletDescriptor>(
        descriptor: E,
        change_descriptor: Option<E>,
        network: Network,
        mut database: D,
    ) -> Result<Self, Error> {
        let (descriptor, keymap) = descriptor.to_wallet_descriptor(network)?;
        database.check_descriptor_checksum(
            ScriptType::External,
            get_checksum(&descriptor.to_string())?.as_bytes(),
        )?;
        let signers = Arc::new(SignersContainer::from(keymap));
        let (change_descriptor, change_signers) = match change_descriptor {
            Some(desc) => {
                let (change_descriptor, change_keymap) = desc.to_wallet_descriptor(network)?;
                database.check_descriptor_checksum(
                    ScriptType::Internal,
                    get_checksum(&change_descriptor.to_string())?.as_bytes(),
                )?;

                let change_signers = Arc::new(SignersContainer::from(change_keymap));
                // if !parsed.same_structure(descriptor.as_ref()) {
                //     return Err(Error::DifferentDescriptorStructure);
                // }

                (Some(change_descriptor), change_signers)
            }
            None => (None, Arc::new(SignersContainer::new())),
        };

        Ok(Wallet {
            descriptor,
            change_descriptor,
            signers,
            change_signers,
            address_validators: Vec::new(),

            network,

            current_height: None,

            client: None,
            database: RefCell::new(database),

            secp: Secp256k1::new(),
        })
    }

    /// Return a newly generated address using the external descriptor
    pub fn get_new_address(&self) -> Result<Address, Error> {
        let index = self.fetch_and_increment_index(ScriptType::External)?;
        let deriv_ctx = descriptor_to_pk_ctx(&self.secp);

        self.descriptor
            .derive(ChildNumber::from_normal_idx(index)?)
            .address(self.network, deriv_ctx)
            .ok_or(Error::ScriptDoesntHaveAddressForm)
    }

    /// Return whether or not a `script` is part of this wallet (either internal or external)
    pub fn is_mine(&self, script: &Script) -> Result<bool, Error> {
        self.database.borrow().is_mine(script)
    }

    /// Return the list of unspent outputs of this wallet
    ///
    /// Note that this methods only operate on the internal database, which first needs to be
    /// [`Wallet::sync`] manually.
    pub fn list_unspent(&self) -> Result<Vec<UTXO>, Error> {
        self.database.borrow().iter_utxos()
    }

    /// Return the list of transactions made and received by the wallet
    ///
    /// Optionally fill the [`TransactionDetails::transaction`] field with the raw transaction if
    /// `include_raw` is `true`.
    ///
    /// Note that this methods only operate on the internal database, which first needs to be
    /// [`Wallet::sync`] manually.
    pub fn list_transactions(&self, include_raw: bool) -> Result<Vec<TransactionDetails>, Error> {
        self.database.borrow().iter_txs(include_raw)
    }

    /// Return the balance, meaning the sum of this wallet's unspent outputs' values
    ///
    /// Note that this methods only operate on the internal database, which first needs to be
    /// [`Wallet::sync`] manually.
    pub fn get_balance(&self) -> Result<u64, Error> {
        Ok(self
            .list_unspent()?
            .iter()
            .fold(0, |sum, i| sum + i.txout.value))
    }

    /// Add an external signer
    ///
    /// See [the `signer` module](signer) for an example.
    pub fn add_signer(
        &mut self,
        script_type: ScriptType,
        id: SignerId,
        ordering: SignerOrdering,
        signer: Arc<dyn Signer>,
    ) {
        let signers = match script_type {
            ScriptType::External => Arc::make_mut(&mut self.signers),
            ScriptType::Internal => Arc::make_mut(&mut self.change_signers),
        };

        signers.add_external(id, ordering, signer);
    }

    /// Add an address validator
    ///
    /// See [the `address_validator` module](address_validator) for an example.
    pub fn add_address_validator(&mut self, validator: Arc<dyn AddressValidator>) {
        self.address_validators.push(validator);
    }

    /// Create a new transaction following the options specified in the `builder`
    ///
    /// ## Example
    ///
    /// ```no_run
    /// # use std::str::FromStr;
    /// # use bitcoin::*;
    /// # use bdk::*;
    /// # use bdk::database::*;
    /// # let descriptor = "wpkh(tpubD6NzVbkrYhZ4Xferm7Pz4VnjdcDPFyjVu5K4iZXQ4pVN8Cks4pHVowTBXBKRhX64pkRyJZJN5xAKj4UDNnLPb5p2sSKXhewoYx5GbTdUFWq/*)";
    /// # let wallet: OfflineWallet<_> = Wallet::new_offline(descriptor, None, Network::Testnet, MemoryDatabase::default())?;
    /// # let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap();
    /// let (psbt, details) = wallet.create_tx(
    ///     TxBuilder::with_recipients(vec![(to_address.script_pubkey(), 50_000)])
    /// )?;
    /// // sign and broadcast ...
    /// # Ok::<(), bdk::Error>(())
    /// ```
    pub fn create_tx<Cs: coin_selection::CoinSelectionAlgorithm<D>>(
        &self,
        builder: TxBuilder<D, Cs, CreateTx>,
    ) -> Result<(PSBT, TransactionDetails), Error> {
        let external_policy = self
            .descriptor
            .extract_policy(&self.signers, &self.secp)?
            .unwrap();
        let internal_policy = self
            .change_descriptor
            .as_ref()
            .map(|desc| {
                Ok::<_, Error>(
                    desc.extract_policy(&self.change_signers, &self.secp)?
                        .unwrap(),
                )
            })
            .transpose()?;

        // The policy allows spending external outputs, but it requires a policy path that hasn't been
        // provided
        if builder.change_policy != tx_builder::ChangeSpendPolicy::OnlyChange
            && external_policy.requires_path()
            && builder.external_policy_path.is_none()
        {
            return Err(Error::SpendingPolicyRequired(ScriptType::External));
        };
        // Same for the internal_policy path, if present
        if let Some(internal_policy) = &internal_policy {
            if builder.change_policy != tx_builder::ChangeSpendPolicy::ChangeForbidden
                && internal_policy.requires_path()
                && builder.internal_policy_path.is_none()
            {
                return Err(Error::SpendingPolicyRequired(ScriptType::Internal));
            };
        }

        let external_requirements = external_policy.get_condition(
            builder
                .external_policy_path
                .as_ref()
                .unwrap_or(&BTreeMap::new()),
        )?;
        let internal_requirements = internal_policy
            .map(|policy| {
                Ok::<_, Error>(
                    policy.get_condition(
                        builder
                            .internal_policy_path
                            .as_ref()
                            .unwrap_or(&BTreeMap::new()),
                    )?,
                )
            })
            .transpose()?;

        let requirements = external_requirements
            .clone()
            .merge(&internal_requirements.unwrap_or_default())?;
        debug!("Policy requirements: {:?}", requirements);

        let version = match builder.version {
            Some(tx_builder::Version(0)) => {
                return Err(Error::Generic("Invalid version `0`".into()))
            }
            Some(tx_builder::Version(1)) if requirements.csv.is_some() => {
                return Err(Error::Generic(
                    "TxBuilder requested version `1`, but at least `2` is needed to use OP_CSV"
                        .into(),
                ))
            }
            Some(tx_builder::Version(x)) => x,
            None if requirements.csv.is_some() => 2,
            _ => 1,
        };

        let lock_time = match builder.locktime {
            None => requirements.timelock.unwrap_or(0),
            Some(x) if requirements.timelock.is_none() => x,
            Some(x) if requirements.timelock.unwrap() <= x => x,
            Some(x) => return Err(Error::Generic(format!("TxBuilder requested timelock of `{}`, but at least `{}` is required to spend from this script", x, requirements.timelock.unwrap())))
        };

        let n_sequence = match (builder.rbf, requirements.csv) {
            (None, Some(csv)) => csv,
            (Some(rbf), Some(csv)) if rbf < csv => return Err(Error::Generic(format!("Cannot enable RBF with nSequence `{}`, since at least `{}` is required to spend with OP_CSV", rbf, csv))),
            (None, _) if requirements.timelock.is_some() => 0xFFFFFFFE,
            (Some(rbf), _) if rbf >= 0xFFFFFFFE => return Err(Error::Generic("Cannot enable RBF with a nSequence >= 0xFFFFFFFE".into())),
            (Some(rbf), _) => rbf,
            (None, _) => 0xFFFFFFFF,
        };

        let mut tx = Transaction {
            version,
            lock_time,
            input: vec![],
            output: vec![],
        };

        let (fee_rate, mut fee_amount) = match builder
            .fee_policy
            .as_ref()
            .unwrap_or(&FeePolicy::FeeRate(FeeRate::default()))
        {
            FeePolicy::FeeAmount(amount) => (FeeRate::from_sat_per_vb(0.0), *amount as f32),
            FeePolicy::FeeRate(rate) => (*rate, 0.0),
        };

        // try not to move from `builder` because we still need to use it later.
        let recipients = match &builder.single_recipient {
            Some(recipient) => vec![(recipient, 0)],
            None => builder.recipients.iter().map(|(r, v)| (r, *v)).collect(),
        };
        if builder.single_recipient.is_some()
            && !builder.manually_selected_only
            && !builder.drain_wallet
        {
            return Err(Error::SingleRecipientNoInputs);
        }
        if recipients.is_empty() {
            return Err(Error::NoRecipients);
        }

        if builder.manually_selected_only && builder.utxos.is_empty() {
            return Err(Error::NoUtxosSelected);
        }

        // we keep it as a float while we accumulate it, and only round it at the end
        let mut outgoing: u64 = 0;
        let mut received: u64 = 0;

        let calc_fee_bytes = |wu| (wu as f32) * fee_rate.as_sat_vb() / 4.0;
        fee_amount += calc_fee_bytes(tx.get_weight());

        for (index, (script_pubkey, satoshi)) in recipients.into_iter().enumerate() {
            let value = match builder.single_recipient {
                Some(_) => 0,
                None if satoshi.is_dust() => return Err(Error::OutputBelowDustLimit(index)),
                None => satoshi,
            };

            if self.is_mine(script_pubkey)? {
                received += value;
            }

            let new_out = TxOut {
                script_pubkey: script_pubkey.clone(),
                value,
            };
            fee_amount += calc_fee_bytes(serialize(&new_out).len() * 4);

            tx.output.push(new_out);

            outgoing += value;
        }

        if builder.change_policy != tx_builder::ChangeSpendPolicy::ChangeAllowed
            && self.change_descriptor.is_none()
        {
            return Err(Error::Generic(
                "The `change_policy` can be set only if the wallet has a change_descriptor".into(),
            ));
        }

        let (required_utxos, optional_utxos) = self.preselect_utxos(
            builder.change_policy,
            &builder.unspendable,
            &builder.utxos,
            builder.drain_wallet,
            builder.manually_selected_only,
            false, // we don't mind using unconfirmed outputs here, hopefully coin selection will sort this out?
        )?;

        let coin_selection::CoinSelectionResult {
            selected,
            selected_amount,
            mut fee_amount,
        } = builder.coin_selection.coin_select(
            self.database.borrow().deref(),
            required_utxos,
            optional_utxos,
            fee_rate,
            outgoing,
            fee_amount,
        )?;
        tx.input = selected
            .iter()
            .map(|u| bitcoin::TxIn {
                previous_output: u.outpoint,
                script_sig: Script::default(),
                sequence: n_sequence,
                witness: vec![],
            })
            .collect();

        // prepare the change output
        let change_output = match builder.single_recipient {
            Some(_) => None,
            None => {
                let change_script = self.get_change_address()?;
                let change_output = TxOut {
                    script_pubkey: change_script,
                    value: 0,
                };

                // take the change into account for fees
                fee_amount += calc_fee_bytes(serialize(&change_output).len() * 4);
                Some(change_output)
            }
        };

        let mut fee_amount = fee_amount.ceil() as u64;
        let change_val = (selected_amount - outgoing).saturating_sub(fee_amount);

        match change_output {
            None if change_val.is_dust() => {
                // single recipient, but the only output would be below dust limit
                return Err(Error::InsufficientFunds); // TODO: or OutputBelowDustLimit?
            }
            Some(_) if change_val.is_dust() => {
                // skip the change output because it's dust, this adds up to the fees
                fee_amount += selected_amount - outgoing;
            }
            Some(mut change_output) => {
                change_output.value = change_val;
                received += change_val;

                tx.output.push(change_output);
            }
            None => {
                // there's only one output, send everything to it
                tx.output[0].value = change_val;

                // the single recipient is our address
                if self.is_mine(&tx.output[0].script_pubkey)? {
                    received = change_val;
                }
            }
        }

        // sort input/outputs according to the chosen algorithm
        builder.ordering.sort_tx(&mut tx);

        let txid = tx.txid();
        let psbt = self.complete_transaction(tx, selected, builder)?;

        let transaction_details = TransactionDetails {
            transaction: None,
            txid,
            timestamp: time::get_timestamp(),
            received,
            sent: selected_amount,
            fees: fee_amount,
            height: None,
        };

        Ok((psbt, transaction_details))
    }

    /// Bump the fee of a transaction following the options specified in the `builder`
    ///
    /// Return an error if the transaction is already confirmed or doesn't explicitly signal RBF.
    ///
    /// **NOTE**: if the original transaction was made with [`TxBuilder::set_single_recipient`],
    /// the [`TxBuilder::maintain_single_recipient`] flag should be enabled to correctly reduce the
    /// only output's value in order to increase the fees.
    ///
    /// If the `builder` specifies some `utxos` that must be spent, they will be added to the
    /// transaction regardless of whether they are necessary or not to cover additional fees.
    ///
    /// ## Example
    ///
    /// ```no_run
    /// # use std::str::FromStr;
    /// # use bitcoin::*;
    /// # use bdk::*;
    /// # use bdk::database::*;
    /// # let descriptor = "wpkh(tpubD6NzVbkrYhZ4Xferm7Pz4VnjdcDPFyjVu5K4iZXQ4pVN8Cks4pHVowTBXBKRhX64pkRyJZJN5xAKj4UDNnLPb5p2sSKXhewoYx5GbTdUFWq/*)";
    /// # let wallet: OfflineWallet<_> = Wallet::new_offline(descriptor, None, Network::Testnet, MemoryDatabase::default())?;
    /// let txid = Txid::from_str("faff0a466b70f5d5f92bd757a92c1371d4838bdd5bc53a06764e2488e51ce8f8").unwrap();
    /// let (psbt, details) = wallet.bump_fee(
    ///     &txid,
    ///     TxBuilder::new().fee_rate(FeeRate::from_sat_per_vb(5.0)),
    /// )?;
    /// // sign and broadcast ...
    /// # Ok::<(), bdk::Error>(())
    /// ```
    // TODO: support for merging multiple transactions while bumping the fees
    // TODO: option to force addition of an extra output? seems bad for privacy to update the
    // change
    pub fn bump_fee<Cs: coin_selection::CoinSelectionAlgorithm<D>>(
        &self,
        txid: &Txid,
        builder: TxBuilder<D, Cs, BumpFee>,
    ) -> Result<(PSBT, TransactionDetails), Error> {
        let mut details = match self.database.borrow().get_tx(&txid, true)? {
            None => return Err(Error::TransactionNotFound),
            Some(tx) if tx.transaction.is_none() => return Err(Error::TransactionNotFound),
            Some(tx) if tx.height.is_some() => return Err(Error::TransactionConfirmed),
            Some(tx) => tx,
        };
        let mut tx = details.transaction.take().unwrap();
        if !tx.input.iter().any(|txin| txin.sequence <= 0xFFFFFFFD) {
            return Err(Error::IrreplaceableTransaction);
        }

        // the new tx must "pay for its bandwidth"
        let vbytes = tx.get_weight() as f32 / 4.0;
        let required_feerate = FeeRate::from_sat_per_vb(details.fees as f32 / vbytes + 1.0);

        // find the index of the output that we can update. either the change or the only one if
        // it's `single_recipient`
        let updatable_output = match builder.single_recipient {
            Some(_) if tx.output.len() != 1 => return Err(Error::SingleRecipientMultipleOutputs),
            Some(_) => Some(0),
            None => {
                let mut change_output = None;
                for (index, txout) in tx.output.iter().enumerate() {
                    // look for an output that we know and that has the right ScriptType. We use
                    // `get_descriptor_for` to find what's the ScriptType for `Internal`
                    // addresses really is, because if there's no change_descriptor it's actually equal
                    // to "External"
                    let (_, change_type) =
                        self.get_descriptor_for_script_type(ScriptType::Internal);
                    match self
                        .database
                        .borrow()
                        .get_path_from_script_pubkey(&txout.script_pubkey)?
                    {
                        Some((script_type, _)) if script_type == change_type => {
                            change_output = Some(index);
                            break;
                        }
                        _ => {}
                    }
                }

                change_output
            }
        };
        let updatable_output = match updatable_output {
            Some(updatable_output) => updatable_output,
            None => {
                // we need a change output, add one here and take into account the extra fees for it
                let change_script = self.get_change_address()?;
                let change_txout = TxOut {
                    script_pubkey: change_script,
                    value: 0,
                };
                tx.output.push(change_txout);

                tx.output.len() - 1
            }
        };

        // initially always remove the output we can change
        let mut removed_updatable_output = tx.output.remove(updatable_output);
        if self.is_mine(&removed_updatable_output.script_pubkey)? {
            details.received -= removed_updatable_output.value;
        }

        let deriv_ctx = descriptor_to_pk_ctx(&self.secp);

        let external_weight = self
            .get_descriptor_for_script_type(ScriptType::External)
            .0
            .max_satisfaction_weight(deriv_ctx)
            .unwrap();
        let internal_weight = self
            .get_descriptor_for_script_type(ScriptType::Internal)
            .0
            .max_satisfaction_weight(deriv_ctx)
            .unwrap();

        let original_sequence = tx.input[0].sequence;

        // remove the inputs from the tx and process them
        let original_txin = tx.input.drain(..).collect::<Vec<_>>();
        let mut original_utxos = original_txin
            .iter()
            .map(|txin| -> Result<(UTXO, usize), Error> {
                let txout = self
                    .database
                    .borrow()
                    .get_previous_output(&txin.previous_output)?
                    .ok_or(Error::UnknownUTXO)?;

                let (weight, is_internal) = match self
                    .database
                    .borrow()
                    .get_path_from_script_pubkey(&txout.script_pubkey)?
                {
                    Some((ScriptType::Internal, _)) => (internal_weight, true),
                    Some((ScriptType::External, _)) => (external_weight, false),
                    None => {
                        // estimate the weight based on the scriptsig/witness size present in the
                        // original transaction
                        let weight =
                            serialize(&txin.script_sig).len() * 4 + serialize(&txin.witness).len();
                        (weight, false)
                    }
                };

                let utxo = UTXO {
                    outpoint: txin.previous_output,
                    txout,
                    is_internal,
                };

                Ok((utxo, weight))
            })
            .collect::<Result<Vec<_>, _>>()?;

        if builder.manually_selected_only && builder.utxos.is_empty() {
            return Err(Error::NoUtxosSelected);
        }

        let builder_extra_utxos = builder
            .utxos
            .iter()
            .filter(|utxo| {
                !original_txin
                    .iter()
                    .any(|txin| &&txin.previous_output == utxo)
            })
            .cloned()
            .collect::<Vec<_>>();

        let (mut required_utxos, optional_utxos) = self.preselect_utxos(
            builder.change_policy,
            &builder.unspendable,
            &builder_extra_utxos[..],
            builder.drain_wallet,
            builder.manually_selected_only,
            true, // we only want confirmed transactions for RBF
        )?;

        required_utxos.append(&mut original_utxos);

        let amount_needed = tx.output.iter().fold(0, |acc, out| acc + out.value);
        let (new_feerate, initial_fee) = match builder
            .fee_policy
            .as_ref()
            .unwrap_or(&FeePolicy::FeeRate(FeeRate::default()))
        {
            FeePolicy::FeeAmount(amount) => {
                if *amount < details.fees {
                    return Err(Error::FeeTooLow {
                        required: details.fees,
                    });
                }
                (FeeRate::from_sat_per_vb(0.0), *amount as f32)
            }
            FeePolicy::FeeRate(rate) => {
                if *rate < required_feerate {
                    return Err(Error::FeeRateTooLow {
                        required: required_feerate,
                    });
                }
                (*rate, tx.get_weight() as f32 / 4.0 * rate.as_sat_vb())
            }
        };

        let coin_selection::CoinSelectionResult {
            selected,
            selected_amount,
            fee_amount,
        } = builder.coin_selection.coin_select(
            self.database.borrow().deref(),
            required_utxos,
            optional_utxos,
            new_feerate,
            amount_needed,
            initial_fee,
        )?;

        tx.input = selected
            .iter()
            .map(|u| bitcoin::TxIn {
                previous_output: u.outpoint,
                script_sig: Script::default(),
                // TODO: use builder.n_sequence??
                sequence: original_sequence,
                witness: vec![],
            })
            .collect();

        details.sent = selected_amount;

        let mut fee_amount = fee_amount.ceil() as u64;
        let removed_output_fee_cost = (serialize(&removed_updatable_output).len() as f32
            * new_feerate.as_sat_vb())
        .ceil() as u64;

        let change_val = selected_amount - amount_needed - fee_amount;
        let change_val_after_add = change_val.saturating_sub(removed_output_fee_cost);
        match builder.single_recipient {
            None if change_val_after_add.is_dust() => {
                // skip the change output because it's dust, this adds up to the fees
                fee_amount += change_val;
            }
            Some(_) if change_val_after_add.is_dust() => {
                // single_recipient but the only output would be below dust limit
                return Err(Error::InsufficientFunds); // TODO: or OutputBelowDustLimit?
            }
            None => {
                removed_updatable_output.value = change_val_after_add;
                fee_amount += removed_output_fee_cost;
                details.received += change_val_after_add;

                tx.output.push(removed_updatable_output);
            }
            Some(_) => {
                removed_updatable_output.value = change_val_after_add;
                fee_amount += removed_output_fee_cost;

                // single recipient and it's our address
                if self.is_mine(&removed_updatable_output.script_pubkey)? {
                    details.received = change_val_after_add;
                }

                tx.output.push(removed_updatable_output);
            }
        }

        // sort input/outputs according to the chosen algorithm
        builder.ordering.sort_tx(&mut tx);

        // TODO: check that we are not replacing more than 100 txs from mempool

        details.txid = tx.txid();
        details.fees = fee_amount;
        details.timestamp = time::get_timestamp();

        let psbt = self.complete_transaction(tx, selected, builder)?;

        Ok((psbt, details))
    }

    /// Sign a transaction with all the wallet's signers, in the order specified by every signer's
    /// [`SignerOrdering`]
    ///
    /// ## Example
    ///
    /// ```no_run
    /// # use std::str::FromStr;
    /// # use bitcoin::*;
    /// # use bdk::*;
    /// # use bdk::database::*;
    /// # let descriptor = "wpkh(tpubD6NzVbkrYhZ4Xferm7Pz4VnjdcDPFyjVu5K4iZXQ4pVN8Cks4pHVowTBXBKRhX64pkRyJZJN5xAKj4UDNnLPb5p2sSKXhewoYx5GbTdUFWq/*)";
    /// # let wallet: OfflineWallet<_> = Wallet::new_offline(descriptor, None, Network::Testnet, MemoryDatabase::default())?;
    /// # let (psbt, _) = wallet.create_tx(TxBuilder::new())?;
    /// let (signed_psbt, finalized) = wallet.sign(psbt, None)?;
    /// # Ok::<(), bdk::Error>(())
    pub fn sign(&self, mut psbt: PSBT, assume_height: Option<u32>) -> Result<(PSBT, bool), Error> {
        // this helps us doing our job later
        self.add_input_hd_keypaths(&mut psbt)?;

        for signer in self
            .signers
            .signers()
            .iter()
            .chain(self.change_signers.signers().iter())
        {
            if signer.sign_whole_tx() {
                signer.sign(&mut psbt, None, &self.secp)?;
            } else {
                for index in 0..psbt.inputs.len() {
                    signer.sign(&mut psbt, Some(index), &self.secp)?;
                }
            }
        }

        // attempt to finalize
        self.finalize_psbt(psbt, assume_height)
    }

    /// Return the spending policies for the wallet's descriptor
    pub fn policies(&self, script_type: ScriptType) -> Result<Option<Policy>, Error> {
        match (script_type, self.change_descriptor.as_ref()) {
            (ScriptType::External, _) => {
                Ok(self.descriptor.extract_policy(&self.signers, &self.secp)?)
            }
            (ScriptType::Internal, None) => Ok(None),
            (ScriptType::Internal, Some(desc)) => {
                Ok(desc.extract_policy(&self.change_signers, &self.secp)?)
            }
        }
    }

    /// Return the "public" version of the wallet's descriptor, meaning a new descriptor that has
    /// the same structure but with every secret key removed
    ///
    /// This can be used to build a watch-only version of a wallet
    pub fn public_descriptor(
        &self,
        script_type: ScriptType,
    ) -> Result<Option<ExtendedDescriptor>, Error> {
        match (script_type, self.change_descriptor.as_ref()) {
            (ScriptType::External, _) => Ok(Some(self.descriptor.clone())),
            (ScriptType::Internal, None) => Ok(None),
            (ScriptType::Internal, Some(desc)) => Ok(Some(desc.clone())),
        }
    }

    /// Try to finalize a PSBT
    pub fn finalize_psbt(
        &self,
        mut psbt: PSBT,
        assume_height: Option<u32>,
    ) -> Result<(PSBT, bool), Error> {
        let tx = &psbt.global.unsigned_tx;
        let mut finished = true;

        for (n, input) in tx.input.iter().enumerate() {
            let psbt_input = &psbt.inputs[n];
            if psbt_input.final_script_sig.is_some() || psbt_input.final_script_witness.is_some() {
                continue;
            }
            // if the height is None in the database it means it's still unconfirmed, so consider
            // that as a very high value
            let create_height = self
                .database
                .borrow()
                .get_tx(&input.previous_output.txid, false)?
                .map(|tx| tx.height.unwrap_or(std::u32::MAX));
            let current_height = assume_height.or(self.current_height);

            debug!(
                "Input #{} - {}, using `create_height` = {:?}, `current_height` = {:?}",
                n, input.previous_output, create_height, current_height
            );

            // - Try to derive the descriptor by looking at the txout. If it's in our database, we
            //   know exactly which `script_type` to use, and which derivation index it is
            // - If that fails, try to derive it by looking at the psbt input: the complete logic
            //   is in `src/descriptor/mod.rs`, but it will basically look at `hd_keypaths`,
            //   `redeem_script` and `witness_script` to determine the right derivation
            // - If that also fails, it will try it on the internal descriptor, if present
            let desc = psbt
                .get_utxo_for(n)
                .map(|txout| self.get_descriptor_for_txout(&txout))
                .transpose()?
                .flatten()
                .or_else(|| {
                    self.descriptor.derive_from_psbt_input(
                        psbt_input,
                        psbt.get_utxo_for(n),
                        &self.secp,
                    )
                })
                .or_else(|| {
                    self.change_descriptor.as_ref().and_then(|desc| {
                        desc.derive_from_psbt_input(psbt_input, psbt.get_utxo_for(n), &self.secp)
                    })
                });

            match desc {
                Some(desc) => {
                    let mut tmp_input = bitcoin::TxIn::default();
                    let deriv_ctx = descriptor_to_pk_ctx(&self.secp);
                    match desc.satisfy(
                        &mut tmp_input,
                        (
                            PsbtInputSatisfier::new(&psbt, n),
                            After::new(current_height, false),
                            Older::new(current_height, create_height, false),
                        ),
                        deriv_ctx,
                    ) {
                        Ok(_) => {
                            let psbt_input = &mut psbt.inputs[n];
                            psbt_input.final_script_sig = Some(tmp_input.script_sig);
                            psbt_input.final_script_witness = Some(tmp_input.witness);
                        }
                        Err(e) => {
                            debug!("satisfy error {:?} for input {}", e, n);
                            finished = false
                        }
                    }
                }
                None => finished = false,
            }
        }

        Ok((psbt, finished))
    }

    pub fn secp_ctx(&self) -> &SecpCtx {
        &self.secp
    }

    // Internals

    fn get_descriptor_for_script_type(
        &self,
        script_type: ScriptType,
    ) -> (&ExtendedDescriptor, ScriptType) {
        match script_type {
            ScriptType::Internal if self.change_descriptor.is_some() => (
                self.change_descriptor.as_ref().unwrap(),
                ScriptType::Internal,
            ),
            _ => (&self.descriptor, ScriptType::External),
        }
    }

    fn get_descriptor_for_txout(&self, txout: &TxOut) -> Result<Option<ExtendedDescriptor>, Error> {
        Ok(self
            .database
            .borrow()
            .get_path_from_script_pubkey(&txout.script_pubkey)?
            .map(|(script_type, child)| (self.get_descriptor_for_script_type(script_type).0, child))
            .map(|(desc, child)| desc.derive(ChildNumber::from_normal_idx(child).unwrap())))
    }

    fn get_change_address(&self) -> Result<Script, Error> {
        let deriv_ctx = descriptor_to_pk_ctx(&self.secp);

        let (desc, script_type) = self.get_descriptor_for_script_type(ScriptType::Internal);
        let index = self.fetch_and_increment_index(script_type)?;

        Ok(desc
            .derive(ChildNumber::from_normal_idx(index)?)
            .script_pubkey(deriv_ctx))
    }

    fn fetch_and_increment_index(&self, script_type: ScriptType) -> Result<u32, Error> {
        let (descriptor, script_type) = self.get_descriptor_for_script_type(script_type);
        let index = match descriptor.is_fixed() {
            true => 0,
            false => self
                .database
                .borrow_mut()
                .increment_last_index(script_type)?,
        };

        if self
            .database
            .borrow()
            .get_script_pubkey_from_path(script_type, index)?
            .is_none()
        {
            self.cache_addresses(script_type, index, CACHE_ADDR_BATCH_SIZE)?;
        }

        let deriv_ctx = descriptor_to_pk_ctx(&self.secp);

        let hd_keypaths = descriptor.get_hd_keypaths(index, &self.secp)?;
        let script = descriptor
            .derive(ChildNumber::from_normal_idx(index)?)
            .script_pubkey(deriv_ctx);
        for validator in &self.address_validators {
            validator.validate(script_type, &hd_keypaths, &script)?;
        }

        Ok(index)
    }

    fn cache_addresses(
        &self,
        script_type: ScriptType,
        from: u32,
        mut count: u32,
    ) -> Result<(), Error> {
        let (descriptor, script_type) = self.get_descriptor_for_script_type(script_type);
        if descriptor.is_fixed() {
            if from > 0 {
                return Ok(());
            }

            count = 1;
        }

        let deriv_ctx = descriptor_to_pk_ctx(&self.secp);

        let mut address_batch = self.database.borrow().begin_batch();

        let start_time = time::Instant::new();
        for i in from..(from + count) {
            address_batch.set_script_pubkey(
                &descriptor
                    .derive(ChildNumber::from_normal_idx(i)?)
                    .script_pubkey(deriv_ctx),
                script_type,
                i,
            )?;
        }

        info!(
            "Derivation of {} addresses from {} took {} ms",
            count,
            from,
            start_time.elapsed().as_millis()
        );

        self.database.borrow_mut().commit_batch(address_batch)?;

        Ok(())
    }

    fn get_available_utxos(&self) -> Result<Vec<(UTXO, usize)>, Error> {
        let deriv_ctx = descriptor_to_pk_ctx(&self.secp);

        let external_weight = self
            .get_descriptor_for_script_type(ScriptType::External)
            .0
            .max_satisfaction_weight(deriv_ctx)
            .unwrap();
        let internal_weight = self
            .get_descriptor_for_script_type(ScriptType::Internal)
            .0
            .max_satisfaction_weight(deriv_ctx)
            .unwrap();

        let add_weight = |utxo: UTXO| {
            let weight = match utxo.is_internal {
                true => internal_weight,
                false => external_weight,
            };

            (utxo, weight)
        };

        let utxos = self.list_unspent()?.into_iter().map(add_weight).collect();

        Ok(utxos)
    }

    /// Given the options returns the list of utxos that must be used to form the
    /// transaction and any further that may be used if needed.
    #[allow(clippy::type_complexity)]
    fn preselect_utxos(
        &self,
        change_policy: tx_builder::ChangeSpendPolicy,
        unspendable: &HashSet<OutPoint>,
        manually_selected: &[OutPoint],
        must_use_all_available: bool,
        manual_only: bool,
        must_only_use_confirmed_tx: bool,
    ) -> Result<(Vec<(UTXO, usize)>, Vec<(UTXO, usize)>), Error> {
        //    must_spend <- manually selected utxos
        //    may_spend  <- all other available utxos
        let mut may_spend = self.get_available_utxos()?;
        let mut must_spend = {
            let must_spend_idx = manually_selected
                .iter()
                .map(|manually_selected| {
                    may_spend
                        .iter()
                        .position(|available| available.0.outpoint == *manually_selected)
                        .ok_or(Error::UnknownUTXO)
                })
                .collect::<Result<Vec<_>, _>>()?;

            must_spend_idx
                .into_iter()
                .map(|i| may_spend.remove(i))
                .collect()
        };

        // NOTE: we are intentionally ignoring `unspendable` here. i.e manual
        // selection overrides unspendable.
        if manual_only {
            return Ok((must_spend, vec![]));
        }

        let satisfies_confirmed = match must_only_use_confirmed_tx {
            true => {
                let database = self.database.borrow_mut();
                may_spend
                    .iter()
                    .map(|u| {
                        database
                            .get_tx(&u.0.outpoint.txid, true)
                            .map(|tx| match tx {
                                None => false,
                                Some(tx) => tx.height.is_some(),
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
            false => vec![true; may_spend.len()],
        };

        let mut i = 0;
        may_spend.retain(|u| {
            let retain = change_policy.is_satisfied_by(&u.0)
                && !unspendable.contains(&u.0.outpoint)
                && satisfies_confirmed[i];
            i += 1;
            retain
        });

        if must_use_all_available {
            must_spend.append(&mut may_spend);
        }

        Ok((must_spend, may_spend))
    }

    fn complete_transaction<
        Cs: coin_selection::CoinSelectionAlgorithm<D>,
        Ctx: TxBuilderContext,
    >(
        &self,
        tx: Transaction,
        selected: Vec<UTXO>,
        builder: TxBuilder<D, Cs, Ctx>,
    ) -> Result<PSBT, Error> {
        let mut psbt = PSBT::from_unsigned_tx(tx)?;
        let lookup_output = selected
            .into_iter()
            .map(|utxo| (utxo.outpoint, utxo))
            .collect::<HashMap<_, _>>();

        // add metadata for the inputs
        for (psbt_input, input) in psbt
            .inputs
            .iter_mut()
            .zip(psbt.global.unsigned_tx.input.iter())
        {
            let utxo = match lookup_output.get(&input.previous_output) {
                Some(utxo) => utxo,
                None => continue,
            };

            // Only set it if the builder has a custom one, otherwise leave blank which defaults to
            // SIGHASH_ALL
            if let Some(sighash_type) = builder.sighash {
                psbt_input.sighash_type = Some(sighash_type);
            }

            // Try to find the prev_script in our db to figure out if this is internal or external,
            // and the derivation index
            let (script_type, child) = match self
                .database
                .borrow()
                .get_path_from_script_pubkey(&utxo.txout.script_pubkey)?
            {
                Some(x) => x,
                None => continue,
            };

            let (desc, _) = self.get_descriptor_for_script_type(script_type);
            psbt_input.hd_keypaths = desc.get_hd_keypaths(child, &self.secp)?;
            let derived_descriptor = desc.derive(ChildNumber::from_normal_idx(child)?);

            psbt_input.redeem_script = derived_descriptor.psbt_redeem_script(&self.secp);
            psbt_input.witness_script = derived_descriptor.psbt_witness_script(&self.secp);

            let prev_output = input.previous_output;
            if let Some(prev_tx) = self.database.borrow().get_raw_tx(&prev_output.txid)? {
                if derived_descriptor.is_witness() {
                    psbt_input.witness_utxo =
                        Some(prev_tx.output[prev_output.vout as usize].clone());
                }
                if !derived_descriptor.is_witness() || builder.force_non_witness_utxo {
                    psbt_input.non_witness_utxo = Some(prev_tx);
                }
            }
        }

        // probably redundant but it doesn't hurt...
        self.add_input_hd_keypaths(&mut psbt)?;

        // add metadata for the outputs
        for (psbt_output, tx_output) in psbt
            .outputs
            .iter_mut()
            .zip(psbt.global.unsigned_tx.output.iter())
        {
            if let Some((script_type, child)) = self
                .database
                .borrow()
                .get_path_from_script_pubkey(&tx_output.script_pubkey)?
            {
                let (desc, _) = self.get_descriptor_for_script_type(script_type);
                psbt_output.hd_keypaths = desc.get_hd_keypaths(child, &self.secp)?;
                if builder.include_output_redeem_witness_script {
                    let derived_descriptor = desc.derive(ChildNumber::from_normal_idx(child)?);
                    psbt_output.witness_script = derived_descriptor.psbt_witness_script(&self.secp);
                    psbt_output.redeem_script = derived_descriptor.psbt_redeem_script(&self.secp);
                };
            }
        }

        Ok(psbt)
    }

    fn add_input_hd_keypaths(&self, psbt: &mut PSBT) -> Result<(), Error> {
        let mut input_utxos = Vec::with_capacity(psbt.inputs.len());
        for n in 0..psbt.inputs.len() {
            input_utxos.push(psbt.get_utxo_for(n).clone());
        }

        // try to add hd_keypaths if we've already seen the output
        for (psbt_input, out) in psbt.inputs.iter_mut().zip(input_utxos.iter()) {
            if let Some(out) = out {
                if let Some((script_type, child)) = self
                    .database
                    .borrow()
                    .get_path_from_script_pubkey(&out.script_pubkey)?
                {
                    debug!("Found descriptor {:?}/{}", script_type, child);

                    // merge hd_keypaths
                    let (desc, _) = self.get_descriptor_for_script_type(script_type);
                    let mut hd_keypaths = desc.get_hd_keypaths(child, &self.secp)?;
                    psbt_input.hd_keypaths.append(&mut hd_keypaths);
                }
            }
        }

        Ok(())
    }
}

impl<B, D> Wallet<B, D>
where
    B: Blockchain,
    D: BatchDatabase,
{
    /// Create a new "online" wallet
    #[maybe_async]
    pub fn new<E: ToWalletDescriptor>(
        descriptor: E,
        change_descriptor: Option<E>,
        network: Network,
        database: D,
        client: B,
    ) -> Result<Self, Error> {
        let mut wallet = Self::new_offline(descriptor, change_descriptor, network, database)?;

        wallet.current_height = Some(maybe_await!(client.get_height())? as u32);
        wallet.client = Some(client);

        Ok(wallet)
    }

    /// Sync the internal database with the blockchain
    #[maybe_async]
    pub fn sync<P: 'static + Progress>(
        &self,
        progress_update: P,
        max_address_param: Option<u32>,
    ) -> Result<(), Error> {
        debug!("Begin sync...");

        let mut run_setup = false;

        let max_address = match self.descriptor.is_fixed() {
            true => 0,
            false => max_address_param.unwrap_or(CACHE_ADDR_BATCH_SIZE),
        };
        if self
            .database
            .borrow()
            .get_script_pubkey_from_path(ScriptType::External, max_address.saturating_sub(1))?
            .is_none()
        {
            run_setup = true;
            self.cache_addresses(ScriptType::External, 0, max_address)?;
        }

        if let Some(change_descriptor) = &self.change_descriptor {
            let max_address = match change_descriptor.is_fixed() {
                true => 0,
                false => max_address_param.unwrap_or(CACHE_ADDR_BATCH_SIZE),
            };

            if self
                .database
                .borrow()
                .get_script_pubkey_from_path(ScriptType::Internal, max_address.saturating_sub(1))?
                .is_none()
            {
                run_setup = true;
                self.cache_addresses(ScriptType::Internal, 0, max_address)?;
            }
        }

        // TODO: what if i generate an address first and cache some addresses?
        // TODO: we should sync if generating an address triggers a new batch to be stored
        if run_setup {
            maybe_await!(self.client.as_ref().ok_or(Error::OfflineClient)?.setup(
                None,
                self.database.borrow_mut().deref_mut(),
                progress_update,
            ))
        } else {
            maybe_await!(self.client.as_ref().ok_or(Error::OfflineClient)?.sync(
                None,
                self.database.borrow_mut().deref_mut(),
                progress_update,
            ))
        }
    }

    /// Return a reference to the internal blockchain client
    pub fn client(&self) -> Option<&B> {
        self.client.as_ref()
    }

    /// Broadcast a transaction to the network
    #[maybe_async]
    pub fn broadcast(&self, tx: Transaction) -> Result<Txid, Error> {
        maybe_await!(self
            .client
            .as_ref()
            .ok_or(Error::OfflineClient)?
            .broadcast(&tx))?;

        Ok(tx.txid())
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use bitcoin::Network;

    use crate::database::memory::MemoryDatabase;
    use crate::database::Database;
    use crate::types::ScriptType;

    use super::*;

    #[test]
    fn test_cache_addresses_fixed() {
        let db = MemoryDatabase::new();
        let wallet: OfflineWallet<_> = Wallet::new_offline(
            "wpkh(L5EZftvrYaSudiozVRzTqLcHLNDoVn7H5HSfM9BAN6tMJX8oTWz6)",
            None,
            Network::Testnet,
            db,
        )
        .unwrap();

        assert_eq!(
            wallet.get_new_address().unwrap().to_string(),
            "tb1qj08ys4ct2hzzc2hcz6h2hgrvlmsjynaw43s835"
        );
        assert_eq!(
            wallet.get_new_address().unwrap().to_string(),
            "tb1qj08ys4ct2hzzc2hcz6h2hgrvlmsjynaw43s835"
        );

        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(ScriptType::External, 0)
            .unwrap()
            .is_some());
        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(ScriptType::Internal, 0)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_cache_addresses() {
        let db = MemoryDatabase::new();
        let wallet: OfflineWallet<_> = Wallet::new_offline("wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)", None, Network::Testnet, db).unwrap();

        assert_eq!(
            wallet.get_new_address().unwrap().to_string(),
            "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
        );
        assert_eq!(
            wallet.get_new_address().unwrap().to_string(),
            "tb1q4er7kxx6sssz3q7qp7zsqsdx4erceahhax77d7"
        );

        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(ScriptType::External, CACHE_ADDR_BATCH_SIZE - 1)
            .unwrap()
            .is_some());
        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(ScriptType::External, CACHE_ADDR_BATCH_SIZE)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_cache_addresses_refill() {
        let db = MemoryDatabase::new();
        let wallet: OfflineWallet<_> = Wallet::new_offline("wpkh(tpubEBr4i6yk5nf5DAaJpsi9N2pPYBeJ7fZ5Z9rmN4977iYLCGco1VyjB9tvvuvYtfZzjD5A8igzgw3HeWeeKFmanHYqksqZXYXGsw5zjnj7KM9/*)", None, Network::Testnet, db).unwrap();

        assert_eq!(
            wallet.get_new_address().unwrap().to_string(),
            "tb1q6yn66vajcctph75pvylgkksgpp6nq04ppwct9a"
        );
        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(ScriptType::External, CACHE_ADDR_BATCH_SIZE - 1)
            .unwrap()
            .is_some());

        for _ in 0..CACHE_ADDR_BATCH_SIZE {
            wallet.get_new_address().unwrap();
        }

        assert!(wallet
            .database
            .borrow_mut()
            .get_script_pubkey_from_path(ScriptType::External, CACHE_ADDR_BATCH_SIZE * 2 - 1)
            .unwrap()
            .is_some());
    }

    pub(crate) fn get_test_wpkh() -> &'static str {
        "wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)"
    }

    pub(crate) fn get_test_single_sig_csv() -> &'static str {
        // and(pk(Alice),older(6))
        "wsh(and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),older(6)))"
    }

    pub(crate) fn get_test_a_or_b_plus_csv() -> &'static str {
        // or(pk(Alice),and(pk(Bob),older(144)))
        "wsh(or_d(pk(cRjo6jqfVNP33HhSS76UhXETZsGTZYx8FMFvR9kpbtCSV1PmdZdu),and_v(v:pk(cMnkdebixpXMPfkcNEjjGin7s94hiehAH4mLbYkZoh9KSiNNmqC8),older(144))))"
    }

    pub(crate) fn get_test_single_sig_cltv() -> &'static str {
        // and(pk(Alice),after(100000))
        "wsh(and_v(v:pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW),after(100000)))"
    }

    pub(crate) fn get_funded_wallet(
        descriptor: &str,
    ) -> (
        OfflineWallet<MemoryDatabase>,
        (String, Option<String>),
        bitcoin::Txid,
    ) {
        let descriptors = testutils!(@descriptors (descriptor));
        let wallet: OfflineWallet<_> = Wallet::new_offline(
            &descriptors.0,
            None,
            Network::Regtest,
            MemoryDatabase::new(),
        )
        .unwrap();

        let txid = wallet.database.borrow_mut().received_tx(
            testutils! {
                @tx ( (@external descriptors, 0) => 50_000 ) (@confirmations 1)
            },
            Some(100),
        );

        (wallet, descriptors, txid)
    }

    macro_rules! assert_fee_rate {
        ($tx:expr, $fees:expr, $fee_rate:expr $( ,@dust_change $( $dust_change:expr )* )* $( ,@add_signature $( $add_signature:expr )* )* ) => ({
            let mut tx = $tx.clone();
            $(
                $( $add_signature )*
                for txin in &mut tx.input {
                    txin.witness.push([0x00; 108].to_vec()); // fake signature
                }
            )*

            #[allow(unused_mut)]
            #[allow(unused_assignments)]
            let mut dust_change = false;
            $(
                $( $dust_change )*
                dust_change = true;
            )*

            let tx_fee_rate = $fees as f32 / (tx.get_weight() as f32 / 4.0);
            let fee_rate = $fee_rate.as_sat_vb();

            if !dust_change {
                assert!((tx_fee_rate - fee_rate).abs() < 0.5, format!("Expected fee rate of {}, the tx has {}", fee_rate, tx_fee_rate));
            } else {
                assert!(tx_fee_rate >= fee_rate, format!("Expected fee rate of at least {}, the tx has {}", fee_rate, tx_fee_rate));
            }
        });
    }

    #[test]
    #[should_panic(expected = "NoRecipients")]
    fn test_create_tx_empty_recipients() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        wallet
            .create_tx(TxBuilder::with_recipients(vec![]))
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "NoUtxosSelected")]
    fn test_create_tx_manually_selected_empty_utxos() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)])
                    .manually_selected_only()
                    .utxos(vec![]),
            )
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "Invalid version `0`")]
    fn test_create_tx_version_0() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        wallet
            .create_tx(TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)]).version(0))
            .unwrap();
    }

    #[test]
    #[should_panic(
        expected = "TxBuilder requested version `1`, but at least `2` is needed to use OP_CSV"
    )]
    fn test_create_tx_version_1_csv() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_csv());
        let addr = wallet.get_new_address().unwrap();
        wallet
            .create_tx(TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)]).version(1))
            .unwrap();
    }

    #[test]
    fn test_create_tx_custom_version() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)]).version(42))
            .unwrap();

        assert_eq!(psbt.global.unsigned_tx.version, 42);
    }

    #[test]
    fn test_create_tx_default_locktime() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(TxBuilder::with_recipients(vec![(
                addr.script_pubkey(),
                25_000,
            )]))
            .unwrap();

        assert_eq!(psbt.global.unsigned_tx.lock_time, 0);
    }

    #[test]
    fn test_create_tx_default_locktime_cltv() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_cltv());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(TxBuilder::with_recipients(vec![(
                addr.script_pubkey(),
                25_000,
            )]))
            .unwrap();

        assert_eq!(psbt.global.unsigned_tx.lock_time, 100_000);
    }

    #[test]
    fn test_create_tx_custom_locktime() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)]).nlocktime(630_000),
            )
            .unwrap();

        assert_eq!(psbt.global.unsigned_tx.lock_time, 630_000);
    }

    #[test]
    fn test_create_tx_custom_locktime_compatible_with_cltv() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_cltv());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)]).nlocktime(630_000),
            )
            .unwrap();

        assert_eq!(psbt.global.unsigned_tx.lock_time, 630_000);
    }

    #[test]
    #[should_panic(
        expected = "TxBuilder requested timelock of `50000`, but at least `100000` is required to spend from this script"
    )]
    fn test_create_tx_custom_locktime_incompatible_with_cltv() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_cltv());
        let addr = wallet.get_new_address().unwrap();
        wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)]).nlocktime(50000),
            )
            .unwrap();
    }

    #[test]
    fn test_create_tx_no_rbf_csv() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_csv());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(TxBuilder::with_recipients(vec![(
                addr.script_pubkey(),
                25_000,
            )]))
            .unwrap();

        assert_eq!(psbt.global.unsigned_tx.input[0].sequence, 6);
    }

    #[test]
    fn test_create_tx_with_default_rbf_csv() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_csv());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)]).enable_rbf(),
            )
            .unwrap();

        assert_eq!(psbt.global.unsigned_tx.input[0].sequence, 0xFFFFFFFD);
    }

    #[test]
    #[should_panic(
        expected = "Cannot enable RBF with nSequence `3`, since at least `6` is required to spend with OP_CSV"
    )]
    fn test_create_tx_with_custom_rbf_csv() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_csv());
        let addr = wallet.get_new_address().unwrap();
        wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)])
                    .enable_rbf_with_sequence(3),
            )
            .unwrap();
    }

    #[test]
    fn test_create_tx_no_rbf_cltv() {
        let (wallet, _, _) = get_funded_wallet(get_test_single_sig_cltv());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(TxBuilder::with_recipients(vec![(
                addr.script_pubkey(),
                25_000,
            )]))
            .unwrap();

        assert_eq!(psbt.global.unsigned_tx.input[0].sequence, 0xFFFFFFFE);
    }

    #[test]
    #[should_panic(expected = "Cannot enable RBF with a nSequence >= 0xFFFFFFFE")]
    fn test_create_tx_invalid_rbf_sequence() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)])
                    .enable_rbf_with_sequence(0xFFFFFFFE),
            )
            .unwrap();
    }

    #[test]
    fn test_create_tx_custom_rbf_sequence() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)])
                    .enable_rbf_with_sequence(0xDEADBEEF),
            )
            .unwrap();

        assert_eq!(psbt.global.unsigned_tx.input[0].sequence, 0xDEADBEEF);
    }

    #[test]
    fn test_create_tx_default_sequence() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(TxBuilder::with_recipients(vec![(
                addr.script_pubkey(),
                25_000,
            )]))
            .unwrap();

        assert_eq!(psbt.global.unsigned_tx.input[0].sequence, 0xFFFFFFFF);
    }

    #[test]
    #[should_panic(
        expected = "The `change_policy` can be set only if the wallet has a change_descriptor"
    )]
    fn test_create_tx_change_policy_no_internal() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)])
                    .do_not_spend_change(),
            )
            .unwrap();
    }

    #[test]
    fn test_create_tx_single_recipient_drain_wallet() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, details) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet(),
            )
            .unwrap();

        assert_eq!(psbt.global.unsigned_tx.output.len(), 1);
        assert_eq!(
            psbt.global.unsigned_tx.output[0].value,
            50_000 - details.fees
        );
    }

    #[test]
    fn test_create_tx_default_fee_rate() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, details) = wallet
            .create_tx(TxBuilder::with_recipients(vec![(
                addr.script_pubkey(),
                25_000,
            )]))
            .unwrap();

        assert_fee_rate!(psbt.extract_tx(), details.fees, FeeRate::default(), @add_signature);
    }

    #[test]
    fn test_create_tx_custom_fee_rate() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, details) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)])
                    .fee_rate(FeeRate::from_sat_per_vb(5.0)),
            )
            .unwrap();

        assert_fee_rate!(psbt.extract_tx(), details.fees, FeeRate::from_sat_per_vb(5.0), @add_signature);
    }

    #[test]
    fn test_create_tx_absolute_fee() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, details) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet()
                    .fee_absolute(100),
            )
            .unwrap();

        assert_eq!(details.fees, 100);
        assert_eq!(psbt.global.unsigned_tx.output.len(), 1);
        assert_eq!(
            psbt.global.unsigned_tx.output[0].value,
            50_000 - details.fees
        );
    }

    #[test]
    fn test_create_tx_absolute_zero_fee() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, details) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet()
                    .fee_absolute(0),
            )
            .unwrap();

        assert_eq!(details.fees, 0);
        assert_eq!(psbt.global.unsigned_tx.output.len(), 1);
        assert_eq!(
            psbt.global.unsigned_tx.output[0].value,
            50_000 - details.fees
        );
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_create_tx_absolute_high_fee() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (_psbt, _details) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet()
                    .fee_absolute(60_000),
            )
            .unwrap();
    }

    #[test]
    fn test_create_tx_add_change() {
        use super::tx_builder::TxOrdering;

        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, details) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)])
                    .ordering(TxOrdering::Untouched),
            )
            .unwrap();

        assert_eq!(psbt.global.unsigned_tx.output.len(), 2);
        assert_eq!(psbt.global.unsigned_tx.output[0].value, 25_000);
        assert_eq!(
            psbt.global.unsigned_tx.output[1].value,
            25_000 - details.fees
        );
    }

    #[test]
    fn test_create_tx_skip_change_dust() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(TxBuilder::with_recipients(vec![(
                addr.script_pubkey(),
                49_800,
            )]))
            .unwrap();

        assert_eq!(psbt.global.unsigned_tx.output.len(), 1);
        assert_eq!(psbt.global.unsigned_tx.output[0].value, 49_800);
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_create_tx_single_recipient_dust_amount() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        // very high fee rate, so that the only output would be below dust
        wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet()
                    .fee_rate(FeeRate::from_sat_per_vb(453.0)),
            )
            .unwrap();
    }

    #[test]
    fn test_create_tx_ordering_respected() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, details) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![
                    (addr.script_pubkey(), 30_000),
                    (addr.script_pubkey(), 10_000),
                ])
                .ordering(super::tx_builder::TxOrdering::BIP69Lexicographic),
            )
            .unwrap();

        assert_eq!(psbt.global.unsigned_tx.output.len(), 3);
        assert_eq!(
            psbt.global.unsigned_tx.output[0].value,
            10_000 - details.fees
        );
        assert_eq!(psbt.global.unsigned_tx.output[1].value, 10_000);
        assert_eq!(psbt.global.unsigned_tx.output[2].value, 30_000);
    }

    #[test]
    fn test_create_tx_default_sighash() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(TxBuilder::with_recipients(vec![(
                addr.script_pubkey(),
                30_000,
            )]))
            .unwrap();

        assert_eq!(psbt.inputs[0].sighash_type, None);
    }

    #[test]
    fn test_create_tx_custom_sighash() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 30_000)])
                    .sighash(bitcoin::SigHashType::Single),
            )
            .unwrap();

        assert_eq!(
            psbt.inputs[0].sighash_type,
            Some(bitcoin::SigHashType::Single)
        );
    }

    #[test]
    fn test_create_tx_input_hd_keypaths() {
        use bitcoin::util::bip32::{DerivationPath, Fingerprint};
        use std::str::FromStr;

        let (wallet, _, _) = get_funded_wallet("wpkh([d34db33f/44'/0'/0']tpubDEnoLuPdBep9bzw5LoGYpsxUQYheRQ9gcgrJhJEcdKFB9cWQRyYmkCyRoTqeD4tJYiVVgt6A3rN6rWn9RYhR9sBsGxji29LYWHuKKbdb1ev/0/*)");
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet(),
            )
            .unwrap();

        assert_eq!(psbt.inputs[0].hd_keypaths.len(), 1);
        assert_eq!(
            psbt.inputs[0].hd_keypaths.values().nth(0).unwrap(),
            &(
                Fingerprint::from_str("d34db33f").unwrap(),
                DerivationPath::from_str("m/44'/0'/0'/0/0").unwrap()
            )
        );
    }

    #[test]
    fn test_create_tx_output_hd_keypaths() {
        use bitcoin::util::bip32::{DerivationPath, Fingerprint};
        use std::str::FromStr;

        let (wallet, descriptors, _) = get_funded_wallet("wpkh([d34db33f/44'/0'/0']tpubDEnoLuPdBep9bzw5LoGYpsxUQYheRQ9gcgrJhJEcdKFB9cWQRyYmkCyRoTqeD4tJYiVVgt6A3rN6rWn9RYhR9sBsGxji29LYWHuKKbdb1ev/0/*)");
        // cache some addresses
        wallet.get_new_address().unwrap();

        let addr = testutils!(@external descriptors, 5);
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet(),
            )
            .unwrap();

        assert_eq!(psbt.outputs[0].hd_keypaths.len(), 1);
        assert_eq!(
            psbt.outputs[0].hd_keypaths.values().nth(0).unwrap(),
            &(
                Fingerprint::from_str("d34db33f").unwrap(),
                DerivationPath::from_str("m/44'/0'/0'/0/5").unwrap()
            )
        );
    }

    #[test]
    fn test_create_tx_set_redeem_script_p2sh() {
        use bitcoin::hashes::hex::FromHex;

        let (wallet, _, _) =
            get_funded_wallet("sh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet(),
            )
            .unwrap();

        assert_eq!(
            psbt.inputs[0].redeem_script,
            Some(Script::from(
                Vec::<u8>::from_hex(
                    "21032b0558078bec38694a84933d659303e2575dae7e91685911454115bfd64487e3ac"
                )
                .unwrap()
            ))
        );
        assert_eq!(psbt.inputs[0].witness_script, None);
    }

    #[test]
    fn test_create_tx_set_witness_script_p2wsh() {
        use bitcoin::hashes::hex::FromHex;

        let (wallet, _, _) =
            get_funded_wallet("wsh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet(),
            )
            .unwrap();

        assert_eq!(psbt.inputs[0].redeem_script, None);
        assert_eq!(
            psbt.inputs[0].witness_script,
            Some(Script::from(
                Vec::<u8>::from_hex(
                    "21032b0558078bec38694a84933d659303e2575dae7e91685911454115bfd64487e3ac"
                )
                .unwrap()
            ))
        );
    }

    #[test]
    fn test_create_tx_set_redeem_witness_script_p2wsh_p2sh() {
        use bitcoin::hashes::hex::FromHex;

        let (wallet, _, _) =
            get_funded_wallet("sh(wsh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)))");
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet(),
            )
            .unwrap();

        let script = Script::from(
            Vec::<u8>::from_hex(
                "21032b0558078bec38694a84933d659303e2575dae7e91685911454115bfd64487e3ac",
            )
            .unwrap(),
        );

        assert_eq!(psbt.inputs[0].redeem_script, Some(script.to_v0_p2wsh()));
        assert_eq!(psbt.inputs[0].witness_script, Some(script));
    }

    #[test]
    fn test_create_tx_non_witness_utxo() {
        let (wallet, _, _) =
            get_funded_wallet("sh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet(),
            )
            .unwrap();

        assert!(psbt.inputs[0].non_witness_utxo.is_some());
        assert!(psbt.inputs[0].witness_utxo.is_none());
    }

    #[test]
    fn test_create_tx_only_witness_utxo() {
        let (wallet, _, _) =
            get_funded_wallet("wsh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet(),
            )
            .unwrap();

        assert!(psbt.inputs[0].non_witness_utxo.is_none());
        assert!(psbt.inputs[0].witness_utxo.is_some());
    }

    #[test]
    fn test_create_tx_shwpkh_has_witness_utxo() {
        let (wallet, _, _) =
            get_funded_wallet("sh(wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet(),
            )
            .unwrap();

        assert!(psbt.inputs[0].non_witness_utxo.is_none());
        assert!(psbt.inputs[0].witness_utxo.is_some());
    }

    #[test]
    fn test_create_tx_both_non_witness_utxo_and_witness_utxo() {
        let (wallet, _, _) =
            get_funded_wallet("wsh(pk(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW))");
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet()
                    .force_non_witness_utxo(),
            )
            .unwrap();

        assert!(psbt.inputs[0].non_witness_utxo.is_some());
        assert!(psbt.inputs[0].witness_utxo.is_some());
    }

    #[test]
    fn test_create_tx_add_utxo() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        let small_output_txid = wallet.database.borrow_mut().received_tx(
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (psbt, details) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 30_000)]).add_utxo(
                    OutPoint {
                        txid: small_output_txid,
                        vout: 0,
                    },
                ),
            )
            .unwrap();

        assert_eq!(
            psbt.global.unsigned_tx.input.len(),
            2,
            "should add an additional input since 25_000 < 30_000"
        );
        assert_eq!(details.sent, 75_000, "total should be sum of both inputs");
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_create_tx_manually_selected_insufficient() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        let small_output_txid = wallet.database.borrow_mut().received_tx(
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 30_000)])
                    .add_utxo(OutPoint {
                        txid: small_output_txid,
                        vout: 0,
                    })
                    .manually_selected_only(),
            )
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "SpendingPolicyRequired(External)")]
    fn test_create_tx_policy_path_required() {
        let (wallet, _, _) = get_funded_wallet(get_test_a_or_b_plus_csv());

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        wallet
            .create_tx(TxBuilder::with_recipients(vec![(
                addr.script_pubkey(),
                30_000,
            )]))
            .unwrap();
    }

    #[test]
    fn test_create_tx_policy_path_no_csv() {
        let (wallet, _, _) = get_funded_wallet(get_test_a_or_b_plus_csv());

        let external_policy = wallet.policies(ScriptType::External).unwrap().unwrap();
        let root_id = external_policy.id;
        // child #0 is just the key "A"
        let path = vec![(root_id, vec![0])].into_iter().collect();

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 30_000)])
                    .policy_path(path, ScriptType::External),
            )
            .unwrap();

        assert_eq!(psbt.global.unsigned_tx.input[0].sequence, 0xFFFFFFFF);
    }

    #[test]
    fn test_create_tx_policy_path_use_csv() {
        let (wallet, _, _) = get_funded_wallet(get_test_a_or_b_plus_csv());

        let external_policy = wallet.policies(ScriptType::External).unwrap().unwrap();
        let root_id = external_policy.id;
        // child #1 is or(pk(B),older(144))
        let path = vec![(root_id, vec![1])].into_iter().collect();

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 30_000)])
                    .policy_path(path, ScriptType::External),
            )
            .unwrap();

        assert_eq!(psbt.global.unsigned_tx.input[0].sequence, 144);
    }

    #[test]
    #[should_panic(expected = "IrreplaceableTransaction")]
    fn test_bump_fee_irreplaceable_tx() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, mut details) = wallet
            .create_tx(TxBuilder::with_recipients(vec![(
                addr.script_pubkey(),
                25_000,
            )]))
            .unwrap();
        let tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the utxos, we know they can't be used anyways
        details.transaction = Some(tx);
        wallet.database.borrow_mut().set_tx(&details).unwrap();

        wallet.bump_fee(&txid, TxBuilder::new()).unwrap();
    }

    #[test]
    #[should_panic(expected = "TransactionConfirmed")]
    fn test_bump_fee_confirmed_tx() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, mut details) = wallet
            .create_tx(TxBuilder::with_recipients(vec![(
                addr.script_pubkey(),
                25_000,
            )]))
            .unwrap();
        let tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the utxos, we know they can't be used anyways
        details.transaction = Some(tx);
        details.height = Some(42);
        wallet.database.borrow_mut().set_tx(&details).unwrap();

        wallet.bump_fee(&txid, TxBuilder::new()).unwrap();
    }

    #[test]
    #[should_panic(expected = "FeeRateTooLow")]
    fn test_bump_fee_low_fee_rate() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, mut details) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)]).enable_rbf(),
            )
            .unwrap();
        let tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the utxos, we know they can't be used anyways
        details.transaction = Some(tx);
        wallet.database.borrow_mut().set_tx(&details).unwrap();

        wallet
            .bump_fee(
                &txid,
                TxBuilder::new().fee_rate(FeeRate::from_sat_per_vb(1.0)),
            )
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "FeeTooLow")]
    fn test_bump_fee_low_abs() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, mut details) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)]).enable_rbf(),
            )
            .unwrap();
        let tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the utxos, we know they can't be used anyways
        details.transaction = Some(tx);
        wallet.database.borrow_mut().set_tx(&details).unwrap();

        wallet
            .bump_fee(&txid, TxBuilder::new().fee_absolute(10))
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "FeeTooLow")]
    fn test_bump_fee_zero_abs() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = wallet.get_new_address().unwrap();
        let (psbt, mut details) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)]).enable_rbf(),
            )
            .unwrap();
        let tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the utxos, we know they can't be used anyways
        details.transaction = Some(tx);
        wallet.database.borrow_mut().set_tx(&details).unwrap();

        wallet
            .bump_fee(&txid, TxBuilder::new().fee_absolute(0))
            .unwrap();
    }

    #[test]
    fn test_bump_fee_reduce_change() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (psbt, mut original_details) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)]).enable_rbf(),
            )
            .unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the new utxos, we know they can't be used anyways
        for txin in &mut tx.input {
            txin.witness.push([0x00; 108].to_vec()); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        let (psbt, details) = wallet
            .bump_fee(
                &txid,
                TxBuilder::new().fee_rate(FeeRate::from_sat_per_vb(2.5)),
            )
            .unwrap();

        assert_eq!(details.sent, original_details.sent);
        assert_eq!(
            details.received + details.fees,
            original_details.received + original_details.fees
        );
        assert!(details.fees > original_details.fees);

        let tx = &psbt.global.unsigned_tx;
        assert_eq!(tx.output.len(), 2);
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey == addr.script_pubkey())
                .unwrap()
                .value,
            25_000
        );
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey != addr.script_pubkey())
                .unwrap()
                .value,
            details.received
        );

        assert_fee_rate!(psbt.extract_tx(), details.fees, FeeRate::from_sat_per_vb(2.5), @add_signature);
    }

    #[test]
    fn test_bump_fee_absolute_reduce_change() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (psbt, mut original_details) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 25_000)]).enable_rbf(),
            )
            .unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the new utxos, we know they can't be used anyways
        for txin in &mut tx.input {
            txin.witness.push([0x00; 108].to_vec()); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        let (psbt, details) = wallet
            .bump_fee(&txid, TxBuilder::new().fee_absolute(200))
            .unwrap();

        assert_eq!(details.sent, original_details.sent);
        assert_eq!(
            details.received + details.fees,
            original_details.received + original_details.fees
        );
        assert!(
            details.fees > original_details.fees,
            "{} > {}",
            details.fees,
            original_details.fees
        );

        let tx = &psbt.global.unsigned_tx;
        assert_eq!(tx.output.len(), 2);
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey == addr.script_pubkey())
                .unwrap()
                .value,
            25_000
        );
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey != addr.script_pubkey())
                .unwrap()
                .value,
            details.received
        );

        assert_eq!(details.fees, 200);
    }

    #[test]
    fn test_bump_fee_reduce_single_recipient() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (psbt, mut original_details) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet()
                    .enable_rbf(),
            )
            .unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        for txin in &mut tx.input {
            txin.witness.push([0x00; 108].to_vec()); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        let (psbt, details) = wallet
            .bump_fee(
                &txid,
                TxBuilder::new()
                    .maintain_single_recipient()
                    .fee_rate(FeeRate::from_sat_per_vb(2.5)),
            )
            .unwrap();

        assert_eq!(details.sent, original_details.sent);
        assert!(details.fees > original_details.fees);

        let tx = &psbt.global.unsigned_tx;
        assert_eq!(tx.output.len(), 1);
        assert_eq!(tx.output[0].value + details.fees, details.sent);

        assert_fee_rate!(psbt.extract_tx(), details.fees, FeeRate::from_sat_per_vb(2.5), @add_signature);
    }

    #[test]
    fn test_bump_fee_absolute_reduce_single_recipient() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (psbt, mut original_details) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet()
                    .enable_rbf(),
            )
            .unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        for txin in &mut tx.input {
            txin.witness.push([0x00; 108].to_vec()); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        let (psbt, details) = wallet
            .bump_fee(
                &txid,
                TxBuilder::new()
                    .maintain_single_recipient()
                    .fee_absolute(300),
            )
            .unwrap();

        assert_eq!(details.sent, original_details.sent);
        assert!(details.fees > original_details.fees);

        let tx = &psbt.global.unsigned_tx;
        assert_eq!(tx.output.len(), 1);
        assert_eq!(tx.output[0].value + details.fees, details.sent);

        assert_eq!(details.fees, 300);
    }

    #[test]
    fn test_bump_fee_drain_wallet() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        // receive an extra tx so that our wallet has two utxos.
        let incoming_txid = wallet.database.borrow_mut().received_tx(
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );
        let outpoint = OutPoint {
            txid: incoming_txid,
            vout: 0,
        };
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (psbt, mut original_details) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .utxos(vec![outpoint])
                    .manually_selected_only()
                    .enable_rbf(),
            )
            .unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        for txin in &mut tx.input {
            txin.witness.push([0x00; 108].to_vec()); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();
        assert_eq!(original_details.sent, 25_000);

        // for the new feerate, it should be enough to reduce the output, but since we specify
        // `drain_wallet` we expect to spend everything
        let (_, details) = wallet
            .bump_fee(
                &txid,
                TxBuilder::new()
                    .drain_wallet()
                    .maintain_single_recipient()
                    .fee_rate(FeeRate::from_sat_per_vb(5.0)),
            )
            .unwrap();
        assert_eq!(details.sent, 75_000);
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_bump_fee_remove_output_manually_selected_only() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        // receive an extra tx so that our wallet has two utxos. then we manually pick only one of
        // them, and make sure that `bump_fee` doesn't try to add more. eventually, it should fail
        // because the fee rate is too high and the single utxo isn't enough to create a non-dust
        // output
        let incoming_txid = wallet.database.borrow_mut().received_tx(
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );
        let outpoint = OutPoint {
            txid: incoming_txid,
            vout: 0,
        };
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (psbt, mut original_details) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .utxos(vec![outpoint])
                    .manually_selected_only()
                    .enable_rbf(),
            )
            .unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        for txin in &mut tx.input {
            txin.witness.push([0x00; 108].to_vec()); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();
        assert_eq!(original_details.sent, 25_000);

        wallet
            .bump_fee(
                &txid,
                TxBuilder::new()
                    .utxos(vec![outpoint])
                    .manually_selected_only()
                    .fee_rate(FeeRate::from_sat_per_vb(225.0)),
            )
            .unwrap();
    }

    #[test]
    fn test_bump_fee_add_input() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        wallet.database.borrow_mut().received_tx(
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (psbt, mut original_details) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 45_000)]).enable_rbf(),
            )
            .unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the new utxos, we know they can't be used anyways
        for txin in &mut tx.input {
            txin.witness.push([0x00; 108].to_vec()); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        let (psbt, details) = wallet
            .bump_fee(
                &txid,
                TxBuilder::new().fee_rate(FeeRate::from_sat_per_vb(50.0)),
            )
            .unwrap();

        assert_eq!(details.sent, original_details.sent + 25_000);
        assert_eq!(details.fees + details.received, 30_000);

        let tx = &psbt.global.unsigned_tx;
        assert_eq!(tx.input.len(), 2);
        assert_eq!(tx.output.len(), 2);
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey == addr.script_pubkey())
                .unwrap()
                .value,
            45_000
        );
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey != addr.script_pubkey())
                .unwrap()
                .value,
            details.received
        );

        assert_fee_rate!(psbt.extract_tx(), details.fees, FeeRate::from_sat_per_vb(50.0), @add_signature);
    }

    #[test]
    fn test_bump_fee_absolute_add_input() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        wallet.database.borrow_mut().received_tx(
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (psbt, mut original_details) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 45_000)]).enable_rbf(),
            )
            .unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the new utxos, we know they can't be used anyways
        for txin in &mut tx.input {
            txin.witness.push([0x00; 108].to_vec()); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        let (psbt, details) = wallet
            .bump_fee(&txid, TxBuilder::new().fee_absolute(6_000))
            .unwrap();

        assert_eq!(details.sent, original_details.sent + 25_000);
        assert_eq!(details.fees + details.received, 30_000);

        let tx = &psbt.global.unsigned_tx;
        assert_eq!(tx.input.len(), 2);
        assert_eq!(tx.output.len(), 2);
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey == addr.script_pubkey())
                .unwrap()
                .value,
            45_000
        );
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey != addr.script_pubkey())
                .unwrap()
                .value,
            details.received
        );

        assert_eq!(details.fees, 6_000);
    }

    #[test]
    fn test_bump_fee_no_change_add_input_and_change() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        let incoming_txid = wallet.database.borrow_mut().received_tx(
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        // initially make a tx without change by using `set_single_recipient`
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (psbt, mut original_details) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .add_utxo(OutPoint {
                        txid: incoming_txid,
                        vout: 0,
                    })
                    .manually_selected_only()
                    .enable_rbf(),
            )
            .unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the new utxos, we know they can't be used anyways
        for txin in &mut tx.input {
            txin.witness.push([0x00; 108].to_vec()); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        // now bump the fees without using `maintain_single_recipient`. the wallet should add an
        // extra input and a change output, and leave the original output untouched
        let (psbt, details) = wallet
            .bump_fee(
                &txid,
                TxBuilder::new().fee_rate(FeeRate::from_sat_per_vb(50.0)),
            )
            .unwrap();

        let original_send_all_amount = original_details.sent - original_details.fees;
        assert_eq!(details.sent, original_details.sent + 50_000);
        assert_eq!(
            details.received,
            75_000 - original_send_all_amount - details.fees
        );

        let tx = &psbt.global.unsigned_tx;
        assert_eq!(tx.input.len(), 2);
        assert_eq!(tx.output.len(), 2);
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey == addr.script_pubkey())
                .unwrap()
                .value,
            original_send_all_amount
        );
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey != addr.script_pubkey())
                .unwrap()
                .value,
            75_000 - original_send_all_amount - details.fees
        );

        assert_fee_rate!(psbt.extract_tx(), details.fees, FeeRate::from_sat_per_vb(50.0), @add_signature);
    }

    #[test]
    fn test_bump_fee_add_input_change_dust() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        wallet.database.borrow_mut().received_tx(
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (psbt, mut original_details) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 45_000)]).enable_rbf(),
            )
            .unwrap();
        let mut tx = psbt.extract_tx();
        assert_eq!(tx.input.len(), 1);
        assert_eq!(tx.output.len(), 2);
        let txid = tx.txid();
        // skip saving the new utxos, we know they can't be used anyways
        for txin in &mut tx.input {
            txin.witness.push([0x00; 108].to_vec()); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        let (psbt, details) = wallet
            .bump_fee(
                &txid,
                TxBuilder::new().fee_rate(FeeRate::from_sat_per_vb(140.0)),
            )
            .unwrap();

        assert_eq!(original_details.received, 5_000 - original_details.fees);

        assert_eq!(details.sent, original_details.sent + 25_000);
        assert_eq!(details.fees, 30_000);
        assert_eq!(details.received, 0);

        let tx = &psbt.global.unsigned_tx;
        assert_eq!(tx.input.len(), 2);
        assert_eq!(tx.output.len(), 1);
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey == addr.script_pubkey())
                .unwrap()
                .value,
            45_000
        );

        assert_fee_rate!(psbt.extract_tx(), details.fees, FeeRate::from_sat_per_vb(140.0), @dust_change, @add_signature);
    }

    #[test]
    fn test_bump_fee_force_add_input() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        let incoming_txid = wallet.database.borrow_mut().received_tx(
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (psbt, mut original_details) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 45_000)]).enable_rbf(),
            )
            .unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the new utxos, we know they can't be used anyways
        for txin in &mut tx.input {
            txin.witness.push([0x00; 108].to_vec()); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        // the new fee_rate is low enough that just reducing the change would be fine, but we force
        // the addition of an extra input with `add_utxo()`
        let (psbt, details) = wallet
            .bump_fee(
                &txid,
                TxBuilder::new()
                    .add_utxo(OutPoint {
                        txid: incoming_txid,
                        vout: 0,
                    })
                    .fee_rate(FeeRate::from_sat_per_vb(5.0)),
            )
            .unwrap();

        assert_eq!(details.sent, original_details.sent + 25_000);
        assert_eq!(details.fees + details.received, 30_000);

        let tx = &psbt.global.unsigned_tx;
        assert_eq!(tx.input.len(), 2);
        assert_eq!(tx.output.len(), 2);
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey == addr.script_pubkey())
                .unwrap()
                .value,
            45_000
        );
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey != addr.script_pubkey())
                .unwrap()
                .value,
            details.received
        );

        assert_fee_rate!(psbt.extract_tx(), details.fees, FeeRate::from_sat_per_vb(5.0), @add_signature);
    }

    #[test]
    fn test_bump_fee_absolute_force_add_input() {
        let (wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        let incoming_txid = wallet.database.borrow_mut().received_tx(
            testutils! (@tx ( (@external descriptors, 0) => 25_000 ) (@confirmations 1)),
            Some(100),
        );

        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (psbt, mut original_details) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 45_000)]).enable_rbf(),
            )
            .unwrap();
        let mut tx = psbt.extract_tx();
        let txid = tx.txid();
        // skip saving the new utxos, we know they can't be used anyways
        for txin in &mut tx.input {
            txin.witness.push([0x00; 108].to_vec()); // fake signature
            wallet
                .database
                .borrow_mut()
                .del_utxo(&txin.previous_output)
                .unwrap();
        }
        original_details.transaction = Some(tx);
        wallet
            .database
            .borrow_mut()
            .set_tx(&original_details)
            .unwrap();

        // the new fee_rate is low enough that just reducing the change would be fine, but we force
        // the addition of an extra input with `add_utxo()`
        let (psbt, details) = wallet
            .bump_fee(
                &txid,
                TxBuilder::new()
                    .add_utxo(OutPoint {
                        txid: incoming_txid,
                        vout: 0,
                    })
                    .fee_absolute(250),
            )
            .unwrap();

        assert_eq!(details.sent, original_details.sent + 25_000);
        assert_eq!(details.fees + details.received, 30_000);

        let tx = &psbt.global.unsigned_tx;
        assert_eq!(tx.input.len(), 2);
        assert_eq!(tx.output.len(), 2);
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey == addr.script_pubkey())
                .unwrap()
                .value,
            45_000
        );
        assert_eq!(
            tx.output
                .iter()
                .find(|txout| txout.script_pubkey != addr.script_pubkey())
                .unwrap()
                .value,
            details.received
        );

        assert_eq!(details.fees, 250);
    }

    #[test]
    fn test_sign_single_xprv() {
        let (wallet, _, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet(),
            )
            .unwrap();

        let (signed_psbt, finalized) = wallet.sign(psbt, None).unwrap();
        assert_eq!(finalized, true);

        let extracted = signed_psbt.extract_tx();
        assert_eq!(extracted.input[0].witness.len(), 2);
    }

    #[test]
    fn test_sign_single_xprv_bip44_path() {
        let (wallet, _, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/44'/0'/0'/0/*)");
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet(),
            )
            .unwrap();

        let (signed_psbt, finalized) = wallet.sign(psbt, None).unwrap();
        assert_eq!(finalized, true);

        let extracted = signed_psbt.extract_tx();
        assert_eq!(extracted.input[0].witness.len(), 2);
    }

    #[test]
    fn test_sign_single_xprv_sh_wpkh() {
        let (wallet, _, _) = get_funded_wallet("sh(wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*))");
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet(),
            )
            .unwrap();

        let (signed_psbt, finalized) = wallet.sign(psbt, None).unwrap();
        assert_eq!(finalized, true);

        let extracted = signed_psbt.extract_tx();
        assert_eq!(extracted.input[0].witness.len(), 2);
    }

    #[test]
    fn test_sign_single_wif() {
        let (wallet, _, _) =
            get_funded_wallet("wpkh(cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW)");
        let addr = wallet.get_new_address().unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet(),
            )
            .unwrap();

        let (signed_psbt, finalized) = wallet.sign(psbt, None).unwrap();
        assert_eq!(finalized, true);

        let extracted = signed_psbt.extract_tx();
        assert_eq!(extracted.input[0].witness.len(), 2);
    }

    #[test]
    fn test_sign_single_xprv_no_hd_keypaths() {
        let (wallet, _, _) = get_funded_wallet("wpkh(tprv8ZgxMBicQKsPd3EupYiPRhaMooHKUHJxNsTfYuScep13go8QFfHdtkG9nRkFGb7busX4isf6X9dURGCoKgitaApQ6MupRhZMcELAxTBRJgS/*)");
        let addr = wallet.get_new_address().unwrap();
        let (mut psbt, _) = wallet
            .create_tx(
                TxBuilder::new()
                    .set_single_recipient(addr.script_pubkey())
                    .drain_wallet(),
            )
            .unwrap();

        psbt.inputs[0].hd_keypaths.clear();
        assert_eq!(psbt.inputs[0].hd_keypaths.len(), 0);

        let (signed_psbt, finalized) = wallet.sign(psbt, None).unwrap();
        assert_eq!(finalized, true);

        let extracted = signed_psbt.extract_tx();
        assert_eq!(extracted.input[0].witness.len(), 2);
    }

    #[test]
    fn test_include_output_redeem_witness_script() {
        let (wallet, _, _) = get_funded_wallet("sh(wsh(multi(1,cVpPVruEDdmutPzisEsYvtST1usBR3ntr8pXSyt6D2YYqXRyPcFW,cRjo6jqfVNP33HhSS76UhXETZsGTZYx8FMFvR9kpbtCSV1PmdZdu)))");
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (psbt, _) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 45_000)])
                    .include_output_redeem_witness_script(),
            )
            .unwrap();

        // p2sh-p2wsh transaction should contain both witness and redeem scripts
        assert!(psbt
            .outputs
            .iter()
            .any(|output| output.redeem_script.is_some() && output.witness_script.is_some()));
    }

    #[test]
    fn test_signing_only_one_of_multiple_inputs() {
        let (wallet, _, _) = get_funded_wallet(get_test_wpkh());
        let addr = Address::from_str("2N1Ffz3WaNzbeLFBb51xyFMHYSEUXcbiSoX").unwrap();
        let (mut psbt, _) = wallet
            .create_tx(
                TxBuilder::with_recipients(vec![(addr.script_pubkey(), 45_000)])
                    .include_output_redeem_witness_script(),
            )
            .unwrap();

        // add another input to the psbt that is at least passable.
        let mut dud_input = bitcoin::util::psbt::Input::default();
        dud_input.witness_utxo = Some(TxOut {
            value: 100_000,
            script_pubkey: miniscript::Descriptor::<bitcoin::PublicKey>::from_str(
                "wpkh(025476c2e83188368da1ff3e292e7acafcdb3566bb0ad253f62fc70f07aeee6357)",
            )
            .unwrap()
            .script_pubkey(miniscript::NullCtx),
        });
        psbt.inputs.push(dud_input);
        psbt.global.unsigned_tx.input.push(bitcoin::TxIn::default());
        let (psbt, is_final) = wallet.sign(psbt, None).unwrap();
        assert!(
            !is_final,
            "shouldn't be final since we can't sign one of the inputs"
        );
        assert!(
            psbt.inputs[0].final_script_witness.is_some(),
            "should finalized input it signed"
        )
    }
}
