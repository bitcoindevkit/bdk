// Bitcoin Dev Kit
// Written in 2020 by Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020-2021 Bitcoin Dev Kit Developers
//
// This file is licensed under the Apache License, Version 2.0 <LICENSE-APACHE
// or http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your option.
// You may not use this file except in accordance with one or both of these
// licenses.

//! Transaction builder
//!
//! ## Example
//!
//! ```
//! # use std::str::FromStr;
//! # use bitcoin::*;
//! # use bdk_wallet::*;
//! # use bdk_wallet::ChangeSet;
//! # use bdk_wallet::error::CreateTxError;
//! # use anyhow::Error;
//! # let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap().assume_checked();
//! # let mut wallet = doctest_wallet!();
//! // create a TxBuilder from a wallet
//! let mut tx_builder = wallet.build_tx();
//!
//! tx_builder
//!     // Create a transaction with one output to `to_address` of 50_000 satoshi
//!     .add_recipient(to_address.script_pubkey(), Amount::from_sat(50_000))
//!     // With a custom fee rate of 5.0 satoshi/vbyte
//!     .fee_rate(FeeRate::from_sat_per_vb(5).expect("valid feerate"))
//!     // Only spend non-change outputs
//!     .do_not_spend_change();
//! let psbt = tx_builder.finish()?;
//! # Ok::<(), anyhow::Error>(())
//! ```

use alloc::{boxed::Box, string::String, vec::Vec};
use core::fmt;

use alloc::sync::Arc;

use bitcoin::psbt::{self, Psbt};
use bitcoin::script::PushBytes;
use bitcoin::{
    absolute, Amount, FeeRate, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid,
    Weight,
};
use rand_core::RngCore;

use super::coin_selection::CoinSelectionAlgorithm;
use super::utils::shuffle_slice;
use super::{CreateTxError, Wallet};
use crate::collections::{BTreeMap, HashSet};
use crate::{KeychainKind, LocalOutput, Utxo, WeightedUtxo};

/// A transaction builder
///
/// A `TxBuilder` is created by calling [`build_tx`] or [`build_fee_bump`] on a wallet. After
/// assigning it, you set options on it until finally calling [`finish`] to consume the builder and
/// generate the transaction.
///
/// Each option setting method on `TxBuilder` takes and returns `&mut self` so you can chain calls
/// as in the following example:
///
/// ```
/// # use bdk_wallet::*;
/// # use bdk_wallet::tx_builder::*;
/// # use bitcoin::*;
/// # use core::str::FromStr;
/// # use bdk_wallet::ChangeSet;
/// # use bdk_wallet::error::CreateTxError;
/// # use anyhow::Error;
/// # let mut wallet = doctest_wallet!();
/// # let addr1 = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap().assume_checked();
/// # let addr2 = addr1.clone();
/// // chaining
/// let psbt1 = {
///     let mut builder = wallet.build_tx();
///     builder
///         .ordering(TxOrdering::Untouched)
///         .add_recipient(addr1.script_pubkey(), Amount::from_sat(50_000))
///         .add_recipient(addr2.script_pubkey(), Amount::from_sat(50_000));
///     builder.finish()?
/// };
///
/// // non-chaining
/// let psbt2 = {
///     let mut builder = wallet.build_tx();
///     builder.ordering(TxOrdering::Untouched);
///     for addr in &[addr1, addr2] {
///         builder.add_recipient(addr.script_pubkey(), Amount::from_sat(50_000));
///     }
///     builder.finish()?
/// };
///
/// assert_eq!(psbt1.unsigned_tx.output[..2], psbt2.unsigned_tx.output[..2]);
/// # Ok::<(), anyhow::Error>(())
/// ```
///
/// At the moment [`coin_selection`] is an exception to the rule as it consumes `self`.
/// This means it is usually best to call [`coin_selection`] on the return value of `build_tx` before assigning it.
///
/// For further examples see [this module](super::tx_builder)'s documentation;
///
/// [`build_tx`]: Wallet::build_tx
/// [`build_fee_bump`]: Wallet::build_fee_bump
/// [`finish`]: Self::finish
/// [`coin_selection`]: Self::coin_selection
#[derive(Debug)]
pub struct TxBuilder<'a, Cs> {
    pub(crate) wallet: &'a mut Wallet,
    pub(crate) params: TxParams,
    pub(crate) coin_selection: Cs,
}

