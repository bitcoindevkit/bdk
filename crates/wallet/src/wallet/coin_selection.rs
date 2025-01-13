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

//! Coin selection
//!
//! This module provides the trait [`CoinSelectionAlgorithm`] that can be implemented to
//! define custom coin selection algorithms.
//!
//! You can specify a custom coin selection algorithm through the [`coin_selection`] method on
//! [`TxBuilder`]. [`DefaultCoinSelectionAlgorithm`] aliases the coin selection algorithm that will
//! be used if it is not explicitly set.
//!
//! [`TxBuilder`]: super::tx_builder::TxBuilder
//! [`coin_selection`]: super::tx_builder::TxBuilder::coin_selection
//!
//! ## Example
//!
//! ```
//! # use std::str::FromStr;
//! # use bitcoin::*;
//! # use bdk_wallet::{self, ChangeSet, coin_selection::*, coin_selection};
//! # use bdk_wallet::error::CreateTxError;
//! # use bdk_wallet::*;
//! # use bdk_wallet::coin_selection::decide_change;
//! # use anyhow::Error;
//! # use rand_core::RngCore;
//! #[derive(Debug)]
//! struct AlwaysSpendEverything;
//!
//! impl CoinSelectionAlgorithm for AlwaysSpendEverything {
//!     fn coin_select<R: RngCore>(
//!         &self,
//!         required_utxos: Vec<WeightedUtxo>,
//!         optional_utxos: Vec<WeightedUtxo>,
//!         fee_rate: FeeRate,
//!         target_amount: Amount,
//!         drain_script: &Script,
//!         rand: &mut R,
//!     ) -> Result<CoinSelectionResult, coin_selection::InsufficientFunds> {
//!         let mut selected_amount = Amount::ZERO;
//!         let mut additional_weight = Weight::ZERO;
//!         let all_utxos_selected = required_utxos
//!             .into_iter()
//!             .chain(optional_utxos)
//!             .scan(
//!                 (&mut selected_amount, &mut additional_weight),
//!                 |(selected_amount, additional_weight), weighted_utxo| {
//!                     **selected_amount += weighted_utxo.utxo.txout().value;
//!                     **additional_weight += TxIn::default()
//!                         .segwit_weight()
//!                         .checked_add(weighted_utxo.satisfaction_weight)
//!                         .expect("`Weight` addition should not cause an integer overflow");
//!                     Some(weighted_utxo.utxo)
//!                 },
//!             )
//!             .collect::<Vec<_>>();
//!         let additional_fees = fee_rate * additional_weight;
//!         let amount_needed_with_fees = additional_fees + target_amount;
//!         if selected_amount < amount_needed_with_fees {
//!             return Err(coin_selection::InsufficientFunds {
//!                 needed: amount_needed_with_fees,
//!                 available: selected_amount,
//!             });
//!         }
//!
//!         let remaining_amount = selected_amount - amount_needed_with_fees;
//!
//!         let excess = decide_change(remaining_amount, fee_rate, drain_script);
//!
//!         Ok(CoinSelectionResult {
//!             selected: all_utxos_selected,
//!             fee_amount: additional_fees,
//!             excess,
//!         })
//!     }
//! }
//!
//! # let mut wallet = doctest_wallet!();
//! // create wallet, sync, ...
//!
//! let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt")
//!     .unwrap()
//!     .require_network(Network::Testnet)
//!     .unwrap();
//! let psbt = {
//!     let mut builder = wallet.build_tx().coin_selection(AlwaysSpendEverything);
//!     builder.add_recipient(to_address.script_pubkey(), Amount::from_sat(50_000));
//!     builder.finish()?
//! };
//!
//! // inspect, sign, broadcast, ...
//!
//! # Ok::<(), anyhow::Error>(())
//! ```

use crate::wallet::utils::IsDust;
use crate::Utxo;
use crate::WeightedUtxo;
use bitcoin::{Amount, FeeRate, SignedAmount};

use alloc::vec::Vec;
use bitcoin::consensus::encode::serialize;
use bitcoin::TxIn;
use bitcoin::{Script, Weight};

use core::convert::TryInto;
use core::fmt::{self, Formatter};
use rand_core::RngCore;

use super::utils::shuffle_slice;
/// Default coin selection algorithm used by [`TxBuilder`](super::tx_builder::TxBuilder) if not
/// overridden
pub type DefaultCoinSelectionAlgorithm = BranchAndBoundCoinSelection<SingleRandomDraw>;

/// Wallet's UTXO set is not enough to cover recipient's requested plus fee.
///
/// This is thrown by [`CoinSelectionAlgorithm`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsufficientFunds {
    /// Amount needed for the transaction
    pub needed: Amount,
    /// Amount available for spending
    pub available: Amount,
}

impl fmt::Display for InsufficientFunds {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Insufficient funds: {} available of {} needed",
            self.available, self.needed
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for InsufficientFunds {}

#[derive(Debug)]
/// Remaining amount after performing coin selection
pub enum Excess {
    /// It's not possible to create spendable output from excess using the current drain output
    NoChange {
        /// Threshold to consider amount as dust for this particular change script_pubkey
        dust_threshold: Amount,
        /// Exceeding amount of current selection over outgoing value and fee costs
        remaining_amount: Amount,
        /// The calculated fee for the drain TxOut with the selected script_pubkey
        change_fee: Amount,
    },
    /// It's possible to create spendable output from excess using the current drain output
    Change {
        /// Effective amount available to create change after deducting the change output fee
        amount: Amount,
        /// The deducted change output fee
        fee: Amount,
    },
}

/// Result of a successful coin selection
#[derive(Debug)]
pub struct CoinSelectionResult {
    /// List of outputs selected for use as inputs
    pub selected: Vec<Utxo>,
    /// Total fee amount for the selected utxos
    pub fee_amount: Amount,
    /// Remaining amount after deducing fees and outgoing outputs
    pub excess: Excess,
}

impl CoinSelectionResult {
    /// The total value of the inputs selected.
    pub fn selected_amount(&self) -> Amount {
        self.selected.iter().map(|u| u.txout().value).sum()
    }

