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
//! # use bdk::wallet::{self, coin_selection::*};
//! # use bdk::*;
//! # use bdk::wallet::coin_selection::decide_change;
//! # const TXIN_BASE_WEIGHT: usize = (32 + 4 + 4) * 4;
//! #[derive(Debug)]
//! struct AlwaysSpendEverything;
//!
//! impl CoinSelectionAlgorithm for AlwaysSpendEverything {
//!     fn coin_select(
//!         &self,
//!         required_utxos: Vec<WeightedUtxo>,
//!         optional_utxos: Vec<WeightedUtxo>,
//!         fee_rate: FeeRate,
//!         target_amount: u64,
//!         drain_script: &Script,
//!     ) -> Result<CoinSelectionResult, bdk::Error> {
//!         let mut selected_amount = 0;
//!         let mut additional_weight = 0;
//!         let all_utxos_selected = required_utxos
//!             .into_iter()
//!             .chain(optional_utxos)
//!             .scan(
//!                 (&mut selected_amount, &mut additional_weight),
//!                 |(selected_amount, additional_weight), weighted_utxo| {
//!                     **selected_amount += weighted_utxo.utxo.txout().value;
//!                     **additional_weight += TXIN_BASE_WEIGHT + weighted_utxo.satisfaction_weight;
//!                     Some(weighted_utxo.utxo)
//!                 },
//!             )
//!             .collect::<Vec<_>>();
//!         let additional_fees = fee_rate.fee_wu(additional_weight);
//!         let amount_needed_with_fees = additional_fees + target_amount;
//!         if selected_amount < amount_needed_with_fees {
//!             return Err(bdk::Error::InsufficientFunds {
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
//! let to_address = Address::from_str("2N4eQYCbKUHCCTUjBJeHcJp9ok6J2GZsTDt").unwrap();
//! let (psbt, details) = {
//!     let mut builder = wallet.build_tx().coin_selection(AlwaysSpendEverything);
//!     builder.add_recipient(to_address.script_pubkey(), 50_000);
//!     builder.finish()?
//! };
//!
//! // inspect, sign, broadcast, ...
//!
//! # Ok::<(), bdk::Error>(())
//! ```

use crate::types::FeeRate;
use crate::wallet::utils::IsDust;
use crate::WeightedUtxo;
use crate::{error::Error, Utxo};

use alloc::vec::Vec;
use bitcoin::consensus::encode::serialize;
use bitcoin::Script;

use core::convert::TryInto;
use rand::seq::SliceRandom;

/// Default coin selection algorithm used by [`TxBuilder`](super::tx_builder::TxBuilder) if not
/// overridden
pub type DefaultCoinSelectionAlgorithm = BranchAndBoundCoinSelection;

// Base weight of a Txin, not counting the weight needed for satisfying it.
// prev_txid (32 bytes) + prev_vout (4 bytes) + sequence (4 bytes)
pub(crate) const TXIN_BASE_WEIGHT: usize = (32 + 4 + 4) * 4;

#[derive(Debug)]
/// Remaining amount after performing coin selection
pub enum Excess {
    /// It's not possible to create spendable output from excess using the current drain output
    NoChange {
        /// Threshold to consider amount as dust for this particular change script_pubkey
        dust_threshold: u64,
        /// Exceeding amount of current selection over outgoing value and fee costs
        remaining_amount: u64,
        /// The calculated fee for the drain TxOut with the selected script_pubkey
        change_fee: u64,
    },
    /// It's possible to create spendable output from excess using the current drain output
    Change {
        /// Effective amount available to create change after deducting the change output fee
        amount: u64,
        /// The deducted change output fee
        fee: u64,
    },
}

/// Result of a successful coin selection
#[derive(Debug)]
pub struct CoinSelectionResult {
    /// List of outputs selected for use as inputs
    pub selected: Vec<Utxo>,
    /// Total fee amount for the selected utxos in satoshis
    pub fee_amount: u64,
    /// Remaining amount after deducing fees and outgoing outputs
    pub excess: Excess,
}

impl CoinSelectionResult {
    /// The total value of the inputs selected.
    pub fn selected_amount(&self) -> u64 {
        self.selected.iter().map(|u| u.txout().value).sum()
    }

