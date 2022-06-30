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
//! # use bdk::*;
//! # use bdk::wallet::tx_builder::CreateTx;
//! # let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap();
//! # let wallet = doctest_wallet!();
//! // create a TxBuilder from a wallet
//! let mut tx_builder = wallet.build_tx();
//!
//! tx_builder
//!     // Create a transaction with one output to `to_address` of 50_000 satoshi
//!     .add_recipient(to_address.script_pubkey(), 50_000)
//!     // With a custom fee rate of 5.0 satoshi/vbyte
//!     .fee_rate(FeeRate::from_sat_per_vb(5.0))
//!     // Only spend non-change outputs
//!     .do_not_spend_change()
//!     // Turn on RBF signaling
//!     .enable_rbf();
//! let (psbt, tx_details) = tx_builder.finish()?;
//! # Ok::<(), bdk::Error>(())
//! ```

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::default::Default;
use std::marker::PhantomData;

use bitcoin::util::psbt::{self, PartiallySignedTransaction as Psbt};
use bitcoin::{OutPoint, Script, Transaction};

use miniscript::descriptor::DescriptorTrait;

use super::coin_selection::{CoinSelectionAlgorithm, DefaultCoinSelectionAlgorithm};
use crate::{database::BatchDatabase, Error, Utxo, Wallet};
use crate::{
    types::{FeeRate, KeychainKind, LocalUtxo, WeightedUtxo},
    TransactionDetails,
};
/// Context in which the [`TxBuilder`] is valid
pub trait TxBuilderContext: std::fmt::Debug + Default + Clone {}

/// Marker type to indicate the [`TxBuilder`] is being used to create a new transaction (as opposed
/// to bumping the fee of an existing one).
#[derive(Debug, Default, Clone)]
pub struct CreateTx;
impl TxBuilderContext for CreateTx {}

/// Marker type to indicate the [`TxBuilder`] is being used to bump the fee of an existing transaction.
#[derive(Debug, Default, Clone)]
pub struct BumpFee;
impl TxBuilderContext for BumpFee {}

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
/// # use bdk::*;
/// # use bdk::wallet::tx_builder::*;
/// # use bitcoin::*;
/// # use core::str::FromStr;
/// # let wallet = doctest_wallet!();
/// # let addr1 = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap();
/// # let addr2 = addr1.clone();
/// // chaining
/// let (psbt1, details) = {
///     let mut builder = wallet.build_tx();
///     builder
///         .ordering(TxOrdering::Untouched)
///         .add_recipient(addr1.script_pubkey(), 50_000)
///         .add_recipient(addr2.script_pubkey(), 50_000);
///     builder.finish()?
/// };
///
/// // non-chaining
/// let (psbt2, details) = {
///     let mut builder = wallet.build_tx();
///     builder.ordering(TxOrdering::Untouched);
///     for addr in &[addr1, addr2] {
///         builder.add_recipient(addr.script_pubkey(), 50_000);
///     }
///     builder.finish()?
/// };
///
/// assert_eq!(psbt1.unsigned_tx.output[..2], psbt2.unsigned_tx.output[..2]);
/// # Ok::<(), bdk::Error>(())
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
pub struct TxBuilder<'a, D, Cs, Ctx> {
    pub(crate) wallet: &'a Wallet<D>,
    pub(crate) params: TxParams,
    pub(crate) coin_selection: Cs,
    pub(crate) phantom: PhantomData<Ctx>,
}