    /// The total value of the inputs selected from the local wallet.
    pub fn local_selected_amount(&self) -> Amount {
        self.selected
            .iter()
            .filter_map(|u| match u {
                Utxo::Local(_) => Some(u.txout().value),
                _ => None,
            })
            .sum()
    }
}

/// Trait for generalized coin selection algorithms
///
/// This trait can be implemented to make the [`Wallet`](super::Wallet) use a customized coin
/// selection algorithm when it creates transactions.
///
/// For an example see [this module](crate::wallet::coin_selection)'s documentation.
pub trait CoinSelectionAlgorithm: core::fmt::Debug {
    /// Perform the coin selection
    ///
    /// - `required_utxos`: the utxos that must be spent regardless of `target_amount` with their
    ///                     weight cost
    /// - `optional_utxos`: the remaining available utxos to satisfy `target_amount` with their
    ///                     weight cost
    /// - `fee_rate`: fee rate to use
    /// - `target_amount`: the outgoing amount and the fees already accumulated from adding
    ///                    outputs and transaction’s header.
    /// - `drain_script`: the script to use in case of change
    /// - `rand`: random number generated used by some coin selection algorithms such as [`SingleRandomDraw`]
    fn coin_select<R: RngCore>(
        &self,
        required_utxos: Vec<WeightedUtxo>,
        optional_utxos: Vec<WeightedUtxo>,
        fee_rate: FeeRate,
        target_amount: Amount,
        drain_script: &Script,
        rand: &mut R,
    ) -> Result<CoinSelectionResult, InsufficientFunds>;
}

/// Simple and dumb coin selection
///
/// This coin selection algorithm sorts the available UTXOs by value and then picks them starting
/// from the largest ones until the required amount is reached.
#[derive(Debug, Default, Clone, Copy)]
pub struct LargestFirstCoinSelection;

impl CoinSelectionAlgorithm for LargestFirstCoinSelection {
    fn coin_select<R: RngCore>(
        &self,
        required_utxos: Vec<WeightedUtxo>,
        mut optional_utxos: Vec<WeightedUtxo>,
        fee_rate: FeeRate,
        target_amount: Amount,
        drain_script: &Script,
        _: &mut R,
    ) -> Result<CoinSelectionResult, InsufficientFunds> {
        // We put the "required UTXOs" first and make sure the optional UTXOs are sorted,
        // initially smallest to largest, before being reversed with `.rev()`.
        let utxos = {
            optional_utxos.sort_unstable_by_key(|wu| wu.utxo.txout().value);
            required_utxos
                .into_iter()
                .map(|utxo| (true, utxo))
                .chain(optional_utxos.into_iter().rev().map(|utxo| (false, utxo)))
        };

        select_sorted_utxos(utxos, fee_rate, target_amount, drain_script)
    }
}

/// OldestFirstCoinSelection always picks the utxo with the smallest blockheight to add to the selected coins next
///
/// This coin selection algorithm sorts the available UTXOs by blockheight and then picks them starting
/// from the oldest ones until the required amount is reached.
#[derive(Debug, Default, Clone, Copy)]
pub struct OldestFirstCoinSelection;

impl CoinSelectionAlgorithm for OldestFirstCoinSelection {
    fn coin_select<R: RngCore>(
        &self,
        required_utxos: Vec<WeightedUtxo>,
        mut optional_utxos: Vec<WeightedUtxo>,
        fee_rate: FeeRate,
        target_amount: Amount,
        drain_script: &Script,
        _: &mut R,
    ) -> Result<CoinSelectionResult, InsufficientFunds> {
        // We put the "required UTXOs" first and make sure the optional UTXOs are sorted from
        // oldest to newest according to blocktime
        // For utxo that doesn't exist in DB, they will have lowest priority to be selected
        let utxos = {
            optional_utxos.sort_unstable_by_key(|wu| match &wu.utxo {
                Utxo::Local(local) => Some(local.chain_position),
                Utxo::Foreign { .. } => None,
            });

            required_utxos
                .into_iter()
                .map(|utxo| (true, utxo))
                .chain(optional_utxos.into_iter().map(|utxo| (false, utxo)))
        };

        select_sorted_utxos(utxos, fee_rate, target_amount, drain_script)
    }
}

/// Decide if change can be created
///
/// - `remaining_amount`: the amount in which the selected coins exceed the target amount
/// - `fee_rate`: required fee rate for the current selection
/// - `drain_script`: script to consider change creation
pub fn decide_change(remaining_amount: Amount, fee_rate: FeeRate, drain_script: &Script) -> Excess {
    // drain_output_len = size(len(script_pubkey)) + len(script_pubkey) + size(output_value)
    let drain_output_len = serialize(drain_script).len() + 8usize;
    let change_fee =
        fee_rate * Weight::from_vb(drain_output_len as u64).expect("overflow occurred");
    let drain_val = remaining_amount.checked_sub(change_fee).unwrap_or_default();

    if drain_val.is_dust(drain_script) {
        let dust_threshold = drain_script.minimal_non_dust();
        Excess::NoChange {
            dust_threshold,
            change_fee,
            remaining_amount,
        }
    } else {
        Excess::Change {
            amount: drain_val,
            fee: change_fee,
        }
    }
}

fn select_sorted_utxos(
    utxos: impl Iterator<Item = (bool, WeightedUtxo)>,
    fee_rate: FeeRate,
    target_amount: Amount,
    drain_script: &Script,
) -> Result<CoinSelectionResult, InsufficientFunds> {
    let mut selected_amount = Amount::ZERO;
    let mut fee_amount = Amount::ZERO;
    let selected = utxos
        .scan(
            (&mut selected_amount, &mut fee_amount),
            |(selected_amount, fee_amount), (must_use, weighted_utxo)| {
                if must_use || **selected_amount < target_amount + **fee_amount {
                    **fee_amount += fee_rate
                        * TxIn::default()
                            .segwit_weight()
                            .checked_add(weighted_utxo.satisfaction_weight)
                            .expect("`Weight` addition should not cause an integer overflow");
                    **selected_amount += weighted_utxo.utxo.txout().value;
                    Some(weighted_utxo.utxo)
                } else {
                    None
                }
            },
        )
        .collect::<Vec<_>>();

    let amount_needed_with_fees = target_amount + fee_amount;
    if selected_amount < amount_needed_with_fees {
        return Err(InsufficientFunds {
            needed: amount_needed_with_fees,
            available: selected_amount,
        });
    }

    let remaining_amount = selected_amount - amount_needed_with_fees;

    let excess = decide_change(remaining_amount, fee_rate, drain_script);

    Ok(CoinSelectionResult {
        selected,
        fee_amount,
        excess,
    })
}

#[derive(Debug, Clone)]
// Adds fee information to an UTXO.
struct OutputGroup {
    weighted_utxo: WeightedUtxo,
    // Amount of fees for spending a certain utxo, calculated using a certain FeeRate
    fee: Amount,
    // The effective value of the UTXO, i.e., the utxo value minus the fee for spending it
    effective_value: SignedAmount,
}

impl OutputGroup {
    fn new(weighted_utxo: WeightedUtxo, fee_rate: FeeRate) -> Self {
        let fee = fee_rate
            * TxIn::default()
                .segwit_weight()
                .checked_add(weighted_utxo.satisfaction_weight)
                .expect("`Weight` addition should not cause an integer overflow");
        let effective_value = weighted_utxo
            .utxo
            .txout()
            .value
            .to_signed()
            .expect("signed amount")
            - fee.to_signed().expect("signed amount");
        OutputGroup {
            weighted_utxo,
            fee,
            effective_value,
        }
    }
}

/// Branch and bound coin selection
///
/// Code adapted from Bitcoin Core's implementation and from Mark Erhardt Master's Thesis: <http://murch.one/wp-content/uploads/2016/11/erhardt2016coinselection.pdf>
#[derive(Debug, Clone)]
pub struct BranchAndBoundCoinSelection<Cs = SingleRandomDraw> {
    size_of_change: u64,
    fallback_algorithm: Cs,
}

/// Error returned by branch and bound coin selection.
#[derive(Debug)]
enum BnbError {
    /// Branch and bound coin selection tries to avoid needing a change by finding the right inputs for
    /// the desired outputs plus fee, if there is not such combination this error is thrown
    NoExactMatch,
    /// Branch and bound coin selection possible attempts with sufficiently big UTXO set could grow
    /// exponentially, thus a limit is set, and when hit, this error is thrown
    TotalTriesExceeded,
}

impl<Cs: Default> Default for BranchAndBoundCoinSelection<Cs> {
    fn default() -> Self {
        Self {
            // P2WPKH cost of change -> value (8 bytes) + script len (1 bytes) + script (22 bytes)
            size_of_change: 8 + 1 + 22,
            fallback_algorithm: Cs::default(),
        }
    }
}

impl<Cs> BranchAndBoundCoinSelection<Cs> {
    /// Create new instance with a target `size_of_change` and `fallback_algorithm`.
    pub fn new(size_of_change: u64, fallback_algorithm: Cs) -> Self {
        Self {
            size_of_change,
            fallback_algorithm,
        }
    }
}

const BNB_TOTAL_TRIES: usize = 100_000;

impl<Cs: CoinSelectionAlgorithm> CoinSelectionAlgorithm for BranchAndBoundCoinSelection<Cs> {
    fn coin_select<R: RngCore>(
        &self,
        required_utxos: Vec<WeightedUtxo>,
        optional_utxos: Vec<WeightedUtxo>,
        fee_rate: FeeRate,
        target_amount: Amount,
        drain_script: &Script,
        rand: &mut R,
    ) -> Result<CoinSelectionResult, InsufficientFunds> {
        // Mapping every (UTXO, usize) to an output group
        let required_ogs: Vec<OutputGroup> = required_utxos
            .iter()
            .map(|u| OutputGroup::new(u.clone(), fee_rate))
            .collect();

        // Mapping every (UTXO, usize) to an output group, filtering UTXOs with a negative
        // effective value
        let optional_ogs: Vec<OutputGroup> = optional_utxos
            .iter()
            .map(|u| OutputGroup::new(u.clone(), fee_rate))
            .filter(|u| u.effective_value.is_positive())
            .collect();

        let curr_value = required_ogs
            .iter()
            .fold(SignedAmount::ZERO, |acc, x| acc + x.effective_value);

        let curr_available_value = optional_ogs
            .iter()
            .fold(SignedAmount::ZERO, |acc, x| acc + x.effective_value);

        let cost_of_change = (Weight::from_vb(self.size_of_change).expect("overflow occurred")
            * fee_rate)
            .to_signed()
            .expect("signed amount");

        // `curr_value` and `curr_available_value` are both the sum of *effective_values* of
        // the UTXOs. For the optional UTXOs (curr_available_value) we filter out UTXOs with
        // negative effective value, so it will always be positive.
        //
        // Since we are required to spend the required UTXOs (curr_value) we have to consider
        // all their effective values, even when negative, which means that curr_value could
        // be negative as well.
        //
        // If the sum of curr_value and curr_available_value is negative or lower than our target,
        // we can immediately exit with an error, as it's guaranteed we will never find a solution
        // if we actually run the BnB.
        let total_value: Result<Amount, _> = (curr_available_value + curr_value).try_into();
        match total_value {
            Ok(v) if v >= target_amount => {}
            _ => {
                // Assume we spend all the UTXOs we can (all the required + all the optional with
                // positive effective value), sum their value and their fee cost.
                let (utxo_fees, utxo_value) = required_ogs.iter().chain(optional_ogs.iter()).fold(
                    (Amount::ZERO, Amount::ZERO),
                    |(mut fees, mut value), utxo| {
                        fees += utxo.fee;
                        value += utxo.weighted_utxo.utxo.txout().value;
                        (fees, value)
                    },
                );

                // Add to the target the fee cost of the UTXOs
                return Err(InsufficientFunds {
                    needed: target_amount + utxo_fees,
                    available: utxo_value,
                });
            }
        }

        let signed_target_amount = target_amount
            .try_into()
            .expect("Bitcoin amount to fit into i64");

        if curr_value > signed_target_amount {
            // remaining_amount can't be negative as that would mean the
            // selection wasn't successful
            // target_amount = amount_needed + (fee_amount - vin_fees)
            let remaining_amount = (curr_value - signed_target_amount)
                .to_unsigned()
                .expect("remaining amount can't be negative");

            let excess = decide_change(remaining_amount, fee_rate, drain_script);

            return Ok(calculate_cs_result(vec![], required_ogs, excess));
        }

        match self.bnb(
            required_ogs,
            optional_ogs,
            curr_value,
            curr_available_value,
            signed_target_amount,
            cost_of_change,
            drain_script,
            fee_rate,
        ) {
            Ok(r) => Ok(r),
            Err(_) => self.fallback_algorithm.coin_select(
                required_utxos,
                optional_utxos,
                fee_rate,
                target_amount,
                drain_script,
                rand,
            ),
        }
    }
}

impl<Cs> BranchAndBoundCoinSelection<Cs> {
    // TODO: make this more Rust-onic :)
    // (And perhaps refactor with less arguments?)
    #[allow(clippy::too_many_arguments)]
    fn bnb(
        &self,
        required_utxos: Vec<OutputGroup>,
        mut optional_utxos: Vec<OutputGroup>,
        mut curr_value: SignedAmount,
        mut curr_available_value: SignedAmount,
        target_amount: SignedAmount,
        cost_of_change: SignedAmount,
        drain_script: &Script,
        fee_rate: FeeRate,
    ) -> Result<CoinSelectionResult, BnbError> {
        // current_selection[i] will contain true if we are using optional_utxos[i],
        // false otherwise. Note that current_selection.len() could be less than
        // optional_utxos.len(), it just means that we still haven't decided if we should keep
        // certain optional_utxos or not.
        let mut current_selection: Vec<bool> = Vec::with_capacity(optional_utxos.len());

        // Sort the utxo_pool
        optional_utxos.sort_unstable_by_key(|a| a.effective_value);
        optional_utxos.reverse();

        // Contains the best selection we found
        let mut best_selection = Vec::new();
        let mut best_selection_value = None;

        // Depth First search loop for choosing the UTXOs
        for _ in 0..BNB_TOTAL_TRIES {
            // Conditions for starting a backtrack
            let mut backtrack = false;
            // Cannot possibly reach target with the amount remaining in the curr_available_value,
            // or the selected value is out of range.
            // Go back and try other branch
            if curr_value + curr_available_value < target_amount
                || curr_value > target_amount + cost_of_change
            {
                backtrack = true;
            } else if curr_value >= target_amount {
                // Selected value is within range, there's no point in going forward. Start
                // backtracking
                backtrack = true;

                // If we found a solution better than the previous one, or if there wasn't previous
                // solution, update the best solution
                if best_selection_value.is_none() || curr_value < best_selection_value.unwrap() {
                    best_selection.clone_from(&current_selection);
                    best_selection_value = Some(curr_value);
                }

                // If we found a perfect match, break here
                if curr_value == target_amount {
                    break;
                }
            }

            // Backtracking, moving backwards
            if backtrack {
                // Walk backwards to find the last included UTXO that still needs to have its omission branch traversed.
                while let Some(false) = current_selection.last() {
                    current_selection.pop();
                    curr_available_value += optional_utxos[current_selection.len()].effective_value;
                }

                if current_selection.last_mut().is_none() {
                    // We have walked back to the first utxo and no branch is untraversed. All solutions searched
                    // If best selection is empty, then there's no exact match
                    if best_selection.is_empty() {
                        return Err(BnbError::NoExactMatch);
                    }
                    break;
                }

                if let Some(c) = current_selection.last_mut() {
                    // Output was included on previous iterations, try excluding now.
                    *c = false;
                }

                let utxo = &optional_utxos[current_selection.len() - 1];
                curr_value -= utxo.effective_value;
            } else {
                // Moving forwards, continuing down this branch
                let utxo = &optional_utxos[current_selection.len()];

                // Remove this utxo from the curr_available_value utxo amount
                curr_available_value -= utxo.effective_value;

                // Inclusion branch first (Largest First Exploration)
                current_selection.push(true);
                curr_value += utxo.effective_value;
            }
        }

        // Check for solution
        if best_selection.is_empty() {
            return Err(BnbError::TotalTriesExceeded);
        }

        // Set output set
        let selected_utxos = optional_utxos
            .into_iter()
            .zip(best_selection)
            .filter_map(|(optional, is_in_best)| if is_in_best { Some(optional) } else { None })
            .collect::<Vec<OutputGroup>>();

        let selected_amount = best_selection_value.unwrap();

        // remaining_amount can't be negative as that would mean the
        // selection wasn't successful
        // target_amount = amount_needed + (fee_amount - vin_fees)
        let remaining_amount = (selected_amount - target_amount)
            .to_unsigned()
            .expect("valid unsigned");

        let excess = decide_change(remaining_amount, fee_rate, drain_script);

        Ok(calculate_cs_result(selected_utxos, required_utxos, excess))
    }
}

/// Pull UTXOs at random until we have enough to meet the target.
#[derive(Debug, Clone, Copy, Default)]
pub struct SingleRandomDraw;

impl CoinSelectionAlgorithm for SingleRandomDraw {
    fn coin_select<R: RngCore>(
        &self,
        required_utxos: Vec<WeightedUtxo>,
        mut optional_utxos: Vec<WeightedUtxo>,
        fee_rate: FeeRate,
        target_amount: Amount,
        drain_script: &Script,
        rand: &mut R,
    ) -> Result<CoinSelectionResult, InsufficientFunds> {
        // We put the required UTXOs first and then the randomize optional UTXOs to take as needed
        let utxos = {
            shuffle_slice(&mut optional_utxos, rand);

            required_utxos
                .into_iter()
                .map(|utxo| (true, utxo))
                .chain(optional_utxos.into_iter().map(|utxo| (false, utxo)))
        };

        // select required UTXOs and then random optional UTXOs.
        select_sorted_utxos(utxos, fee_rate, target_amount, drain_script)
    }
}

fn calculate_cs_result(
    mut selected_utxos: Vec<OutputGroup>,
    mut required_utxos: Vec<OutputGroup>,
    excess: Excess,
) -> CoinSelectionResult {
    selected_utxos.append(&mut required_utxos);
    let fee_amount = selected_utxos.iter().map(|u| u.fee).sum();
    let selected = selected_utxos
        .into_iter()
        .map(|u| u.weighted_utxo.utxo)
        .collect::<Vec<_>>();

    CoinSelectionResult {
        selected,
        fee_amount,
        excess,
    }
}

#[cfg(test)]
mod test {
    use assert_matches::assert_matches;
    use bitcoin::hashes::Hash;
    use bitcoin::OutPoint;
    use chain::{ChainPosition, ConfirmationBlockTime};
    use core::str::FromStr;
    use rand::rngs::StdRng;