/// The parameters for transaction creation sans coin selection algorithm.
//TODO: TxParams should eventually be exposed publicly.
#[derive(Default, Debug, Clone)]
pub(crate) struct TxParams {
    pub(crate) recipients: Vec<(ScriptBuf, Amount)>,
    pub(crate) drain_wallet: bool,
    pub(crate) drain_to: Option<ScriptBuf>,
    pub(crate) fee_policy: Option<FeePolicy>,
    pub(crate) internal_policy_path: Option<BTreeMap<String, Vec<usize>>>,
    pub(crate) external_policy_path: Option<BTreeMap<String, Vec<usize>>>,
    pub(crate) utxos: Vec<WeightedUtxo>,
    pub(crate) unspendable: HashSet<OutPoint>,
    pub(crate) manually_selected_only: bool,
    pub(crate) sighash: Option<psbt::PsbtSighashType>,
    pub(crate) ordering: TxOrdering,
    pub(crate) locktime: Option<absolute::LockTime>,
    pub(crate) sequence: Option<Sequence>,
    pub(crate) version: Option<Version>,
    pub(crate) change_policy: ChangeSpendPolicy,
    pub(crate) only_witness_utxo: bool,
    pub(crate) add_global_xpubs: bool,
    pub(crate) include_output_redeem_witness_script: bool,
    pub(crate) bumping_fee: Option<PreviousFee>,
    pub(crate) current_height: Option<absolute::LockTime>,
    pub(crate) allow_dust: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PreviousFee {
    pub absolute: Amount,
    pub rate: FeeRate,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum FeePolicy {
    FeeRate(FeeRate),
    FeeAmount(Amount),
}

impl Default for FeePolicy {
    fn default() -> Self {
        FeePolicy::FeeRate(FeeRate::BROADCAST_MIN)
    }
}

// Methods supported for any CoinSelectionAlgorithm.
impl<'a, Cs> TxBuilder<'a, Cs> {
    /// Set a custom fee rate.
    ///
    /// This method sets the mining fee paid by the transaction as a rate on its size.
    /// This means that the total fee paid is equal to `fee_rate` times the size
    /// of the transaction. Default is 1 sat/vB in accordance with Bitcoin Core's default
    /// relay policy.
    ///
    /// Note that this is really a minimum feerate -- it's possible to
    /// overshoot it slightly since adding a change output to drain the remaining
    /// excess might not be viable.
    pub fn fee_rate(&mut self, fee_rate: FeeRate) -> &mut Self {
        self.params.fee_policy = Some(FeePolicy::FeeRate(fee_rate));
        self
    }

    /// Set an absolute fee
    /// The fee_absolute method refers to the absolute transaction fee in [`Amount`].
    /// If anyone sets both the `fee_absolute` method and the `fee_rate` method,
    /// the `FeePolicy` enum will be set by whichever method was called last,
    /// as the [`FeeRate`] and `FeeAmount` are mutually exclusive.
    ///
    /// Note that this is really a minimum absolute fee -- it's possible to
    /// overshoot it slightly since adding a change output to drain the remaining
    /// excess might not be viable.
    pub fn fee_absolute(&mut self, fee_amount: Amount) -> &mut Self {
        self.params.fee_policy = Some(FeePolicy::FeeAmount(fee_amount));
        self
    }

    /// Set the policy path to use while creating the transaction for a given keychain.
    ///
    /// This method accepts a map where the key is the policy node id (see
    /// [`Policy::id`](crate::descriptor::Policy::id)) and the value is the list of the indexes of
    /// the items that are intended to be satisfied from the policy node (see
    /// [`SatisfiableItem::Thresh::items`](crate::descriptor::policy::SatisfiableItem::Thresh::items)).
    ///
    /// ## Example
    ///
    /// An example of when the policy path is needed is the following descriptor:
    /// `wsh(thresh(2,pk(A),sj:and_v(v:pk(B),n:older(6)),snj:and_v(v:pk(C),after(630000))))`,
    /// derived from the miniscript policy `thresh(2,pk(A),and(pk(B),older(6)),and(pk(C),after(630000)))`.
    /// It declares three descriptor fragments, and at the top level it uses `thresh()` to
    /// ensure that at least two of them are satisfied. The individual fragments are:
    ///
    /// 1. `pk(A)`
    /// 2. `and(pk(B),older(6))`
    /// 3. `and(pk(C),after(630000))`
    ///
    /// When those conditions are combined in pairs, it's clear that the transaction needs to be created
    /// differently depending on how the user intends to satisfy the policy afterwards:
    ///
    /// * If fragments `1` and `2` are used, the transaction will need to use a specific
    ///   `n_sequence` in order to spend an `OP_CSV` branch.
    /// * If fragments `1` and `3` are used, the transaction will need to use a specific `locktime`
    ///   in order to spend an `OP_CLTV` branch.
    /// * If fragments `2` and `3` are used, the transaction will need both.
    ///
    /// When the spending policy is represented as a tree (see
    /// [`Wallet::policies`](super::Wallet::policies)), every node
    /// is assigned a unique identifier that can be used in the policy path to specify which of
    /// the node's children the user intends to satisfy: for instance, assuming the `thresh()`
    /// root node of this example has an id of `aabbccdd`, the policy path map would look like:
    ///
    /// `{ "aabbccdd" => [0, 1] }`
    ///
    /// where the key is the node's id, and the value is a list of the children that should be
    /// used, in no particular order.
    ///
    /// If a particularly complex descriptor has multiple ambiguous thresholds in its structure,
    /// multiple entries can be added to the map, one for each node that requires an explicit path.
    ///
    /// ```
    /// # use std::str::FromStr;
    /// # use std::collections::BTreeMap;
    /// # use bitcoin::*;
    /// # use bdk_wallet::*;
    /// # let to_address =
    /// Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt")
    ///     .unwrap()
    ///     .assume_checked();
    /// # let mut wallet = doctest_wallet!();
    /// let mut path = BTreeMap::new();
    /// path.insert("aabbccdd".to_string(), vec![0, 1]);
    ///
    /// let builder = wallet
    ///     .build_tx()
    ///     .add_recipient(to_address.script_pubkey(), Amount::from_sat(50_000))
    ///     .policy_path(path, KeychainKind::External);
    ///
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn policy_path(
        &mut self,
        policy_path: BTreeMap<String, Vec<usize>>,
        keychain: KeychainKind,
    ) -> &mut Self {
        let to_update = match keychain {
            KeychainKind::Internal => &mut self.params.internal_policy_path,
            KeychainKind::External => &mut self.params.external_policy_path,
        };

        *to_update = Some(policy_path);
        self
    }

    /// Add the list of outpoints to the internal list of UTXOs that **must** be spent.
    ///
    /// If an error occurs while adding any of the UTXOs then none of them are added and the error is returned.
    ///
    /// These have priority over the "unspendable" utxos, meaning that if a utxo is present both in
    /// the "utxos" and the "unspendable" list, it will be spent.
    pub fn add_utxos(&mut self, outpoints: &[OutPoint]) -> Result<&mut Self, AddUtxoError> {
        {
            let wallet = &mut self.wallet;
            let utxos = outpoints
                .iter()
                .map(|outpoint| {
                    wallet
                        .get_utxo(*outpoint)
                        .ok_or(AddUtxoError::UnknownUtxo(*outpoint))
                })
                .collect::<Result<Vec<_>, _>>()?;

            for utxo in utxos {
                let descriptor = wallet.public_descriptor(utxo.keychain);
                let satisfaction_weight = descriptor.max_weight_to_satisfy().unwrap();
                self.params.utxos.push(WeightedUtxo {
                    satisfaction_weight,
                    utxo: Utxo::Local(utxo),
                });
            }
        }

        Ok(self)
    }

    /// Add a utxo to the internal list of utxos that **must** be spent
    ///
    /// These have priority over the "unspendable" utxos, meaning that if a utxo is present both in
    /// the "utxos" and the "unspendable" list, it will be spent.
    pub fn add_utxo(&mut self, outpoint: OutPoint) -> Result<&mut Self, AddUtxoError> {
        self.add_utxos(&[outpoint])
    }

    /// Add a foreign UTXO i.e. a UTXO not owned by this wallet.
    ///
    /// At a minimum to add a foreign UTXO we need:
    ///
    /// 1. `outpoint`: To add it to the raw transaction.
    /// 2. `psbt_input`: To know the value.
    /// 3. `satisfaction_weight`: To know how much weight/vbytes the input will add to the transaction for fee calculation.
    ///
    /// There are several security concerns about adding foreign UTXOs that application
    /// developers should consider. First, how do you know the value of the input is correct? If a
    /// `non_witness_utxo` is provided in the `psbt_input` then this method implicitly verifies the
    /// value by checking it against the transaction. If only a `witness_utxo` is provided then this
    /// method doesn't verify the value but just takes it as a given -- it is up to you to check
    /// that whoever sent you the `input_psbt` was not lying!
    ///
    /// Secondly, you must somehow provide `satisfaction_weight` of the input. Depending on your
    /// application it may be important that this be known precisely. If not, a malicious
    /// counterparty may fool you into putting in a value that is too low, giving the transaction a
    /// lower than expected feerate. They could also fool you into putting a value that is too high
    /// causing you to pay a fee that is too high. The party who is broadcasting the transaction can
    /// of course check the real input weight matches the expected weight prior to broadcasting.
    ///
    /// To guarantee the `max_weight_to_satisfy` is correct, you can require the party providing the
    /// `psbt_input` provide a miniscript descriptor for the input so you can check it against the
    /// `script_pubkey` and then ask it for the [`max_weight_to_satisfy`].
    ///
    /// This is an **EXPERIMENTAL** feature, API and other major changes are expected.
    ///
    /// In order to use [`Wallet::calculate_fee`] or [`Wallet::calculate_fee_rate`] for a transaction
    /// created with foreign UTXO(s) you must manually insert the corresponding TxOut(s) into the tx
    /// graph using the [`Wallet::insert_txout`] function.
    ///
    /// # Errors
    ///
    /// This method returns errors in the following circumstances:
    ///
    /// 1. The `psbt_input` does not contain a `witness_utxo` or `non_witness_utxo`.
    /// 2. The data in `non_witness_utxo` does not match what is in `outpoint`.
    ///
    /// Note unless you set [`only_witness_utxo`] any non-taproot `psbt_input` you pass to this
    /// method must have `non_witness_utxo` set otherwise you will get an error when [`finish`]
    /// is called.
    ///
    /// [`only_witness_utxo`]: Self::only_witness_utxo
    /// [`finish`]: Self::finish
    /// [`max_weight_to_satisfy`]: miniscript::Descriptor::max_weight_to_satisfy
    pub fn add_foreign_utxo(
        &mut self,
        outpoint: OutPoint,
        psbt_input: psbt::Input,
        satisfaction_weight: Weight,
    ) -> Result<&mut Self, AddForeignUtxoError> {
        self.add_foreign_utxo_with_sequence(
            outpoint,
            psbt_input,
            satisfaction_weight,
            Sequence::MAX,
        )
    }

    /// Same as [add_foreign_utxo](TxBuilder::add_foreign_utxo) but allows to set the nSequence value.
    pub fn add_foreign_utxo_with_sequence(
        &mut self,
        outpoint: OutPoint,
        psbt_input: psbt::Input,
        satisfaction_weight: Weight,
        sequence: Sequence,
    ) -> Result<&mut Self, AddForeignUtxoError> {
        if psbt_input.witness_utxo.is_none() {
            match psbt_input.non_witness_utxo.as_ref() {
                Some(tx) => {
                    if tx.compute_txid() != outpoint.txid {
                        return Err(AddForeignUtxoError::InvalidTxid {
                            input_txid: tx.compute_txid(),
                            foreign_utxo: outpoint,
                        });
                    }
                    if tx.output.len() <= outpoint.vout as usize {
                        return Err(AddForeignUtxoError::InvalidOutpoint(outpoint));
                    }
                }
                None => {
                    return Err(AddForeignUtxoError::MissingUtxo);
                }
            }
        }

        self.params.utxos.push(WeightedUtxo {
            satisfaction_weight,
            utxo: Utxo::Foreign {
                outpoint,
                sequence,
                psbt_input: Box::new(psbt_input),
            },
        });

        Ok(self)
    }

    /// Only spend utxos added by [`add_utxo`].
    ///
    /// The wallet will **not** add additional utxos to the transaction even if they are needed to
    /// make the transaction valid.
    ///
    /// [`add_utxo`]: Self::add_utxo
    pub fn manually_selected_only(&mut self) -> &mut Self {
        self.params.manually_selected_only = true;
        self
    }

    /// Replace the internal list of unspendable utxos with a new list
    ///
    /// It's important to note that the "must-be-spent" utxos added with [`TxBuilder::add_utxo`]
    /// have priority over these. See the docs of the two linked methods for more details.
    pub fn unspendable(&mut self, unspendable: Vec<OutPoint>) -> &mut Self {
        self.params.unspendable = unspendable.into_iter().collect();
        self
    }

    /// Add a utxo to the internal list of unspendable utxos
    ///
    /// It's important to note that the "must-be-spent" utxos added with [`TxBuilder::add_utxo`]
    /// have priority over this. See the docs of the two linked methods for more details.
    pub fn add_unspendable(&mut self, unspendable: OutPoint) -> &mut Self {
        self.params.unspendable.insert(unspendable);
        self
    }

    /// Sign with a specific sig hash
    ///
    /// **Use this option very carefully**
    pub fn sighash(&mut self, sighash: psbt::PsbtSighashType) -> &mut Self {
        self.params.sighash = Some(sighash);
        self
    }

    /// Choose the ordering for inputs and outputs of the transaction
    pub fn ordering(&mut self, ordering: TxOrdering) -> &mut Self {
        self.params.ordering = ordering;
        self
    }

    /// Use a specific nLockTime while creating the transaction
    ///
    /// This can cause conflicts if the wallet's descriptors contain an "after" (OP_CLTV) operator.
    pub fn nlocktime(&mut self, locktime: absolute::LockTime) -> &mut Self {
        self.params.locktime = Some(locktime);
        self
    }

    /// Build a transaction with a specific version
    ///
    /// The `version` should always be greater than `0` and greater than `1` if the wallet's
    /// descriptors contain an "older" (OP_CSV) operator.
    pub fn version(&mut self, version: i32) -> &mut Self {
        self.params.version = Some(Version(version));
        self
    }

    /// Do not spend change outputs
    ///
    /// This effectively adds all the change outputs to the "unspendable" list. See
    /// [`TxBuilder::unspendable`]. This method assumes the presence of an internal
    /// keychain, otherwise it has no effect.
    pub fn do_not_spend_change(&mut self) -> &mut Self {
        self.params.change_policy = ChangeSpendPolicy::ChangeForbidden;
        self
    }

    /// Only spend change outputs
    ///
    /// This effectively adds all the non-change outputs to the "unspendable" list. See
    /// [`TxBuilder::unspendable`]. This method assumes the presence of an internal
    /// keychain, otherwise it has no effect.
    pub fn only_spend_change(&mut self) -> &mut Self {
        self.params.change_policy = ChangeSpendPolicy::OnlyChange;
        self
    }

    /// Set a specific [`ChangeSpendPolicy`]. See [`TxBuilder::do_not_spend_change`] and
    /// [`TxBuilder::only_spend_change`] for some shortcuts. This method assumes the presence
    /// of an internal keychain, otherwise it has no effect.
    pub fn change_policy(&mut self, change_policy: ChangeSpendPolicy) -> &mut Self {
        self.params.change_policy = change_policy;
        self
    }

    /// Only Fill-in the [`psbt::Input::witness_utxo`](bitcoin::psbt::Input::witness_utxo) field when spending from
    /// SegWit descriptors.
    ///
    /// This reduces the size of the PSBT, but some signers might reject them due to the lack of
    /// the `non_witness_utxo`.
    pub fn only_witness_utxo(&mut self) -> &mut Self {
        self.params.only_witness_utxo = true;
        self
    }

    /// Fill-in the [`psbt::Output::redeem_script`](bitcoin::psbt::Output::redeem_script) and
    /// [`psbt::Output::witness_script`](bitcoin::psbt::Output::witness_script) fields.
    ///
    /// This is useful for signers which always require it, like ColdCard hardware wallets.
    pub fn include_output_redeem_witness_script(&mut self) -> &mut Self {
        self.params.include_output_redeem_witness_script = true;
        self
    }

    /// Fill-in the `PSBT_GLOBAL_XPUB` field with the extended keys contained in both the external
    /// and internal descriptors
    ///
    /// This is useful for offline signers that take part to a multisig. Some hardware wallets like
    /// BitBox and ColdCard are known to require this.
    pub fn add_global_xpubs(&mut self) -> &mut Self {
        self.params.add_global_xpubs = true;
        self
    }

    /// Spend all the available inputs. This respects filters like [`TxBuilder::unspendable`] and the change policy.
    pub fn drain_wallet(&mut self) -> &mut Self {
        self.params.drain_wallet = true;
        self
    }

    /// Choose the coin selection algorithm
    ///
    /// Overrides the [`CoinSelectionAlgorithm`].
    ///
    /// Note that this function consumes the builder and returns it so it is usually best to put this as the first call on the builder.
    pub fn coin_selection<P: CoinSelectionAlgorithm>(self, coin_selection: P) -> TxBuilder<'a, P> {
        TxBuilder {
            wallet: self.wallet,
            params: self.params,
            coin_selection,
        }
    }

    /// Set an exact nSequence value
    ///
    /// This can cause conflicts if the wallet's descriptors contain an
    /// "older" (OP_CSV) operator and the given `nsequence` is lower than the CSV value.
    pub fn set_exact_sequence(&mut self, n_sequence: Sequence) -> &mut Self {
        self.params.sequence = Some(n_sequence);
        self
    }

    /// Set the current blockchain height.
    ///
    /// This will be used to:
    /// 1. Set the nLockTime for preventing fee sniping.
    ///    **Note**: This will be ignored if you manually specify a nlocktime using [`TxBuilder::nlocktime`].
    /// 2. Decide whether coinbase outputs are mature or not. If the coinbase outputs are not
    ///    mature at `current_height`, we ignore them in the coin selection.
    ///    If you want to create a transaction that spends immature coinbase inputs, manually
    ///    add them using [`TxBuilder::add_utxos`].
    ///
    /// In both cases, if you don't provide a current height, we use the last sync height.
    pub fn current_height(&mut self, height: u32) -> &mut Self {
        self.params.current_height =
            Some(absolute::LockTime::from_height(height).expect("Invalid height"));
        self
    }

    /// Set whether or not the dust limit is checked.
    ///
    /// **Note**: by avoiding a dust limit check you may end up with a transaction that is non-standard.
    pub fn allow_dust(&mut self, allow_dust: bool) -> &mut Self {
        self.params.allow_dust = allow_dust;
        self
    }

    /// Replace the recipients already added with a new list
    pub fn set_recipients(&mut self, recipients: Vec<(ScriptBuf, Amount)>) -> &mut Self {
        self.params.recipients = recipients;
        self
    }

    /// Add a recipient to the internal list
    pub fn add_recipient(&mut self, script_pubkey: ScriptBuf, amount: Amount) -> &mut Self {
        self.params.recipients.push((script_pubkey, amount));
        self
    }

    /// Add data as an output, using OP_RETURN
    pub fn add_data<T: AsRef<PushBytes>>(&mut self, data: &T) -> &mut Self {
        let script = ScriptBuf::new_op_return(data);
        self.add_recipient(script, Amount::ZERO);
        self
    }

    /// Sets the address to *drain* excess coins to.
    ///
    /// Usually, when there are excess coins they are sent to a change address generated by the
    /// wallet. This option replaces the usual change address with an arbitrary `script_pubkey` of
    /// your choosing. Just as with a change output, if the drain output is not needed (the excess
    /// coins are too small) it will not be included in the resulting transaction. The only
    /// difference is that it is valid to use `drain_to` without setting any ordinary recipients
    /// with [`add_recipient`] (but it is perfectly fine to add recipients as well).
    ///
    /// If you choose not to set any recipients, you should provide the utxos that the
    /// transaction should spend via [`add_utxos`].
    ///
    /// # Example
    ///
    /// `drain_to` is very useful for draining all the coins in a wallet with [`drain_wallet`] to a
    /// single address.
    ///
    /// ```
    /// # use std::str::FromStr;
    /// # use bitcoin::*;
    /// # use bdk_wallet::*;
    /// # use bdk_wallet::ChangeSet;
    /// # use bdk_wallet::error::CreateTxError;
    /// # use anyhow::Error;
    /// # let to_address =
    /// Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt")
    ///     .unwrap()
    ///     .assume_checked();
    /// # let mut wallet = doctest_wallet!();
    /// let mut tx_builder = wallet.build_tx();
    ///
    /// tx_builder
    ///     // Spend all outputs in this wallet.
    ///     .drain_wallet()
    ///     // Send the excess (which is all the coins minus the fee) to this address.
    ///     .drain_to(to_address.script_pubkey())
    ///     .fee_rate(FeeRate::from_sat_per_vb(5).expect("valid feerate"));
    /// let psbt = tx_builder.finish()?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    ///
    /// [`add_recipient`]: Self::add_recipient
    /// [`add_utxos`]: Self::add_utxos
    /// [`drain_wallet`]: Self::drain_wallet
    pub fn drain_to(&mut self, script_pubkey: ScriptBuf) -> &mut Self {
        self.params.drain_to = Some(script_pubkey);
        self
    }
}

impl<'a, Cs: CoinSelectionAlgorithm> TxBuilder<'a, Cs> {
    /// Finish building the transaction.
    ///
    /// Uses the thread-local random number generator (rng).
    ///
    /// Returns a new [`Psbt`] per [`BIP174`].
    ///
    /// [`BIP174`]: https://github.com/bitcoin/bips/blob/master/bip-0174.mediawiki
    ///
    /// **WARNING**: To avoid change address reuse you must persist the changes resulting from one
    /// or more calls to this method before closing the wallet. See [`Wallet::reveal_next_address`].
    #[cfg(feature = "std")]
    pub fn finish(self) -> Result<Psbt, CreateTxError> {
        self.finish_with_aux_rand(&mut bitcoin::key::rand::thread_rng())
    }