    /// The total value of the inputs selected from the local wallet.
    pub fn local_selected_amount(&self) -> u64 {
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
    /// - `database`: a reference to the wallet's database that can be used to lookup additional
    ///               details for a specific UTXO
    /// - `required_utxos`: the utxos that must be spent regardless of `target_amount` with their
    ///                     weight cost
    /// - `optional_utxos`: the remaining available utxos to satisfy `target_amount` with their
    ///                     weight cost
    /// - `fee_rate`: fee rate to use
    /// - `target_amount`: the outgoing amount in satoshis and the fees already
    ///                    accumulated from added outputs and transaction’s header.
    /// - `drain_script`: the script to use in case of change
    #[allow(clippy::too_many_arguments)]
    fn coin_select(
        &self,
        required_utxos: Vec<WeightedUtxo>,
        optional_utxos: Vec<WeightedUtxo>,
        fee_rate: FeeRate,
        target_amount: u64,
        drain_script: &Script,
    ) -> Result<CoinSelectionResult, Error>;
}

/// Simple and dumb coin selection
///
/// This coin selection algorithm sorts the available UTXOs by value and then picks them starting
/// from the largest ones until the required amount is reached.
#[derive(Debug, Default, Clone, Copy)]
pub struct LargestFirstCoinSelection;

impl CoinSelectionAlgorithm for LargestFirstCoinSelection {
    fn coin_select(
        &self,
        required_utxos: Vec<WeightedUtxo>,
        mut optional_utxos: Vec<WeightedUtxo>,
        fee_rate: FeeRate,
        target_amount: u64,
        drain_script: &Script,
    ) -> Result<CoinSelectionResult, Error> {
        log::debug!(
            "target_amount = `{}`, fee_rate = `{:?}`",
            target_amount,
            fee_rate
        );

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
    fn coin_select(
        &self,
        required_utxos: Vec<WeightedUtxo>,
        mut optional_utxos: Vec<WeightedUtxo>,
        fee_rate: FeeRate,
        target_amount: u64,
        drain_script: &Script,
    ) -> Result<CoinSelectionResult, Error> {
        // We put the "required UTXOs" first and make sure the optional UTXOs are sorted from
        // oldest to newest according to blocktime
        // For utxo that doesn't exist in DB, they will have lowest priority to be selected
        let utxos = {
            optional_utxos.sort_unstable_by_key(|wu| match &wu.utxo {
                Utxo::Local(local) => Some(local.confirmation_time),
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
pub fn decide_change(remaining_amount: u64, fee_rate: FeeRate, drain_script: &Script) -> Excess {
    // drain_output_len = size(len(script_pubkey)) + len(script_pubkey) + size(output_value)
    let drain_output_len = serialize(drain_script).len() + 8usize;
    let change_fee = fee_rate.fee_vb(drain_output_len);
    let drain_val = remaining_amount.saturating_sub(change_fee);

    if drain_val.is_dust(drain_script) {
        let dust_threshold = drain_script.dust_value().to_sat();
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
    target_amount: u64,
    drain_script: &Script,
) -> Result<CoinSelectionResult, Error> {
    let mut selected_amount = 0;
    let mut fee_amount = 0;
    let selected = utxos
        .scan(
            (&mut selected_amount, &mut fee_amount),
            |(selected_amount, fee_amount), (must_use, weighted_utxo)| {
                if must_use || **selected_amount < target_amount + **fee_amount {
                    **fee_amount +=
                        fee_rate.fee_wu(TXIN_BASE_WEIGHT + weighted_utxo.satisfaction_weight);
                    **selected_amount += weighted_utxo.utxo.txout().value;

                    log::debug!(
                        "Selected {}, updated fee_amount = `{}`",
                        weighted_utxo.utxo.outpoint(),
                        fee_amount
                    );

                    Some(weighted_utxo.utxo)
                } else {
                    None
                }
            },
        )
        .collect::<Vec<_>>();

    let amount_needed_with_fees = target_amount + fee_amount;
    if selected_amount < amount_needed_with_fees {
        return Err(Error::InsufficientFunds {
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
    fee: u64,
    // The effective value of the UTXO, i.e., the utxo value minus the fee for spending it
    effective_value: i64,
}

impl OutputGroup {
    fn new(weighted_utxo: WeightedUtxo, fee_rate: FeeRate) -> Self {
        let fee = fee_rate.fee_wu(TXIN_BASE_WEIGHT + weighted_utxo.satisfaction_weight);
        let effective_value = weighted_utxo.utxo.txout().value as i64 - fee as i64;
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
pub struct BranchAndBoundCoinSelection {
    size_of_change: u64,
}

impl Default for BranchAndBoundCoinSelection {
    fn default() -> Self {
        Self {
            // P2WPKH cost of change -> value (8 bytes) + script len (1 bytes) + script (22 bytes)
            size_of_change: 8 + 1 + 22,
        }
    }
}

impl BranchAndBoundCoinSelection {
    /// Create new instance with target size for change output
    pub fn new(size_of_change: u64) -> Self {
        Self { size_of_change }
    }
}

const BNB_TOTAL_TRIES: usize = 100_000;

impl CoinSelectionAlgorithm for BranchAndBoundCoinSelection {
    fn coin_select(
        &self,
        required_utxos: Vec<WeightedUtxo>,
        optional_utxos: Vec<WeightedUtxo>,
        fee_rate: FeeRate,
        target_amount: u64,
        drain_script: &Script,
    ) -> Result<CoinSelectionResult, Error> {
        // Mapping every (UTXO, usize) to an output group
        let required_utxos: Vec<OutputGroup> = required_utxos
            .into_iter()
            .map(|u| OutputGroup::new(u, fee_rate))
            .collect();

        // Mapping every (UTXO, usize) to an output group, filtering UTXOs with a negative
        // effective value
        let optional_utxos: Vec<OutputGroup> = optional_utxos
            .into_iter()
            .map(|u| OutputGroup::new(u, fee_rate))
            .filter(|u| u.effective_value.is_positive())
            .collect();

        let curr_value = required_utxos
            .iter()
            .fold(0, |acc, x| acc + x.effective_value);

        let curr_available_value = optional_utxos
            .iter()
            .fold(0, |acc, x| acc + x.effective_value);

        let cost_of_change = self.size_of_change as f32 * fee_rate.as_sat_per_vb();

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
        let total_value: Result<u64, _> = (curr_available_value + curr_value).try_into();
        match total_value {
            Ok(v) if v >= target_amount => {}
            _ => {
                // Assume we spend all the UTXOs we can (all the required + all the optional with
                // positive effective value), sum their value and their fee cost.
                let (utxo_fees, utxo_value) = required_utxos
                    .iter()
                    .chain(optional_utxos.iter())
                    .fold((0, 0), |(mut fees, mut value), utxo| {
                        fees += utxo.fee;
                        value += utxo.weighted_utxo.utxo.txout().value;

                        (fees, value)
                    });

                // Add to the target the fee cost of the UTXOs
                return Err(Error::InsufficientFunds {
                    needed: target_amount + utxo_fees,
                    available: utxo_value,
                });
            }
        }

        let target_amount = target_amount
            .try_into()
            .expect("Bitcoin amount to fit into i64");

        if curr_value > target_amount {
            // remaining_amount can't be negative as that would mean the
            // selection wasn't successful
            // target_amount = amount_needed + (fee_amount - vin_fees)
            let remaining_amount = (curr_value - target_amount) as u64;

            let excess = decide_change(remaining_amount, fee_rate, drain_script);

            return Ok(BranchAndBoundCoinSelection::calculate_cs_result(
                vec![],
                required_utxos,
                excess,
            ));
        }

        Ok(self
            .bnb(
                required_utxos.clone(),
                optional_utxos.clone(),
                curr_value,
                curr_available_value,
                target_amount,
                cost_of_change,
                drain_script,
                fee_rate,
            )
            .unwrap_or_else(|_| {
                self.single_random_draw(
                    required_utxos,
                    optional_utxos,
                    curr_value,
                    target_amount,
                    drain_script,
                    fee_rate,
                )
            }))
    }
}

impl BranchAndBoundCoinSelection {
    // TODO: make this more Rust-onic :)
    // (And perhaps refactor with less arguments?)
    #[allow(clippy::too_many_arguments)]
    fn bnb(
        &self,
        required_utxos: Vec<OutputGroup>,
        mut optional_utxos: Vec<OutputGroup>,
        mut curr_value: i64,
        mut curr_available_value: i64,
        target_amount: i64,
        cost_of_change: f32,
        drain_script: &Script,
        fee_rate: FeeRate,
    ) -> Result<CoinSelectionResult, Error> {
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
                || curr_value > target_amount + cost_of_change as i64
            {
                backtrack = true;
            } else if curr_value >= target_amount {
                // Selected value is within range, there's no point in going forward. Start
                // backtracking
                backtrack = true;

                // If we found a solution better than the previous one, or if there wasn't previous
                // solution, update the best solution
                if best_selection_value.is_none() || curr_value < best_selection_value.unwrap() {
                    best_selection = current_selection.clone();
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
                        return Err(Error::BnBNoExactMatch);
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
            return Err(Error::BnBTotalTriesExceeded);
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
        let remaining_amount = (selected_amount - target_amount) as u64;

        let excess = decide_change(remaining_amount, fee_rate, drain_script);

        Ok(BranchAndBoundCoinSelection::calculate_cs_result(
            selected_utxos,
            required_utxos,
            excess,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn single_random_draw(
        &self,
        required_utxos: Vec<OutputGroup>,
        mut optional_utxos: Vec<OutputGroup>,
        curr_value: i64,
        target_amount: i64,
        drain_script: &Script,
        fee_rate: FeeRate,
    ) -> CoinSelectionResult {
        optional_utxos.shuffle(&mut rand::thread_rng());
        let selected_utxos = optional_utxos.into_iter().fold(
            (curr_value, vec![]),
            |(mut amount, mut utxos), utxo| {
                if amount >= target_amount {
                    (amount, utxos)
                } else {
                    amount += utxo.effective_value;
                    utxos.push(utxo);
                    (amount, utxos)
                }
            },
        );

        // remaining_amount can't be negative as that would mean the
        // selection wasn't successful
        // target_amount = amount_needed + (fee_amount - vin_fees)
        let remaining_amount = (selected_utxos.0 - target_amount) as u64;

        let excess = decide_change(remaining_amount, fee_rate, drain_script);

        BranchAndBoundCoinSelection::calculate_cs_result(selected_utxos.1, required_utxos, excess)
    }

    fn calculate_cs_result(
        mut selected_utxos: Vec<OutputGroup>,
        mut required_utxos: Vec<OutputGroup>,
        excess: Excess,
    ) -> CoinSelectionResult {
        selected_utxos.append(&mut required_utxos);
        let fee_amount = selected_utxos.iter().map(|u| u.fee).sum::<u64>();
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
}

#[cfg(test)]
mod test {
    use assert_matches::assert_matches;
    use core::str::FromStr;

    use bdk_chain::ConfirmationTime;
    use bitcoin::{OutPoint, Script, TxOut};

    use super::*;
    use crate::types::*;
    use crate::wallet::Vbytes;

    use rand::rngs::StdRng;
    use rand::seq::SliceRandom;
    use rand::{Rng, RngCore, SeedableRng};

    // n. of items on witness (1WU) + signature len (1WU) + signature and sighash (72WU)
    // + pubkey len (1WU) + pubkey (33WU) + script sig len (1 byte, 4WU)
    const P2WPKH_SATISFACTION_SIZE: usize = 1 + 1 + 72 + 1 + 33 + 4;

    const FEE_AMOUNT: u64 = 50;

    fn utxo(value: u64, index: u32, confirmation_time: ConfirmationTime) -> WeightedUtxo {
        assert!(index < 10);
        let outpoint = OutPoint::from_str(&format!(
            "000000000000000000000000000000000000000000000000000000000000000{}:0",
            index
        ))
        .unwrap();
        WeightedUtxo {
            satisfaction_weight: P2WPKH_SATISFACTION_SIZE,
            utxo: Utxo::Local(LocalUtxo {
                outpoint,
                txout: TxOut {
                    value,
                    script_pubkey: Script::new(),
                },
                keychain: KeychainKind::External,
                is_spent: false,
                derivation_index: 42,
                confirmation_time,
            }),
        }
    }

    fn get_test_utxos() -> Vec<WeightedUtxo> {
        vec![
            utxo(100_000, 0, ConfirmationTime::Unconfirmed { last_seen: 0 }),
            utxo(
                FEE_AMOUNT - 40,
                1,
                ConfirmationTime::Unconfirmed { last_seen: 0 },
            ),
            utxo(200_000, 2, ConfirmationTime::Unconfirmed { last_seen: 0 }),
        ]
    }

    fn get_oldest_first_test_utxos() -> Vec<WeightedUtxo> {
        // ensure utxos are from different tx
        let utxo1 = utxo(
            120_000,
            1,
            ConfirmationTime::Confirmed {
                height: 1,
                time: 1231006505,
            },
        );
        let utxo2 = utxo(
            80_000,
            2,
            ConfirmationTime::Confirmed {
                height: 2,
                time: 1231006505,
            },
        );
        let utxo3 = utxo(
            300_000,
            3,
            ConfirmationTime::Confirmed {
                height: 3,
                time: 1231006505,
            },
        );
        vec![utxo1, utxo2, utxo3]
    }

    fn generate_random_utxos(rng: &mut StdRng, utxos_number: usize) -> Vec<WeightedUtxo> {
        let mut res = Vec::new();
        for _ in 0..utxos_number {
            res.push(WeightedUtxo {
                satisfaction_weight: P2WPKH_SATISFACTION_SIZE,
                utxo: Utxo::Local(LocalUtxo {
                    outpoint: OutPoint::from_str(
                        "ebd9813ecebc57ff8f30797de7c205e3c7498ca950ea4341ee51a685ff2fa30a:0",
                    )
                    .unwrap(),
                    txout: TxOut {
                        value: rng.gen_range(0..200000000),
                        script_pubkey: Script::new(),
                    },
                    keychain: KeychainKind::External,
                    is_spent: false,
                    derivation_index: rng.next_u32(),
                    confirmation_time: if rng.gen_bool(0.5) {
                        ConfirmationTime::Confirmed {
                            height: rng.next_u32(),
                            time: rng.next_u64(),
                        }
                    } else {
                        ConfirmationTime::Unconfirmed { last_seen: 0 }
                    },
                }),
            });
        }
        res
    }

    fn generate_same_value_utxos(utxos_value: u64, utxos_number: usize) -> Vec<WeightedUtxo> {
        let utxo = WeightedUtxo {
            satisfaction_weight: P2WPKH_SATISFACTION_SIZE,
            utxo: Utxo::Local(LocalUtxo {
                outpoint: OutPoint::from_str(
                    "ebd9813ecebc57ff8f30797de7c205e3c7498ca950ea4341ee51a685ff2fa30a:0",
                )
                .unwrap(),
                txout: TxOut {
                    value: utxos_value,
                    script_pubkey: Script::new(),
                },
                keychain: KeychainKind::External,
                is_spent: false,
                derivation_index: 42,
                confirmation_time: ConfirmationTime::Unconfirmed { last_seen: 0 },
            }),
        };
        vec![utxo; utxos_number]
    }

    fn sum_random_utxos(mut rng: &mut StdRng, utxos: &mut Vec<WeightedUtxo>) -> u64 {
        let utxos_picked_len = rng.gen_range(2..utxos.len() / 2);
        utxos.shuffle(&mut rng);
        utxos[..utxos_picked_len]
            .iter()
            .map(|u| u.utxo.txout().value)
            .sum()
    }

    #[test]
    fn test_largest_first_coin_selection_success() {
        let utxos = get_test_utxos();
        let drain_script = Script::default();
        let target_amount = 250_000 + FEE_AMOUNT;

        let result = LargestFirstCoinSelection::default()
            .coin_select(
                utxos,
                vec![],
                FeeRate::from_sat_per_vb(1.0),
                target_amount,
                &drain_script,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), 300_010);
        assert_eq!(result.fee_amount, 204)
    }

    #[test]
    fn test_largest_first_coin_selection_use_all() {
        let utxos = get_test_utxos();
        let drain_script = Script::default();
        let target_amount = 20_000 + FEE_AMOUNT;

        let result = LargestFirstCoinSelection::default()
            .coin_select(
                utxos,
                vec![],
                FeeRate::from_sat_per_vb(1.0),
                target_amount,
                &drain_script,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), 300_010);
        assert_eq!(result.fee_amount, 204);
    }

    #[test]
    fn test_largest_first_coin_selection_use_only_necessary() {
        let utxos = get_test_utxos();
        let drain_script = Script::default();
        let target_amount = 20_000 + FEE_AMOUNT;

        let result = LargestFirstCoinSelection::default()
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                target_amount,
                &drain_script,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected_amount(), 200_000);
        assert_eq!(result.fee_amount, 68);
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_largest_first_coin_selection_insufficient_funds() {
        let utxos = get_test_utxos();
        let drain_script = Script::default();
        let target_amount = 500_000 + FEE_AMOUNT;

        LargestFirstCoinSelection::default()
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                target_amount,
                &drain_script,
            )
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_largest_first_coin_selection_insufficient_funds_high_fees() {
        let utxos = get_test_utxos();
        let drain_script = Script::default();
        let target_amount = 250_000 + FEE_AMOUNT;

        LargestFirstCoinSelection::default()
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1000.0),
                target_amount,
                &drain_script,
            )
            .unwrap();
    }

    #[test]
    fn test_oldest_first_coin_selection_success() {
        let utxos = get_oldest_first_test_utxos();
        let drain_script = Script::default();
        let target_amount = 180_000 + FEE_AMOUNT;

        let result = OldestFirstCoinSelection::default()
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                target_amount,
                &drain_script,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected_amount(), 200_000);
        assert_eq!(result.fee_amount, 136)
    }

    #[test]
    fn test_oldest_first_coin_selection_use_all() {
        let utxos = get_oldest_first_test_utxos();
        let drain_script = Script::default();
        let target_amount = 20_000 + FEE_AMOUNT;

        let result = OldestFirstCoinSelection::default()
            .coin_select(
                utxos,
                vec![],
                FeeRate::from_sat_per_vb(1.0),
                target_amount,
                &drain_script,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), 500_000);
        assert_eq!(result.fee_amount, 204);
    }

    #[test]
    fn test_oldest_first_coin_selection_use_only_necessary() {
        let utxos = get_oldest_first_test_utxos();
        let drain_script = Script::default();
        let target_amount = 20_000 + FEE_AMOUNT;

        let result = OldestFirstCoinSelection::default()
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                target_amount,
                &drain_script,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected_amount(), 120_000);
        assert_eq!(result.fee_amount, 68);
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_oldest_first_coin_selection_insufficient_funds() {
        let utxos = get_oldest_first_test_utxos();
        let drain_script = Script::default();
        let target_amount = 600_000 + FEE_AMOUNT;

        OldestFirstCoinSelection::default()
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                target_amount,
                &drain_script,
            )
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_oldest_first_coin_selection_insufficient_funds_high_fees() {
        let utxos = get_oldest_first_test_utxos();

        let target_amount: u64 = utxos.iter().map(|wu| wu.utxo.txout().value).sum::<u64>() - 50;
        let drain_script = Script::default();

        OldestFirstCoinSelection::default()
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1000.0),
                target_amount,
                &drain_script,
            )
            .unwrap();
    }

    #[test]
    fn test_bnb_coin_selection_success() {
        // In this case bnb won't find a suitable match and single random draw will
        // select three outputs
        let utxos = generate_same_value_utxos(100_000, 20);

        let drain_script = Script::default();

        let target_amount = 250_000 + FEE_AMOUNT;

        let result = BranchAndBoundCoinSelection::default()
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                target_amount,
                &drain_script,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), 300_000);
        assert_eq!(result.fee_amount, 204);
    }

    #[test]
    fn test_bnb_coin_selection_required_are_enough() {
        let utxos = get_test_utxos();
        let drain_script = Script::default();
        let target_amount = 20_000 + FEE_AMOUNT;

        let result = BranchAndBoundCoinSelection::default()
            .coin_select(
                utxos.clone(),
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                target_amount,
                &drain_script,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), 300_010);
        assert_eq!(result.fee_amount, 204);
    }

    #[test]
    fn test_bnb_coin_selection_optional_are_enough() {
        let utxos = get_test_utxos();
        let drain_script = Script::default();
        let target_amount = 299756 + FEE_AMOUNT;

        let result = BranchAndBoundCoinSelection::default()
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                target_amount,
                &drain_script,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected_amount(), 300000);
        assert_eq!(result.fee_amount, 136);
    }

    #[test]
    #[ignore]
    fn test_bnb_coin_selection_required_not_enough() {
        let utxos = get_test_utxos();

        let required = vec![utxos[0].clone()];
        let mut optional = utxos[1..].to_vec();
        optional.push(utxo(
            500_000,
            3,
            ConfirmationTime::Unconfirmed { last_seen: 0 },
        ));

        // Defensive assertions, for sanity and in case someone changes the test utxos vector.
        let amount: u64 = required.iter().map(|u| u.utxo.txout().value).sum();
        assert_eq!(amount, 100_000);
        let amount: u64 = optional.iter().map(|u| u.utxo.txout().value).sum();
        assert!(amount > 150_000);
        let drain_script = Script::default();

        let target_amount = 150_000 + FEE_AMOUNT;

        let result = BranchAndBoundCoinSelection::default()
            .coin_select(
                required,
                optional,
                FeeRate::from_sat_per_vb(1.0),
                target_amount,
                &drain_script,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected_amount(), 300_000);
        assert_eq!(result.fee_amount, 136);
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_bnb_coin_selection_insufficient_funds() {
        let utxos = get_test_utxos();
        let drain_script = Script::default();
        let target_amount = 500_000 + FEE_AMOUNT;

        BranchAndBoundCoinSelection::default()
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                target_amount,
                &drain_script,
            )
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_bnb_coin_selection_insufficient_funds_high_fees() {
        let utxos = get_test_utxos();
        let drain_script = Script::default();
        let target_amount = 250_000 + FEE_AMOUNT;

        BranchAndBoundCoinSelection::default()
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1000.0),
                target_amount,
                &drain_script,
            )
            .unwrap();
    }

    #[test]
    fn test_bnb_coin_selection_check_fee_rate() {
        let utxos = get_test_utxos();
        let drain_script = Script::default();
        let target_amount = 99932; // first utxo's effective value

        let result = BranchAndBoundCoinSelection::new(0)
            .coin_select(
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                target_amount,
                &drain_script,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected_amount(), 100_000);
        let input_size = (TXIN_BASE_WEIGHT + P2WPKH_SATISFACTION_SIZE).vbytes();
        // the final fee rate should be exactly the same as the fee rate given
        assert!((1.0 - (result.fee_amount as f32 / input_size as f32)).abs() < f32::EPSILON);
    }

    #[test]
    fn test_bnb_coin_selection_exact_match() {
        let seed = [0; 32];
        let mut rng: StdRng = SeedableRng::from_seed(seed);

        for _i in 0..200 {
            let mut optional_utxos = generate_random_utxos(&mut rng, 16);
            let target_amount = sum_random_utxos(&mut rng, &mut optional_utxos);
            let drain_script = Script::default();
            let result = BranchAndBoundCoinSelection::new(0)
                .coin_select(
                    vec![],
                    optional_utxos,
                    FeeRate::from_sat_per_vb(0.0),
                    target_amount,
                    &drain_script,
                )
                .unwrap();
            assert_eq!(result.selected_amount(), target_amount);
        }
    }

    #[test]
    #[should_panic(expected = "BnBNoExactMatch")]
    fn test_bnb_function_no_exact_match() {
        let fee_rate = FeeRate::from_sat_per_vb(10.0);
        let utxos: Vec<OutputGroup> = get_test_utxos()
            .into_iter()
            .map(|u| OutputGroup::new(u, fee_rate))
            .collect();

        let curr_available_value = utxos.iter().fold(0, |acc, x| acc + x.effective_value);

        let size_of_change = 31;
        let cost_of_change = size_of_change as f32 * fee_rate.as_sat_per_vb();

        let drain_script = Script::default();
        let target_amount = 20_000 + FEE_AMOUNT;
        BranchAndBoundCoinSelection::new(size_of_change)
            .bnb(
                vec![],
                utxos,
                0,
                curr_available_value,
                target_amount as i64,
                cost_of_change,
                &drain_script,
                fee_rate,
            )
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "BnBTotalTriesExceeded")]
    fn test_bnb_function_tries_exceeded() {
        let fee_rate = FeeRate::from_sat_per_vb(10.0);
        let utxos: Vec<OutputGroup> = generate_same_value_utxos(100_000, 100_000)
            .into_iter()
            .map(|u| OutputGroup::new(u, fee_rate))
            .collect();

        let curr_available_value = utxos.iter().fold(0, |acc, x| acc + x.effective_value);

        let size_of_change = 31;
        let cost_of_change = size_of_change as f32 * fee_rate.as_sat_per_vb();
        let target_amount = 20_000 + FEE_AMOUNT;

        let drain_script = Script::default();

        BranchAndBoundCoinSelection::new(size_of_change)
            .bnb(
                vec![],
                utxos,
                0,
                curr_available_value,
                target_amount as i64,
                cost_of_change,
                &drain_script,
                fee_rate,
            )
            .unwrap();
    }

    // The match won't be exact but still in the range
    #[test]
    fn test_bnb_function_almost_exact_match_with_fees() {
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let size_of_change = 31;
        let cost_of_change = size_of_change as f32 * fee_rate.as_sat_per_vb();

        let utxos: Vec<_> = generate_same_value_utxos(50_000, 10)
            .into_iter()
            .map(|u| OutputGroup::new(u, fee_rate))
            .collect();

        let curr_value = 0;

        let curr_available_value = utxos.iter().fold(0, |acc, x| acc + x.effective_value);

        // 2*(value of 1 utxo)  - 2*(1 utxo fees with 1.0sat/vbyte fee rate) -
        // cost_of_change + 5.
        let target_amount = 2 * 50_000 - 2 * 67 - cost_of_change.ceil() as i64 + 5;

        let drain_script = Script::default();

        let result = BranchAndBoundCoinSelection::new(size_of_change)
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
        assert_eq!(result.selected_amount(), 100_000);
        assert_eq!(result.fee_amount, 136);
    }

    // TODO: bnb() function should be optimized, and this test should be done with more utxos
    #[test]
    fn test_bnb_function_exact_match_more_utxos() {
        let seed = [0; 32];
        let mut rng: StdRng = SeedableRng::from_seed(seed);
        let fee_rate = FeeRate::from_sat_per_vb(0.0);

        for _ in 0..200 {
            let optional_utxos: Vec<_> = generate_random_utxos(&mut rng, 40)
                .into_iter()
                .map(|u| OutputGroup::new(u, fee_rate))
                .collect();

            let curr_value = 0;

            let curr_available_value = optional_utxos
                .iter()
                .fold(0, |acc, x| acc + x.effective_value);

            let target_amount =
                optional_utxos[3].effective_value + optional_utxos[23].effective_value;

            let drain_script = Script::default();

            let result = BranchAndBoundCoinSelection::new(0)
                .bnb(
                    vec![],
                    optional_utxos,
                    curr_value,
                    curr_available_value,
                    target_amount,
                    0.0,
                    &drain_script,
                    fee_rate,
                )
                .unwrap();
            assert_eq!(result.selected_amount(), target_amount as u64);
        }
    }

    #[test]
    fn test_single_random_draw_function_success() {
        let seed = [0; 32];
        let mut rng: StdRng = SeedableRng::from_seed(seed);
        let mut utxos = generate_random_utxos(&mut rng, 300);
        let target_amount = sum_random_utxos(&mut rng, &mut utxos) + FEE_AMOUNT;

        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let utxos: Vec<OutputGroup> = utxos
            .into_iter()
            .map(|u| OutputGroup::new(u, fee_rate))
            .collect();

        let drain_script = Script::default();

        let result = BranchAndBoundCoinSelection::default().single_random_draw(
            vec![],
            utxos,
            0,
            target_amount as i64,
            &drain_script,
            fee_rate,
        );

        assert!(result.selected_amount() > target_amount);
        assert_eq!(result.fee_amount, (result.selected.len() * 68) as u64);
    }

    #[test]
    fn test_bnb_exclude_negative_effective_value() {
        let utxos = get_test_utxos();
        let drain_script = Script::default();

        let selection = BranchAndBoundCoinSelection::default().coin_select(
            vec![],
            utxos,
            FeeRate::from_sat_per_vb(10.0),
            500_000,
            &drain_script,
        );

        assert_matches!(
            selection,
            Err(Error::InsufficientFunds {
                available: 300_000,
                ..
            })
        );
    }

    #[test]
    fn test_bnb_include_negative_effective_value_when_required() {
        let utxos = get_test_utxos();
        let drain_script = Script::default();

        let (required, optional) = utxos
            .into_iter()
            .partition(|u| matches!(u, WeightedUtxo { utxo, .. } if utxo.txout().value < 1000));

        let selection = BranchAndBoundCoinSelection::default().coin_select(
            required,
            optional,
            FeeRate::from_sat_per_vb(10.0),
            500_000,
            &drain_script,
        );

        assert_matches!(
            selection,
            Err(Error::InsufficientFunds {
                available: 300_010,
                ..
            })
        );
    }

    #[test]
    fn test_bnb_sum_of_effective_value_negative() {
        let utxos = get_test_utxos();
        let drain_script = Script::default();

        let selection = BranchAndBoundCoinSelection::default().coin_select(
            utxos,
            vec![],
            FeeRate::from_sat_per_vb(10_000.0),
            500_000,
            &drain_script,
        );

        assert_matches!(
            selection,
            Err(Error::InsufficientFunds {
                available: 300_010,
                ..
            })
        );
    }
}