    use bitcoin::{Amount, BlockHash, ScriptBuf, TxIn, TxOut};

    use super::*;
    use crate::types::*;

    use rand::prelude::SliceRandom;
    use rand::{thread_rng, Rng, RngCore, SeedableRng};

    // signature len (1WU) + signature and sighash (72WU)
    // + pubkey len (1WU) + pubkey (33WU)
    const P2WPKH_SATISFACTION_SIZE: usize = 1 + 72 + 1 + 33;

    const FEE_AMOUNT: Amount = Amount::from_sat(50);

    fn unconfirmed_utxo(value: Amount, index: u32, last_seen: u64) -> WeightedUtxo {
        utxo(
            value,
            index,
            ChainPosition::Unconfirmed {
                last_seen: Some(last_seen),
            },
        )
    }

    fn confirmed_utxo(
        value: Amount,
        index: u32,
        confirmation_height: u32,
        confirmation_time: u64,
    ) -> WeightedUtxo {
        utxo(
            value,
            index,
            ChainPosition::Confirmed {
                anchor: ConfirmationBlockTime {
                    block_id: chain::BlockId {
                        height: confirmation_height,
                        hash: bitcoin::BlockHash::all_zeros(),
                    },
                    confirmation_time,
                },
                transitively: None,
            },
        )
    }