    /// Finish building the transaction.
    ///
    /// Uses a provided random number generator (rng).
    ///
    /// Returns a new [`Psbt`] per [`BIP174`].
    ///
    /// [`BIP174`]: https://github.com/bitcoin/bips/blob/master/bip-0174.mediawiki
    ///
    /// **WARNING**: To avoid change address reuse you must persist the changes resulting from one
    /// or more calls to this method before closing the wallet. See [`Wallet::reveal_next_address`].
    pub fn finish_with_aux_rand(self, rng: &mut impl RngCore) -> Result<Psbt, CreateTxError> {
        self.wallet.create_tx(self.coin_selection, self.params, rng)
    }
}

#[derive(Debug)]
/// Error returned from [`TxBuilder::add_utxo`] and [`TxBuilder::add_utxos`]
pub enum AddUtxoError {
    /// Happens when trying to spend an UTXO that is not in the internal database
    UnknownUtxo(OutPoint),
}

impl fmt::Display for AddUtxoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownUtxo(outpoint) => write!(
                f,
                "UTXO not found in the internal database for txid: {} with vout: {}",
                outpoint.txid, outpoint.vout
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for AddUtxoError {}

#[derive(Debug)]
/// Error returned from [`TxBuilder::add_foreign_utxo`].
pub enum AddForeignUtxoError {
    /// Foreign utxo outpoint txid does not match PSBT input txid
    InvalidTxid {
        /// PSBT input txid
        input_txid: Txid,
        /// Foreign UTXO outpoint
        foreign_utxo: OutPoint,
    },
    /// Requested outpoint doesn't exist in the tx (vout greater than available outputs)
    InvalidOutpoint(OutPoint),
    /// Foreign utxo missing witness_utxo or non_witness_utxo
    MissingUtxo,
}

impl fmt::Display for AddForeignUtxoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTxid {
                input_txid,
                foreign_utxo,
            } => write!(
                f,
                "Foreign UTXO outpoint txid: {} does not match PSBT input txid: {}",
                foreign_utxo.txid, input_txid,
            ),
            Self::InvalidOutpoint(outpoint) => write!(
                f,
                "Requested outpoint doesn't exist for txid: {} with vout: {}",
                outpoint.txid, outpoint.vout,
            ),
            Self::MissingUtxo => write!(f, "Foreign utxo missing witness_utxo or non_witness_utxo"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for AddForeignUtxoError {}

type TxSort<T> = dyn (Fn(&T, &T) -> core::cmp::Ordering) + Send + Sync;

/// Ordering of the transaction's inputs and outputs
#[derive(Clone, Default)]
pub enum TxOrdering {
    /// Randomized (default)
    #[default]
    Shuffle,
    /// Unchanged
    Untouched,
    /// Provide custom comparison functions for sorting
    Custom {
        /// Transaction inputs sort function
        input_sort: Arc<TxSort<TxIn>>,
        /// Transaction outputs sort function
        output_sort: Arc<TxSort<TxOut>>,
    },
}

impl core::fmt::Debug for TxOrdering {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            TxOrdering::Shuffle => write!(f, "Shuffle"),
            TxOrdering::Untouched => write!(f, "Untouched"),
            TxOrdering::Custom { .. } => write!(f, "Custom"),
        }
    }
}

impl TxOrdering {
    /// Sort transaction inputs and outputs by [`TxOrdering`] variant.
    ///
    /// Uses the thread-local random number generator (rng).
    #[cfg(feature = "std")]
    pub fn sort_tx(&self, tx: &mut Transaction) {
        self.sort_tx_with_aux_rand(tx, &mut bitcoin::key::rand::thread_rng())
    }