/// The parameters for transaction creation sans coin selection algorithm.
//TODO: TxParams should eventually be exposed publicly.
#[derive(Default, Debug, Clone)]
pub(crate) struct TxParams {
    pub(crate) recipients: Vec<(Script, u64)>,
    pub(crate) drain_wallet: bool,
    pub(crate) drain_to: Option<Script>,
    pub(crate) fee_policy: Option<FeePolicy>,
    pub(crate) internal_policy_path: Option<BTreeMap<String, Vec<usize>>>,
    pub(crate) external_policy_path: Option<BTreeMap<String, Vec<usize>>>,
    pub(crate) utxos: Vec<WeightedUtxo>,
    pub(crate) unspendable: HashSet<OutPoint>,
    pub(crate) manually_selected_only: bool,
    pub(crate) sighash: Option<psbt::PsbtSighashType>,
    pub(crate) ordering: TxOrdering,
    pub(crate) locktime: Option<u32>,
    pub(crate) rbf: Option<RbfValue>,
    pub(crate) version: Option<Version>,
    pub(crate) change_policy: ChangeSpendPolicy,
    pub(crate) only_witness_utxo: bool,
    pub(crate) add_global_xpubs: bool,
    pub(crate) include_output_redeem_witness_script: bool,
    pub(crate) bumping_fee: Option<PreviousFee>,
    pub(crate) current_height: Option<u32>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PreviousFee {
    pub absolute: u64,
    pub rate: f32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum FeePolicy {
    FeeRate(FeeRate),
    FeeAmount(u64),
}

impl std::default::Default for FeePolicy {
    fn default() -> Self {
        FeePolicy::FeeRate(FeeRate::default_min_relay_fee())
    }
}

impl<'a, Cs: Clone, Ctx, D> Clone for TxBuilder<'a, D, Cs, Ctx> {
    fn clone(&self) -> Self {
        TxBuilder {
            wallet: self.wallet,
            params: self.params.clone(),
            coin_selection: self.coin_selection.clone(),
            phantom: PhantomData,
        }
    }
}

// methods supported by both contexts, for any CoinSelectionAlgorithm
impl<'a, D: BatchDatabase, Cs: CoinSelectionAlgorithm<D>, Ctx: TxBuilderContext>
    TxBuilder<'a, D, Cs, Ctx>
{
    /// Set a custom fee rate
    pub fn fee_rate(&mut self, fee_rate: FeeRate) -> &mut Self {
        self.params.fee_policy = Some(FeePolicy::FeeRate(fee_rate));
        self
    }

    /// Set an absolute fee
    pub fn fee_absolute(&mut self, fee_amount: u64) -> &mut Self {
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
    /// # use bdk::*;
    /// # let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap();
    /// # let wallet = doctest_wallet!();
    /// let mut path = BTreeMap::new();
    /// path.insert("aabbccdd".to_string(), vec![0, 1]);
    ///
    /// let builder = wallet
    ///     .build_tx()
    ///     .add_recipient(to_address.script_pubkey(), 50_000)
    ///     .policy_path(path, KeychainKind::External);
    ///
    /// # Ok::<(), bdk::Error>(())
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
    pub fn add_utxos(&mut self, outpoints: &[OutPoint]) -> Result<&mut Self, Error> {
        let utxos = outpoints
            .iter()
            .map(|outpoint| self.wallet.get_utxo(*outpoint)?.ok_or(Error::UnknownUtxo))
            .collect::<Result<Vec<_>, _>>()?;

        for utxo in utxos {
            let descriptor = self.wallet.get_descriptor_for_keychain(utxo.keychain);
            let satisfaction_weight = descriptor.max_satisfaction_weight().unwrap();
            self.params.utxos.push(WeightedUtxo {
                satisfaction_weight,
                utxo: Utxo::Local(utxo),
            });
        }

        Ok(self)
    }

    /// Add a utxo to the internal list of utxos that **must** be spent
    ///
    /// These have priority over the "unspendable" utxos, meaning that if a utxo is present both in
    /// the "utxos" and the "unspendable" list, it will be spent.
    pub fn add_utxo(&mut self, outpoint: OutPoint) -> Result<&mut Self, Error> {
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
    /// To guarantee the `satisfaction_weight` is correct, you can require the party providing the
    /// `psbt_input` provide a miniscript descriptor for the input so you can check it against the
    /// `script_pubkey` and then ask it for the [`max_satisfaction_weight`].
    ///
    /// This is an **EXPERIMENTAL** feature, API and other major changes are expected.
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
    /// [`max_satisfaction_weight`]: miniscript::Descriptor::max_satisfaction_weight
    pub fn add_foreign_utxo(
        &mut self,
        outpoint: OutPoint,
        psbt_input: psbt::Input,
        satisfaction_weight: usize,
    ) -> Result<&mut Self, Error> {
        if psbt_input.witness_utxo.is_none() {
            match psbt_input.non_witness_utxo.as_ref() {
                Some(tx) => {
                    if tx.txid() != outpoint.txid {
                        return Err(Error::Generic(
                            "Foreign utxo outpoint does not match PSBT input".into(),
                        ));
                    }
                    if tx.output.len() <= outpoint.vout as usize {
                        return Err(Error::InvalidOutpoint(outpoint));
                    }
                }
                None => {
                    return Err(Error::Generic(
                        "Foreign utxo missing witness_utxo or non_witness_utxo".into(),
                    ))
                }
            }
        }

        self.params.utxos.push(WeightedUtxo {
            satisfaction_weight,
            utxo: Utxo::Foreign {
                outpoint,
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
    pub fn nlocktime(&mut self, locktime: u32) -> &mut Self {
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
    /// [`TxBuilder::unspendable`].
    pub fn do_not_spend_change(&mut self) -> &mut Self {
        self.params.change_policy = ChangeSpendPolicy::ChangeForbidden;
        self
    }

    /// Only spend change outputs
    ///
    /// This effectively adds all the non-change outputs to the "unspendable" list. See
    /// [`TxBuilder::unspendable`].
    pub fn only_spend_change(&mut self) -> &mut Self {
        self.params.change_policy = ChangeSpendPolicy::OnlyChange;
        self
    }

    /// Set a specific [`ChangeSpendPolicy`]. See [`TxBuilder::do_not_spend_change`] and
    /// [`TxBuilder::only_spend_change`] for some shortcuts.
    pub fn change_policy(&mut self, change_policy: ChangeSpendPolicy) -> &mut Self {
        self.params.change_policy = change_policy;
        self
    }

    /// Only Fill-in the [`psbt::Input::witness_utxo`](bitcoin::util::psbt::Input::witness_utxo) field when spending from
    /// SegWit descriptors.
    ///
    /// This reduces the size of the PSBT, but some signers might reject them due to the lack of
    /// the `non_witness_utxo`.
    pub fn only_witness_utxo(&mut self) -> &mut Self {
        self.params.only_witness_utxo = true;
        self
    }

    /// Fill-in the [`psbt::Output::redeem_script`](bitcoin::util::psbt::Output::redeem_script) and
    /// [`psbt::Output::witness_script`](bitcoin::util::psbt::Output::witness_script) fields.
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
    /// Overrides the [`DefaultCoinSelectionAlgorithm`](super::coin_selection::DefaultCoinSelectionAlgorithm).
    ///
    /// Note that this function consumes the builder and returns it so it is usually best to put this as the first call on the builder.
    pub fn coin_selection<P: CoinSelectionAlgorithm<D>>(
        self,
        coin_selection: P,
    ) -> TxBuilder<'a, D, P, Ctx> {
        TxBuilder {
            wallet: self.wallet,
            params: self.params,
            coin_selection,
            phantom: PhantomData,
        }
    }

    /// Finish the building the transaction.
    ///
    /// Returns the [`BIP174`] "PSBT" and summary details about the transaction.
    ///
    /// [`BIP174`]: https://github.com/bitcoin/bips/blob/master/bip-0174.mediawiki
    pub fn finish(self) -> Result<(Psbt, TransactionDetails), Error> {
        self.wallet.create_tx(self.coin_selection, self.params)
    }

    /// Enable signaling RBF
    ///
    /// This will use the default nSequence value of `0xFFFFFFFD`.
    pub fn enable_rbf(&mut self) -> &mut Self {
        self.params.rbf = Some(RbfValue::Default);
        self
    }

    /// Enable signaling RBF with a specific nSequence value
    ///
    /// This can cause conflicts if the wallet's descriptors contain an "older" (OP_CSV) operator
    /// and the given `nsequence` is lower than the CSV value.
    ///
    /// If the `nsequence` is higher than `0xFFFFFFFD` an error will be thrown, since it would not
    /// be a valid nSequence to signal RBF.
    pub fn enable_rbf_with_sequence(&mut self, nsequence: u32) -> &mut Self {
        self.params.rbf = Some(RbfValue::Value(nsequence));
        self
    }

    /// Set the current blockchain height.
    ///
    /// This will be used to set the nLockTime for preventing fee sniping. If the current height is
    /// not provided, the last sync height will be used instead.
    ///
    /// **Note**: This will be ignored if you manually specify a nlocktime using [`TxBuilder::nlocktime`].
    pub fn set_current_height(&mut self, height: u32) -> &mut Self {
        self.params.current_height = Some(height);
        self
    }
}

impl<'a, D: BatchDatabase, Cs: CoinSelectionAlgorithm<D>> TxBuilder<'a, D, Cs, CreateTx> {
    /// Replace the recipients already added with a new list
    pub fn set_recipients(&mut self, recipients: Vec<(Script, u64)>) -> &mut Self {
        self.params.recipients = recipients;
        self
    }

    /// Add a recipient to the internal list
    pub fn add_recipient(&mut self, script_pubkey: Script, amount: u64) -> &mut Self {
        self.params.recipients.push((script_pubkey, amount));
        self
    }

    /// Add data as an output, using OP_RETURN
    pub fn add_data(&mut self, data: &[u8]) -> &mut Self {
        let script = Script::new_op_return(data);
        self.add_recipient(script, 0u64);
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
    /// If you choose not to set any recipients, you should either provide the utxos that the
    /// transaction should spend via [`add_utxos`], or set [`drain_wallet`] to spend all of them.
    ///
    /// When bumping the fees of a transaction made with this option, you probably want to
    /// use [`allow_shrinking`] to allow this output to be reduced to pay for the extra fees.
    ///
    /// # Example
    ///
    /// `drain_to` is very useful for draining all the coins in a wallet with [`drain_wallet`] to a
    /// single address.
    ///
    /// ```
    /// # use std::str::FromStr;
    /// # use bitcoin::*;
    /// # use bdk::*;
    /// # use bdk::wallet::tx_builder::CreateTx;
    /// # let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap();
    /// # let wallet = doctest_wallet!();
    /// let mut tx_builder = wallet.build_tx();
    ///
    /// tx_builder
    ///     // Spend all outputs in this wallet.
    ///     .drain_wallet()
    ///     // Send the excess (which is all the coins minus the fee) to this address.
    ///     .drain_to(to_address.script_pubkey())
    ///     .fee_rate(FeeRate::from_sat_per_vb(5.0))
    ///     .enable_rbf();
    /// let (psbt, tx_details) = tx_builder.finish()?;
    /// # Ok::<(), bdk::Error>(())
    /// ```
    ///
    /// [`allow_shrinking`]: Self::allow_shrinking
    /// [`add_recipient`]: Self::add_recipient
    /// [`add_utxos`]: Self::add_utxos
    /// [`drain_wallet`]: Self::drain_wallet
    pub fn drain_to(&mut self, script_pubkey: Script) -> &mut Self {
        self.params.drain_to = Some(script_pubkey);
        self
    }
}

// methods supported only by bump_fee
impl<'a, D: BatchDatabase> TxBuilder<'a, D, DefaultCoinSelectionAlgorithm, BumpFee> {
    /// Explicitly tells the wallet that it is allowed to reduce the amount of the output matching this
    /// `script_pubkey` in order to bump the transaction fee. Without specifying this the wallet
    /// will attempt to find a change output to shrink instead.
    ///
    /// **Note** that the output may shrink to below the dust limit and therefore be removed. If it is
    /// preserved then it is currently not guaranteed to be in the same position as it was
    /// originally.
    ///
    /// Returns an `Err` if `script_pubkey` can't be found among the recipients of the
    /// transaction we are bumping.
    pub fn allow_shrinking(&mut self, script_pubkey: Script) -> Result<&mut Self, Error> {
        match self
            .params
            .recipients
            .iter()
            .position(|(recipient_script, _)| *recipient_script == script_pubkey)
        {
            Some(position) => {
                self.params.recipients.remove(position);
                self.params.drain_to = Some(script_pubkey);
                Ok(self)
            }
            None => Err(Error::Generic(format!(
                "{} was not in the original transaction",
                script_pubkey
            ))),
        }
    }
}

/// Ordering of the transaction's inputs and outputs
#[derive(Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Clone, Copy)]
pub enum TxOrdering {
    /// Randomized (default)
    Shuffle,
    /// Unchanged
    Untouched,
    /// BIP69 / Lexicographic
    Bip69Lexicographic,
}

impl Default for TxOrdering {
    fn default() -> Self {
        TxOrdering::Shuffle
    }
}

impl TxOrdering {
    /// Sort transaction inputs and outputs by [`TxOrdering`] variant
    pub fn sort_tx(&self, tx: &mut Transaction) {
        match self {
            TxOrdering::Untouched => {}
            TxOrdering::Shuffle => {
                use rand::seq::SliceRandom;
                #[cfg(test)]
                use rand::SeedableRng;

                #[cfg(not(test))]
                let mut rng = rand::thread_rng();
                #[cfg(test)]
                let mut rng = rand::rngs::StdRng::seed_from_u64(0);

                tx.output.shuffle(&mut rng);
            }
            TxOrdering::Bip69Lexicographic => {
                tx.input.sort_unstable_by_key(|txin| {
                    (txin.previous_output.txid, txin.previous_output.vout)
                });
                tx.output
                    .sort_unstable_by_key(|txout| (txout.value, txout.script_pubkey.clone()));
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

/// RBF nSequence value
///
/// Has a default value of `0xFFFFFFFD`
#[derive(Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Clone, Copy)]
pub(crate) enum RbfValue {
    Default,
    Value(u32),
}

impl RbfValue {
    pub(crate) fn get_value(&self) -> u32 {
        match self {
            RbfValue::Default => 0xFFFFFFFD,
            RbfValue::Value(v) => *v,
        }
    }
}

/// Policy regarding the use of change outputs when creating a transaction
#[derive(Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Clone, Copy)]
pub enum ChangeSpendPolicy {
    /// Use both change and non-change outputs (default)
    ChangeAllowed,
    /// Only use change outputs (see [`TxBuilder::only_spend_change`])
    OnlyChange,
    /// Only use non-change outputs (see [`TxBuilder::do_not_spend_change`])
    ChangeForbidden,
}

impl Default for ChangeSpendPolicy {
    fn default() -> Self {
        ChangeSpendPolicy::ChangeAllowed
    }
}

impl ChangeSpendPolicy {
    pub(crate) fn is_satisfied_by(&self, utxo: &LocalUtxo) -> bool {
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
    use bitcoin::hashes::hex::FromHex;

    use super::*;

    #[test]
    fn test_output_ordering_default_shuffle() {
        assert_eq!(TxOrdering::default(), TxOrdering::Shuffle);
    }

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

        TxOrdering::Shuffle.sort_tx(&mut tx);

        assert_eq!(original_tx.input, tx.input);
        assert_ne!(original_tx.output, tx.output);
    }

    #[test]
    fn test_output_ordering_bip69() {
        use std::str::FromStr;

        let original_tx = ordering_test_tx!();
        let mut tx = original_tx;

        TxOrdering::Bip69Lexicographic.sort_tx(&mut tx);

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

        assert_eq!(tx.output[0].value, 800);
        assert_eq!(tx.output[1].script_pubkey, From::from(vec![0xAA]));
        assert_eq!(tx.output[2].script_pubkey, From::from(vec![0xAA, 0xEE]));
    }

    fn get_test_utxos() -> Vec<LocalUtxo> {
        vec![
            LocalUtxo {
                outpoint: OutPoint {
                    txid: Default::default(),
                    vout: 0,
                },
                txout: Default::default(),
                keychain: KeychainKind::External,
                is_spent: false,
            },
            LocalUtxo {
                outpoint: OutPoint {
                    txid: Default::default(),
                    vout: 1,
                },
                txout: Default::default(),
                keychain: KeychainKind::Internal,
                is_spent: false,
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