    fn utxo(
        value: Amount,
        index: u32,
        chain_position: ChainPosition<ConfirmationBlockTime>,
    ) -> WeightedUtxo {
        assert!(index < 10);
        let outpoint = OutPoint::from_str(&format!(
            "000000000000000000000000000000000000000000000000000000000000000{}:0",
            index
        ))
        .unwrap();
        WeightedUtxo {
            satisfaction_weight: Weight::from_wu_usize(P2WPKH_SATISFACTION_SIZE),
            utxo: Utxo::Local(LocalOutput {
                outpoint,
                txout: TxOut {
                    value,
                    script_pubkey: ScriptBuf::new(),
                },
                keychain: KeychainKind::External,
                is_spent: false,
                derivation_index: 42,
                chain_position,
            }),
        }
    }

    fn get_test_utxos() -> Vec<WeightedUtxo> {
        vec![
            unconfirmed_utxo(Amount::from_sat(100_000), 0, 0),
            unconfirmed_utxo(FEE_AMOUNT - Amount::from_sat(40), 1, 0),
            unconfirmed_utxo(Amount::from_sat(200_000), 2, 0),
        ]
    }

    fn get_oldest_first_test_utxos() -> Vec<WeightedUtxo> {
        // ensure utxos are from different tx
        let utxo1 = confirmed_utxo(Amount::from_sat(120_000), 1, 1, 1231006505);
        let utxo2 = confirmed_utxo(Amount::from_sat(80_000), 2, 2, 1231006505);
        let utxo3 = confirmed_utxo(Amount::from_sat(300_000), 3, 3, 1231006505);
        vec![utxo1, utxo2, utxo3]
    }