    /// Sort transaction inputs and outputs by [`TxOrdering`] variant.
    ///
    /// Uses a provided random number generator (rng).
    pub fn sort_tx_with_aux_rand(&self, tx: &mut Transaction, rng: &mut impl RngCore) {
        match self {
            TxOrdering::Untouched => {}
            TxOrdering::Shuffle => {
                shuffle_slice(&mut tx.input, rng);
                shuffle_slice(&mut tx.output, rng);
            }
            TxOrdering::Custom {
                input_sort,
                output_sort,
            } => {
                tx.input.sort_unstable_by(|a, b| input_sort(a, b));
                tx.output.sort_unstable_by(|a, b| output_sort(a, b));
            }
        }
    }
}

/// Transaction version
///
/// Has a default value of `1`
#[derive(Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Clone, Copy)]
pub(crate) struct Version(pub(crate) i32);

impl Default for Version {
    fn default() -> Self {
        Version(1)
    }
}

/// Policy regarding the use of change outputs when creating a transaction
#[derive(Default, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Clone, Copy)]
pub enum ChangeSpendPolicy {
    /// Use both change and non-change outputs (default)
    #[default]
    ChangeAllowed,
    /// Only use change outputs (see [`TxBuilder::only_spend_change`])
    OnlyChange,
    /// Only use non-change outputs (see [`TxBuilder::do_not_spend_change`])
    ChangeForbidden,
}

impl ChangeSpendPolicy {
    pub(crate) fn is_satisfied_by(&self, utxo: &LocalOutput) -> bool {
        match self {
            ChangeSpendPolicy::ChangeAllowed => true,
            ChangeSpendPolicy::OnlyChange => utxo.keychain == KeychainKind::Internal,
            ChangeSpendPolicy::ChangeForbidden => utxo.keychain == KeychainKind::External,
        }
    }
}

#[cfg(test)]
mod test {
    const ORDERING_TEST_TX: &str = "0200000003c26f3eb7932f7acddc5ddd26602b77e7516079b03090a16e2c2f54\
                                    85d1fd600f0100000000ffffffffc26f3eb7932f7acddc5ddd26602b77e75160\
                                    79b03090a16e2c2f5485d1fd600f0000000000ffffffff571fb3e02278217852\
                                    dd5d299947e2b7354a639adc32ec1fa7b82cfb5dec530e0500000000ffffffff\
                                    03e80300000000000002aaeee80300000000000001aa200300000000000001ff\
                                    00000000";
    macro_rules! ordering_test_tx {
        () => {
            deserialize::<bitcoin::Transaction>(&Vec::<u8>::from_hex(ORDERING_TEST_TX).unwrap())
                .unwrap()
        };
    }