    fn generate_random_utxos(rng: &mut StdRng, utxos_number: usize) -> Vec<WeightedUtxo> {
        let mut res = Vec::new();
        for i in 0..utxos_number {
            res.push(WeightedUtxo {
                satisfaction_weight: Weight::from_wu_usize(P2WPKH_SATISFACTION_SIZE),
                utxo: Utxo::Local(LocalOutput {
                    outpoint: OutPoint::from_str(&format!(
                        "ebd9813ecebc57ff8f30797de7c205e3c7498ca950ea4341ee51a685ff2fa30a:{}",
                        i
                    ))
                    .unwrap(),
                    txout: TxOut {
                        value: Amount::from_sat(rng.gen_range(0..200000000)),
                        script_pubkey: ScriptBuf::new(),
                    },
                    keychain: KeychainKind::External,
                    is_spent: false,
                    derivation_index: rng.next_u32(),
                    chain_position: if rng.gen_bool(0.5) {
                        ChainPosition::Confirmed {
                            anchor: ConfirmationBlockTime {
                                block_id: chain::BlockId {
                                    height: rng.next_u32(),
                                    hash: BlockHash::all_zeros(),
                                },
                                confirmation_time: rng.next_u64(),
                            },
                            transitively: None,
                        }
                    } else {
                        ChainPosition::Unconfirmed { last_seen: Some(0) }
                    },
                }),
            });
        }
        res
    }

    fn generate_same_value_utxos(utxos_value: Amount, utxos_number: usize) -> Vec<WeightedUtxo> {
        (0..utxos_number)
            .map(|i| WeightedUtxo {
                satisfaction_weight: Weight::from_wu_usize(P2WPKH_SATISFACTION_SIZE),
                utxo: Utxo::Local(LocalOutput {
                    outpoint: OutPoint::from_str(&format!(
                        "ebd9813ecebc57ff8f30797de7c205e3c7498ca950ea4341ee51a685ff2fa30a:{}",
                        i
                    ))
                    .unwrap(),
                    txout: TxOut {
                        value: utxos_value,
                        script_pubkey: ScriptBuf::new(),
                    },
                    keychain: KeychainKind::External,
                    is_spent: false,
                    derivation_index: 42,
                    chain_position: ChainPosition::Unconfirmed { last_seen: Some(0) },
                }),
            })
            .collect()
    }

    fn sum_random_utxos(mut rng: &mut StdRng, utxos: &mut [WeightedUtxo]) -> Amount {
        let utxos_picked_len = rng.gen_range(2..utxos.len() / 2);
        utxos.shuffle(&mut rng);
        utxos[..utxos_picked_len]
            .iter()
            .map(|u| u.utxo.txout().value)
            .sum()
    }

    fn calc_target_amount(utxos: &[WeightedUtxo], fee_rate: FeeRate) -> Amount {
        utxos
            .iter()
            .cloned()
            .map(|utxo| OutputGroup::new(utxo, fee_rate).effective_value)
            .sum::<SignedAmount>()
            .to_unsigned()
            .expect("unsigned amount")
    }

    #[test]
    fn test_largest_first_coin_selection_success() {
        let utxos = get_test_utxos();
        let drain_script = ScriptBuf::default();
        let target_amount = Amount::from_sat(250_000) + FEE_AMOUNT;

        let result = LargestFirstCoinSelection
            .coin_select(
                utxos,
                vec![],
                FeeRate::from_sat_per_vb_unchecked(1),
                target_amount,
                &drain_script,
                &mut thread_rng(),
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), Amount::from_sat(300_010));
        assert_eq!(result.fee_amount, Amount::from_sat(204));
    }