    use bitcoin::consensus::deserialize;
    use bitcoin::hex::FromHex;
    use bitcoin::TxOut;

    use super::*;
    #[test]
    fn test_output_ordering_untouched() {
        let original_tx = ordering_test_tx!();
        let mut tx = original_tx.clone();

        TxOrdering::Untouched.sort_tx(&mut tx);

        assert_eq!(original_tx, tx);
    }

    #[test]
    fn test_output_ordering_shuffle() {
        let original_tx = ordering_test_tx!();
        let mut tx = original_tx.clone();

        (0..40)
            .find(|_| {
                TxOrdering::Shuffle.sort_tx(&mut tx);
                original_tx.input != tx.input
            })
            .expect("it should have moved the inputs at least once");

        let mut tx = original_tx.clone();
        (0..40)
            .find(|_| {
                TxOrdering::Shuffle.sort_tx(&mut tx);
                original_tx.output != tx.output
            })
            .expect("it should have moved the outputs at least once");
    }

    #[test]
    fn test_output_ordering_custom_but_bip69() {
        use core::str::FromStr;

        let original_tx = ordering_test_tx!();
        let mut tx = original_tx;

        let bip69_txin_cmp = |tx_a: &TxIn, tx_b: &TxIn| {
            let project_outpoint = |t: &TxIn| (t.previous_output.txid, t.previous_output.vout);
            project_outpoint(tx_a).cmp(&project_outpoint(tx_b))
        };

        let bip69_txout_cmp = |tx_a: &TxOut, tx_b: &TxOut| {
            let project_utxo = |t: &TxOut| (t.value, t.script_pubkey.clone());
            project_utxo(tx_a).cmp(&project_utxo(tx_b))
        };

        let custom_bip69_ordering = TxOrdering::Custom {
            input_sort: Arc::new(bip69_txin_cmp),
            output_sort: Arc::new(bip69_txout_cmp),
        };

        custom_bip69_ordering.sort_tx(&mut tx);

        assert_eq!(
            tx.input[0].previous_output,
            bitcoin::OutPoint::from_str(
                "0e53ec5dfb2cb8a71fec32dc9a634a35b7e24799295ddd5278217822e0b31f57:5"
            )
            .unwrap()
        );
        assert_eq!(
            tx.input[1].previous_output,
            bitcoin::OutPoint::from_str(
                "0f60fdd185542f2c6ea19030b0796051e7772b6026dd5ddccd7a2f93b73e6fc2:0"
            )
            .unwrap()
        );
        assert_eq!(
            tx.input[2].previous_output,
            bitcoin::OutPoint::from_str(
                "0f60fdd185542f2c6ea19030b0796051e7772b6026dd5ddccd7a2f93b73e6fc2:1"
            )
            .unwrap()
        );

        assert_eq!(tx.output[0].value.to_sat(), 800);
        assert_eq!(tx.output[1].script_pubkey, ScriptBuf::from(vec![0xAA]));
        assert_eq!(
            tx.output[2].script_pubkey,
            ScriptBuf::from(vec![0xAA, 0xEE])
        );
    }