    #[test]
    fn test_largest_first_coin_selection_use_all() {
        let utxos = get_test_utxos();
        let drain_script = ScriptBuf::default();
        let target_amount = Amount::from_sat(20_000) + FEE_AMOUNT;

        let result = LargestFirstCoinSelection
            .coin_select(
                utxos,
                vec![],
                FeeRate::from_sat_per_vb_unchecked(1),
                target_amount,
                &drain_script,
                &mut thread_rng(),
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), Amount::from_sat(300_010));
        assert_eq!(result.fee_amount, Amount::from_sat(204));
    }

    #[test]
    fn test_largest_first_coin_selection_use_only_necessary() {
        let utxos = get_test_utxos();
        let drain_script = ScriptBuf::default();
        let target_amount = Amount::from_sat(20_000) + FEE_AMOUNT;

        let result = LargestFirstCoinSelection
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb_unchecked(1),
                target_amount,
                &drain_script,
                &mut thread_rng(),
            )
            .unwrap();

        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected_amount(), Amount::from_sat(200_000));
        assert_eq!(result.fee_amount, Amount::from_sat(68));
    }

    #[test]
    fn test_largest_first_coin_selection_insufficient_funds() {
        let utxos = get_test_utxos();
        let drain_script = ScriptBuf::default();
        let target_amount = Amount::from_sat(500_000) + FEE_AMOUNT;

        let result = LargestFirstCoinSelection.coin_select(
            vec![],
            utxos,
            FeeRate::from_sat_per_vb_unchecked(1),
            target_amount,
            &drain_script,
            &mut thread_rng(),
        );
        assert!(matches!(result, Err(InsufficientFunds { .. })));
    }

    #[test]
    fn test_largest_first_coin_selection_insufficient_funds_high_fees() {
        let utxos = get_test_utxos();
        let drain_script = ScriptBuf::default();
        let target_amount = Amount::from_sat(250_000) + FEE_AMOUNT;

        let result = LargestFirstCoinSelection.coin_select(
            vec![],
            utxos,
            FeeRate::from_sat_per_vb_unchecked(1000),
            target_amount,
            &drain_script,
            &mut thread_rng(),
        );
        assert!(matches!(result, Err(InsufficientFunds { .. })));
    }

    #[test]
    fn test_oldest_first_coin_selection_success() {
        let utxos = get_oldest_first_test_utxos();
        let drain_script = ScriptBuf::default();
        let target_amount = Amount::from_sat(180_000) + FEE_AMOUNT;

        let result = OldestFirstCoinSelection
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb_unchecked(1),
                target_amount,
                &drain_script,
                &mut thread_rng(),
            )
            .unwrap();

        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected_amount(), Amount::from_sat(200_000));
        assert_eq!(result.fee_amount, Amount::from_sat(136));
    }

    #[test]
    fn test_oldest_first_coin_selection_use_all() {
        let utxos = get_oldest_first_test_utxos();
        let drain_script = ScriptBuf::default();
        let target_amount = Amount::from_sat(20_000) + FEE_AMOUNT;

        let result = OldestFirstCoinSelection
            .coin_select(
                utxos,
                vec![],
                FeeRate::from_sat_per_vb_unchecked(1),
                target_amount,
                &drain_script,
                &mut thread_rng(),
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), Amount::from_sat(500_000));
        assert_eq!(result.fee_amount, Amount::from_sat(204));
    }

    #[test]
    fn test_oldest_first_coin_selection_use_only_necessary() {
        let utxos = get_oldest_first_test_utxos();
        let drain_script = ScriptBuf::default();
        let target_amount = Amount::from_sat(20_000) + FEE_AMOUNT;

        let result = OldestFirstCoinSelection
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb_unchecked(1),
                target_amount,
                &drain_script,
                &mut thread_rng(),
            )
            .unwrap();

        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected_amount(), Amount::from_sat(120_000));
        assert_eq!(result.fee_amount, Amount::from_sat(68));
    }

    #[test]
    fn test_oldest_first_coin_selection_insufficient_funds() {
        let utxos = get_oldest_first_test_utxos();
        let drain_script = ScriptBuf::default();
        let target_amount = Amount::from_sat(600_000) + FEE_AMOUNT;

        let result = OldestFirstCoinSelection.coin_select(
            vec![],
            utxos,
            FeeRate::from_sat_per_vb_unchecked(1),
            target_amount,
            &drain_script,
            &mut thread_rng(),
        );
        assert!(matches!(result, Err(InsufficientFunds { .. })));
    }

    #[test]
    fn test_oldest_first_coin_selection_insufficient_funds_high_fees() {
        let utxos = get_oldest_first_test_utxos();

        let target_amount =
            utxos.iter().map(|wu| wu.utxo.txout().value).sum::<Amount>() - Amount::from_sat(50);
        let drain_script = ScriptBuf::default();

        let result = OldestFirstCoinSelection.coin_select(
            vec![],
            utxos,
            FeeRate::from_sat_per_vb_unchecked(1000),
            target_amount,
            &drain_script,
            &mut thread_rng(),
        );
        assert!(matches!(result, Err(InsufficientFunds { .. })));
    }

    #[test]
    fn test_bnb_coin_selection_success() {
        // In this case bnb won't find a suitable match and single random draw will
        // select three outputs
        let utxos = generate_same_value_utxos(Amount::from_sat(100_000), 20);
        let drain_script = ScriptBuf::default();
        let target_amount = Amount::from_sat(250_000) + FEE_AMOUNT;

        let result = BranchAndBoundCoinSelection::<SingleRandomDraw>::default()
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb_unchecked(1),
                target_amount,
                &drain_script,
                &mut thread_rng(),
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), Amount::from_sat(300_000));
        assert_eq!(result.fee_amount, Amount::from_sat(204));
    }

    #[test]
    fn test_bnb_coin_selection_required_are_enough() {
        let utxos = get_test_utxos();
        let drain_script = ScriptBuf::default();
        let target_amount = Amount::from_sat(20_000) + FEE_AMOUNT;

        let result = BranchAndBoundCoinSelection::<SingleRandomDraw>::default()
            .coin_select(
                utxos.clone(),
                utxos,
                FeeRate::from_sat_per_vb_unchecked(1),
                target_amount,
                &drain_script,
                &mut thread_rng(),
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), Amount::from_sat(300_010));
        assert_eq!(result.fee_amount, Amount::from_sat(204));
    }

    #[test]
    fn test_bnb_coin_selection_optional_are_enough() {
        let utxos = get_test_utxos();
        let drain_script = ScriptBuf::default();
        let fee_rate = FeeRate::BROADCAST_MIN;
        // first and third utxo's effective value
        let target_amount = calc_target_amount(&[utxos[0].clone(), utxos[2].clone()], fee_rate);

        let result = BranchAndBoundCoinSelection::<SingleRandomDraw>::default()
            .coin_select(
                vec![],
                utxos,
                fee_rate,
                target_amount,
                &drain_script,
                &mut thread_rng(),
            )
            .unwrap();

        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected_amount(), Amount::from_sat(300000));
        assert_eq!(result.fee_amount, Amount::from_sat(136));
    }

    #[test]
    fn test_single_random_draw_function_success() {
        let seed = [0; 32];
        let mut rng: StdRng = SeedableRng::from_seed(seed);
        let mut utxos = generate_random_utxos(&mut rng, 300);
        let target_amount = sum_random_utxos(&mut rng, &mut utxos) + FEE_AMOUNT;
        let fee_rate = FeeRate::from_sat_per_vb_unchecked(1);
        let drain_script = ScriptBuf::default();

        let result = SingleRandomDraw.coin_select(
            vec![],
            utxos,
            fee_rate,
            target_amount,
            &drain_script,
            &mut thread_rng(),
        );

        assert!(
            matches!(result, Ok(CoinSelectionResult {selected, fee_amount, ..})
                if selected.iter().map(|u| u.txout().value).sum::<Amount>() > target_amount
                && fee_amount == Amount::from_sat(selected.len() as u64 * 68)
            )
        );
    }

    #[test]
    fn test_single_random_draw_function_error() {
        let seed = [0; 32];
        let mut rng: StdRng = SeedableRng::from_seed(seed);

        // 100_000, 10, 200_000
        let utxos = get_test_utxos();
        let target_amount = Amount::from_sat(300_000) + FEE_AMOUNT;
        let fee_rate = FeeRate::from_sat_per_vb_unchecked(1);
        let drain_script = ScriptBuf::default();

        let result = SingleRandomDraw.coin_select(
            vec![],
            utxos,
            fee_rate,
            target_amount,
            &drain_script,
            &mut rng,
        );

        assert!(matches!(result, Err(InsufficientFunds {needed, available})
                if needed == Amount::from_sat(300_254) && available == Amount::from_sat(300_010)));
    }

    #[test]
    fn test_bnb_coin_selection_required_not_enough() {
        let utxos = get_test_utxos();

        let required = vec![utxos[0].clone()];
        let mut optional = utxos[1..].to_vec();
        optional.push(utxo(
            Amount::from_sat(500_000),
            3,
            ChainPosition::<ConfirmationBlockTime>::Unconfirmed { last_seen: Some(0) },
        ));

        // Defensive assertions, for sanity and in case someone changes the test utxos vector.
        let amount = required
            .iter()
            .map(|u| u.utxo.txout().value)
            .sum::<Amount>();
        assert_eq!(amount, Amount::from_sat(100_000));
        let amount = optional
            .iter()
            .map(|u| u.utxo.txout().value)
            .sum::<Amount>();
        assert!(amount > Amount::from_sat(150_000));
        let drain_script = ScriptBuf::default();

        let fee_rate = FeeRate::BROADCAST_MIN;
        // first and third utxo's effective value
        let target_amount = calc_target_amount(&[utxos[0].clone(), utxos[2].clone()], fee_rate);

        let result = BranchAndBoundCoinSelection::<SingleRandomDraw>::default()
            .coin_select(
                required,
                optional,
                fee_rate,
                target_amount,
                &drain_script,
                &mut thread_rng(),
            )
            .unwrap();

        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected_amount(), Amount::from_sat(300_000));
        assert_eq!(result.fee_amount, Amount::from_sat(136));
    }

    #[test]
    fn test_bnb_coin_selection_insufficient_funds() {
        let utxos = get_test_utxos();
        let drain_script = ScriptBuf::default();
        let target_amount = Amount::from_sat(500_000) + FEE_AMOUNT;

        let result = BranchAndBoundCoinSelection::<SingleRandomDraw>::default().coin_select(
            vec![],
            utxos,
            FeeRate::from_sat_per_vb_unchecked(1),
            target_amount,
            &drain_script,
            &mut thread_rng(),
        );

        assert!(matches!(result, Err(InsufficientFunds { .. })));
    }

    #[test]
    fn test_bnb_coin_selection_insufficient_funds_high_fees() {
        let utxos = get_test_utxos();
        let drain_script = ScriptBuf::default();
        let target_amount = Amount::from_sat(250_000) + FEE_AMOUNT;

        let result = BranchAndBoundCoinSelection::<SingleRandomDraw>::default().coin_select(
            vec![],
            utxos,
            FeeRate::from_sat_per_vb_unchecked(1000),
            target_amount,
            &drain_script,
            &mut thread_rng(),
        );
        assert!(matches!(result, Err(InsufficientFunds { .. })));
    }

    #[test]
    fn test_bnb_coin_selection_check_fee_rate() {
        let utxos = get_test_utxos();
        let drain_script = ScriptBuf::default();
        let fee_rate = FeeRate::BROADCAST_MIN;
        // first utxo's effective value
        let target_amount = calc_target_amount(&utxos[0..1], fee_rate);

        let result = BranchAndBoundCoinSelection::<SingleRandomDraw>::default()
            .coin_select(
                vec![],
                utxos,
                fee_rate,
                target_amount,
                &drain_script,
                &mut thread_rng(),
            )
            .unwrap();

        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected_amount(), Amount::from_sat(100_000));
        let input_weight =
            TxIn::default().segwit_weight().to_wu() + P2WPKH_SATISFACTION_SIZE as u64;
        // the final fee rate should be exactly the same as the fee rate given
        let result_feerate = result.fee_amount / Weight::from_wu(input_weight);
        assert_eq!(result_feerate, fee_rate);
    }

    #[test]
    fn test_bnb_coin_selection_exact_match() {
        let seed = [0; 32];
        let mut rng: StdRng = SeedableRng::from_seed(seed);

        for _i in 0..200 {
            let mut optional_utxos = generate_random_utxos(&mut rng, 16);
            let target_amount = sum_random_utxos(&mut rng, &mut optional_utxos);
            let drain_script = ScriptBuf::default();
            let result = BranchAndBoundCoinSelection::<SingleRandomDraw>::default()
                .coin_select(
                    vec![],
                    optional_utxos,
                    FeeRate::ZERO,
                    target_amount,
                    &drain_script,
                    &mut thread_rng(),
                )
                .unwrap();
            assert_eq!(result.selected_amount(), target_amount);
        }
    }

    #[test]
    fn test_bnb_function_no_exact_match() {
        let fee_rate = FeeRate::from_sat_per_vb_unchecked(10);
        let utxos: Vec<OutputGroup> = get_test_utxos()
            .into_iter()
            .map(|u| OutputGroup::new(u, fee_rate))
            .collect();

        let curr_available_value = utxos
            .iter()
            .fold(SignedAmount::ZERO, |acc, x| acc + x.effective_value);

        let size_of_change = 31;
        let cost_of_change = (Weight::from_vb_unchecked(size_of_change) * fee_rate)
            .to_signed()
            .unwrap();

        let drain_script = ScriptBuf::default();
        let target_amount = SignedAmount::from_sat(20_000) + FEE_AMOUNT.to_signed().unwrap();
        let result = BranchAndBoundCoinSelection::new(size_of_change, SingleRandomDraw).bnb(
            vec![],
            utxos,
            SignedAmount::ZERO,
            curr_available_value,
            target_amount,
            cost_of_change,
            &drain_script,
            fee_rate,
        );
        assert!(matches!(result, Err(BnbError::NoExactMatch)));
    }

    #[test]
    fn test_bnb_function_tries_exceeded() {
        let fee_rate = FeeRate::from_sat_per_vb_unchecked(10);
        let utxos: Vec<OutputGroup> = generate_same_value_utxos(Amount::from_sat(100_000), 100_000)
            .into_iter()
            .map(|u| OutputGroup::new(u, fee_rate))
            .collect();

        let curr_available_value = utxos
            .iter()
            .fold(SignedAmount::ZERO, |acc, x| acc + x.effective_value);

        let size_of_change = 31;
        let cost_of_change = (Weight::from_vb_unchecked(size_of_change) * fee_rate)
            .to_signed()
            .unwrap();
        let target_amount = SignedAmount::from_sat(20_000) + FEE_AMOUNT.to_signed().unwrap();

        let drain_script = ScriptBuf::default();

        let result = BranchAndBoundCoinSelection::new(size_of_change, SingleRandomDraw).bnb(
            vec![],
            utxos,
            SignedAmount::ZERO,
            curr_available_value,
            target_amount,
            cost_of_change,
            &drain_script,
            fee_rate,
        );
        assert!(matches!(result, Err(BnbError::TotalTriesExceeded)));
    }

    // The match won't be exact but still in the range
    #[test]
    fn test_bnb_function_almost_exact_match_with_fees() {
        let fee_rate = FeeRate::from_sat_per_vb_unchecked(1);
        let size_of_change = 31;
        let cost_of_change = (Weight::from_vb_unchecked(size_of_change) * fee_rate)
            .to_signed()
            .unwrap();

        let utxos: Vec<_> = generate_same_value_utxos(Amount::from_sat(50_000), 10)
            .into_iter()
            .map(|u| OutputGroup::new(u, fee_rate))
            .collect();

        let curr_value = SignedAmount::ZERO;

        let curr_available_value = utxos
            .iter()
            .fold(SignedAmount::ZERO, |acc, x| acc + x.effective_value);

        // 2*(value of 1 utxo)  - 2*(1 utxo fees with 1.0sat/vbyte fee rate) -
        // cost_of_change + 5.
        let target_amount = 2 * 50_000 - 2 * 67 - cost_of_change.to_sat() + 5;
        let target_amount = SignedAmount::from_sat(target_amount);

        let drain_script = ScriptBuf::default();

        let result = BranchAndBoundCoinSelection::new(size_of_change, SingleRandomDraw)
            .bnb(
                vec![],
                utxos,
                curr_value,
                curr_available_value,
                target_amount,
                cost_of_change,
                &drain_script,
                fee_rate,
            )
            .unwrap();
        assert_eq!(result.selected_amount(), Amount::from_sat(100_000));
        assert_eq!(result.fee_amount, Amount::from_sat(136));
    }

    // TODO: bnb() function should be optimized, and this test should be done with more utxos
    #[test]
    fn test_bnb_function_exact_match_more_utxos() {
        let seed = [0; 32];
        let mut rng: StdRng = SeedableRng::from_seed(seed);
        let fee_rate = FeeRate::ZERO;

        for _ in 0..200 {
            let optional_utxos: Vec<_> = generate_random_utxos(&mut rng, 40)
                .into_iter()
                .map(|u| OutputGroup::new(u, fee_rate))
                .collect();

            let curr_value = SignedAmount::ZERO;

            let curr_available_value = optional_utxos
                .iter()
                .fold(SignedAmount::ZERO, |acc, x| acc + x.effective_value);

            let target_amount =
                optional_utxos[3].effective_value + optional_utxos[23].effective_value;

            let drain_script = ScriptBuf::default();

            let result = BranchAndBoundCoinSelection::<SingleRandomDraw>::default()
                .bnb(
                    vec![],
                    optional_utxos,
                    curr_value,
                    curr_available_value,
                    target_amount,
                    SignedAmount::ZERO,
                    &drain_script,
                    fee_rate,
                )
                .unwrap();
            assert_eq!(
                result.selected_amount(),
                target_amount.to_unsigned().unwrap()
            );
        }
    }

    #[test]
    fn test_bnb_exclude_negative_effective_value() {
        let utxos = get_test_utxos();
        let drain_script = ScriptBuf::default();

        let selection = BranchAndBoundCoinSelection::<SingleRandomDraw>::default().coin_select(
            vec![],
            utxos,
            FeeRate::from_sat_per_vb_unchecked(10),
            Amount::from_sat(500_000),
            &drain_script,
            &mut thread_rng(),
        );

        assert_matches!(
            selection,
            Err(InsufficientFunds {
                available,
                ..
            }) if available.to_sat() == 300_000
        );
    }

    #[test]
    fn test_bnb_include_negative_effective_value_when_required() {
        let utxos = get_test_utxos();
        let drain_script = ScriptBuf::default();

        let (required, optional) = utxos.into_iter().partition(
            |u| matches!(u, WeightedUtxo { utxo, .. } if utxo.txout().value.to_sat() < 1000),
        );

        let selection = BranchAndBoundCoinSelection::<SingleRandomDraw>::default().coin_select(
            required,
            optional,
            FeeRate::from_sat_per_vb_unchecked(10),
            Amount::from_sat(500_000),
            &drain_script,
            &mut thread_rng(),
        );

        assert_matches!(
            selection,
            Err(InsufficientFunds {
                available,
                ..
            }) if available.to_sat() == 300_010
        );
    }

    #[test]
    fn test_bnb_sum_of_effective_value_negative() {
        let utxos = get_test_utxos();
        let drain_script = ScriptBuf::default();

        let selection = BranchAndBoundCoinSelection::<SingleRandomDraw>::default().coin_select(
            utxos,
            vec![],
            FeeRate::from_sat_per_vb_unchecked(10_000),
            Amount::from_sat(500_000),
            &drain_script,
            &mut thread_rng(),
        );

        assert_matches!(
            selection,
            Err(InsufficientFunds {
                available,
                ..
            }) if available.to_sat() == 300_010
        );
    }

    #[test]
    fn test_bnb_fallback_algorithm() {
        // utxo value
        // 120k + 80k + 300k
        let optional_utxos = get_oldest_first_test_utxos();
        let feerate = FeeRate::BROADCAST_MIN;
        let target_amount = Amount::from_sat(190_000);
        let drain_script = ScriptBuf::new();
        // bnb won't find exact match and should select oldest first
        let bnb_with_oldest_first =
            BranchAndBoundCoinSelection::new(8 + 1 + 22, OldestFirstCoinSelection);
        let res = bnb_with_oldest_first
            .coin_select(
                vec![],
                optional_utxos,
                feerate,
                target_amount,
                &drain_script,
                &mut thread_rng(),
            )
            .unwrap();
        assert_eq!(res.selected_amount(), Amount::from_sat(200_000));
    }

    #[test]
    fn test_deterministic_coin_selection_picks_same_utxos() {
        enum CoinSelectionAlgo {
            BranchAndBound,
            OldestFirst,
            LargestFirst,
        }

        struct TestCase<'a> {
            name: &'a str,
            coin_selection_algo: CoinSelectionAlgo,
            exp_vouts: &'a [u32],
        }

        let test_cases = [
            TestCase {
                name: "branch and bound",
                coin_selection_algo: CoinSelectionAlgo::BranchAndBound,
                // note: we expect these to be sorted largest first, which indicates
                // BnB succeeded with no fallback
                exp_vouts: &[29, 28, 27],
            },
            TestCase {
                name: "oldest first",
                coin_selection_algo: CoinSelectionAlgo::OldestFirst,
                exp_vouts: &[0, 1, 2],
            },
            TestCase {
                name: "largest first",
                coin_selection_algo: CoinSelectionAlgo::LargestFirst,
                exp_vouts: &[29, 28, 27],
            },
        ];

        let optional = generate_same_value_utxos(Amount::from_sat(100_000), 30);
        let fee_rate = FeeRate::from_sat_per_vb_unchecked(1);
        let target_amount = calc_target_amount(&optional[0..3], fee_rate);
        assert_eq!(target_amount, Amount::from_sat(299_796));
        let drain_script = ScriptBuf::default();

        for tc in test_cases {
            let optional = optional.clone();

            let result = match tc.coin_selection_algo {
                CoinSelectionAlgo::BranchAndBound => {
                    BranchAndBoundCoinSelection::<SingleRandomDraw>::default().coin_select(
                        vec![],
                        optional,
                        fee_rate,
                        target_amount,
                        &drain_script,
                        &mut thread_rng(),
                    )
                }
                CoinSelectionAlgo::OldestFirst => OldestFirstCoinSelection.coin_select(
                    vec![],
                    optional,
                    fee_rate,
                    target_amount,
                    &drain_script,
                    &mut thread_rng(),
                ),
                CoinSelectionAlgo::LargestFirst => LargestFirstCoinSelection.coin_select(
                    vec![],
                    optional,
                    fee_rate,
                    target_amount,
                    &drain_script,
                    &mut thread_rng(),
                ),
            };

            assert!(result.is_ok(), "coin_select failed {}", tc.name);
            let result = result.unwrap();
            assert!(matches!(result.excess, Excess::NoChange { .. },));
            assert_eq!(
                result.selected.len(),
                3,
                "wrong selected len for {}",
                tc.name
            );
            assert_eq!(
                result.selected_amount(),
                Amount::from_sat(300_000),
                "wrong selected amount for {}",
                tc.name
            );
            assert_eq!(
                result.fee_amount,
                Amount::from_sat(204),
                "wrong fee amount for {}",
                tc.name
            );
            let vouts = result
                .selected
                .iter()
                .map(|utxo| utxo.outpoint().vout)
                .collect::<Vec<u32>>();
            assert_eq!(vouts, tc.exp_vouts, "wrong selected vouts for {}", tc.name);
        }
    }
}