    #[test]
    fn test_output_ordering_custom_with_sha256() {
        use bitcoin::hashes::{sha256, Hash};

        let original_tx = ordering_test_tx!();
        let mut tx_1 = original_tx.clone();
        let mut tx_2 = original_tx.clone();
        let shared_secret = "secret_tweak";

        let hash_txin_with_shared_secret_seed = Arc::new(|tx_a: &TxIn, tx_b: &TxIn| {
            let secret_digest_from_txin = |txin: &TxIn| {
                sha256::Hash::hash(
                    &[
                        &txin.previous_output.txid.to_raw_hash()[..],
                        &txin.previous_output.vout.to_be_bytes(),
                        shared_secret.as_bytes(),
                    ]
                    .concat(),
                )
            };
            secret_digest_from_txin(tx_a).cmp(&secret_digest_from_txin(tx_b))
        });

        let hash_txout_with_shared_secret_seed = Arc::new(|tx_a: &TxOut, tx_b: &TxOut| {
            let secret_digest_from_txout = |txin: &TxOut| {
                sha256::Hash::hash(
                    &[
                        &txin.value.to_sat().to_be_bytes(),
                        &txin.script_pubkey.clone().into_bytes()[..],
                        shared_secret.as_bytes(),
                    ]
                    .concat(),
                )
            };
            secret_digest_from_txout(tx_a).cmp(&secret_digest_from_txout(tx_b))
        });

        let custom_ordering_from_salted_sha256_1 = TxOrdering::Custom {
            input_sort: hash_txin_with_shared_secret_seed.clone(),
            output_sort: hash_txout_with_shared_secret_seed.clone(),
        };

        let custom_ordering_from_salted_sha256_2 = TxOrdering::Custom {
            input_sort: hash_txin_with_shared_secret_seed,
            output_sort: hash_txout_with_shared_secret_seed,
        };

        custom_ordering_from_salted_sha256_1.sort_tx(&mut tx_1);
        custom_ordering_from_salted_sha256_2.sort_tx(&mut tx_2);

        // Check the ordering is consistent between calls
        assert_eq!(tx_1, tx_2);
        // Check transaction order has changed
        assert_ne!(tx_1, original_tx);
        assert_ne!(tx_2, original_tx);
    }

    fn get_test_utxos() -> Vec<LocalOutput> {
        use bitcoin::hashes::Hash;

        vec![
            LocalOutput {
                outpoint: OutPoint {
                    txid: bitcoin::Txid::from_slice(&[0; 32]).unwrap(),
                    vout: 0,
                },
                txout: TxOut::NULL,
                keychain: KeychainKind::External,
                is_spent: false,
                chain_position: chain::ChainPosition::Unconfirmed(0),
                derivation_index: 0,
            },
            LocalOutput {
                outpoint: OutPoint {
                    txid: bitcoin::Txid::from_slice(&[0; 32]).unwrap(),
                    vout: 1,
                },
                txout: TxOut::NULL,
                keychain: KeychainKind::Internal,
                is_spent: false,
                chain_position: chain::ChainPosition::Confirmed(chain::ConfirmationBlockTime {
                    block_id: chain::BlockId {
                        height: 32,
                        hash: bitcoin::BlockHash::all_zeros(),
                    },
                    confirmation_time: 42,
                }),
                derivation_index: 1,
            },
        ]
    }

    #[test]
    fn test_change_spend_policy_default() {
        let change_spend_policy = ChangeSpendPolicy::default();
        let filtered = get_test_utxos()
            .into_iter()
            .filter(|u| change_spend_policy.is_satisfied_by(u))
            .count();

        assert_eq!(filtered, 2);
    }

    #[test]
    fn test_change_spend_policy_no_internal() {
        let change_spend_policy = ChangeSpendPolicy::ChangeForbidden;
        let filtered = get_test_utxos()
            .into_iter()
            .filter(|u| change_spend_policy.is_satisfied_by(u))
            .collect::<Vec<_>>();

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].keychain, KeychainKind::External);
    }

    #[test]
    fn test_change_spend_policy_only_internal() {
        let change_spend_policy = ChangeSpendPolicy::OnlyChange;
        let filtered = get_test_utxos()
            .into_iter()
            .filter(|u| change_spend_policy.is_satisfied_by(u))
            .collect::<Vec<_>>();

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].keychain, KeychainKind::Internal);
    }

    #[test]
    fn test_default_tx_version_1() {
        let version = Version::default();
        assert_eq!(version.0, 1);
    }
}
