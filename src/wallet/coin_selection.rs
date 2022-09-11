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
//! # use bdk::database::Database;
//! # use bdk::*;
//! # const TXIN_BASE_WEIGHT: usize = (32 + 4 + 4) * 4;
//! #[derive(Debug)]
//! struct AlwaysSpendEverything;
//!
//! impl<D: Database> CoinSelectionAlgorithm<D> for AlwaysSpendEverything {
//!     fn coin_select(
//!         &self,
//!         database: &D,
//!         optional_utxos: Vec<OutputGroup>,
//!         fee_rate: FeeRate,
//!         target_amount: u64,
//!         available_value: i64,
//!     ) -> Result<Vec<OutputGroup>, bdk::Error> {
//!         Ok(optional_utxos)
//!     }
//! }
//!
//! # let wallet = doctest_wallet!();
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

use crate::database::Database;
use crate::types::{FeeRate, WeightedScriptPubkey, WeightedUtxo};
use crate::wallet::utils::IsDust;
use crate::{error::Error, Utxo};

use bitcoin::consensus::encode::serialize;
use bitcoin::Script;

use rand::seq::SliceRandom;
#[cfg(not(test))]
use rand::thread_rng;
#[cfg(test)]
use rand::{rngs::StdRng, SeedableRng};
use std::collections::HashMap;
use std::convert::TryInto;

/// Default coin selection algorithm used by [`TxBuilder`](super::tx_builder::TxBuilder) if not
/// overridden
#[cfg(not(test))]
pub type DefaultCoinSelectionAlgorithm = BranchAndBoundCoinSelection;
#[cfg(test)]
pub type DefaultCoinSelectionAlgorithm = LargestFirstCoinSelection; // make the tests more predictable

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

/// Perform the coin selection
///
/// - `algorithm`: the algorithm to use to select the UTXOs for the transaction
/// - `database`: database reference to use by those algorithms that require extra information
///               about UTXOs
/// - `required_utxos`: the utxos that must be spent regardless of `target_amount` with their
///                     weight cost
/// - `optional_utxos`: the remaining available utxos to satisfy `target_amount` with their
///                     weight cost
/// - `fee_rate`: fee rate to use
/// - `target_amount`: the outgoing amount in satoshis and the fees already
///                    accumulated from added outputs and transaction’s header.
/// - `weighted_drain_script`: the drain script used to create the change output associated with
///                            the max satisfaction weight needed to spend it
pub fn process_and_select_coins<D: Database, Cs: CoinSelectionAlgorithm<D>>(
    algorithm: Cs,
    database: &D,
    required_utxos: Vec<WeightedUtxo>,
    optional_utxos: Vec<WeightedUtxo>,
    fee_rate: FeeRate,
    target_amount: u64,
    weighted_drain_script: &WeightedScriptPubkey,
) -> Result<CoinSelectionResult, Error> {
    // ####################################################################
    // ######################### PREPROCESSING ############################
    // ####################################################################

    // Mapping every (UTXO, usize) to an output group
    let mut required_output_groups: Vec<OutputGroup> = required_utxos
        .into_iter()
        .map(|u| OutputGroup::new(u, fee_rate))
        .collect();

    // Mapping every (UTXO, usize) to an output group.
    let optional_output_groups: Vec<OutputGroup> = optional_utxos
        .into_iter()
        .map(|u| OutputGroup::new(u, fee_rate))
        .filter(|u| u.effective_value.is_positive())
        .collect();

    let (required_effective_value, required_fees) = required_output_groups
        .iter()
        .fold((0, 0), |(eff_value, fees), x| {
            (eff_value + x.effective_value, fees + x.fee)
        });

    let (optional_effective_value, optional_fees) = optional_output_groups
        .iter()
        .fold((0, 0), |(eff_value, fees), x| {
            (eff_value + x.effective_value, fees + x.fee)
        });

    // `required_effective_value` and `optional_effective_value` are both the sum of *effective_values* of
    // the UTXOs. For the optional UTXOs (optional_effective_value) we filter out UTXOs with
    // negative effective value, so it will always be positive.
    //
    // Since we are required to spend the required UTXOs (required_effective_value) we have to consider
    // all their effective values, even when negative, which means that required_effective_value could
    // be negative as well.
    //
    // If the sum of required_effective_value and optional_effective_value is negative or lower than our target,
    // we can immediately exit with an error, as it's guaranteed we will never find a solution
    // if we actually run the coin selection algorithm
    let total_effective_value = optional_effective_value + required_effective_value;
    let total_value: Result<u64, _> = total_effective_value.try_into();
    match total_value {
        Ok(v) if v >= target_amount => {}
        _ => {
            // Assume we spend all the UTXOs we can (all the required + all the optional with
            // positive effective value), sum their value and their fee cost.
            let utxo_fees = optional_fees + required_fees;
            let utxo_value = total_effective_value + utxo_fees as i64;

            // Add to the target the fee cost of the UTXOs
            return Err(Error::InsufficientFunds {
                needed: target_amount + utxo_fees,
                available: utxo_value as u64,
            });
        }
    }

    let target_amount_i64 = target_amount
        .try_into()
        .expect("Bitcoin amount to fit into i64");

    // ####################################################################
    // ######################### COIN SELECTION ###########################
    // ####################################################################

    let (output_groups, effective_value, fee_amount) =
        if required_effective_value > target_amount_i64 {
            (
                required_output_groups,
                required_effective_value,
                required_fees,
            )
        } else if total_effective_value == target_amount_i64 {
            let mut selected_output_groups = optional_output_groups;
            selected_output_groups.append(&mut required_output_groups);
            (
                selected_output_groups,
                total_effective_value,
                optional_fees + required_fees,
            )
        } else {
            // from now on, target_amount can only be positive
            let target_amount = (target_amount_i64 - required_effective_value) as u64;
            let mut selected_output_groups = algorithm.coin_select(
                database,
                optional_output_groups,
                fee_rate,
                target_amount,
                optional_effective_value,
            )?;
            let selected_effective_value: i64 = selected_output_groups
                .iter()
                .map(|og| og.effective_value)
                .sum();
            let selected_fees: u64 = selected_output_groups.iter().map(|og| og.fee).sum();
            selected_output_groups.append(&mut required_output_groups);
            (
                selected_output_groups,
                selected_effective_value + required_effective_value,
                selected_fees + required_fees,
            )
        };

    // ####################################################################
    // ############################ GET EXCESS ############################
    // ####################################################################

    // if coin_select finish, it means it found a valid coin selection. The effective value of that
    // coin selection should be greater than target_amount (an u64) so it's safe to apply the
    // conversion from i64 to u64
    let effective_value = effective_value as u64;

    // remaining_amount = selected_amount - (target_amount + selection_fees)
    let remaining_amount = effective_value.saturating_sub(target_amount);

    let excess = decide_change(
        remaining_amount,
        fee_rate,
        &weighted_drain_script.script_pubkey,
    );

    // ####################################################################
    // ############################ GET UTXOS #############################
    // ####################################################################

    let selected = output_groups
        .into_iter()
        .map(|x| x.weighted_utxo.utxo)
        .collect::<Vec<_>>();

    // ####################################################################
    // ##################### BUILD COIN SELECTION RESULT ##################
    // ####################################################################

    Ok(CoinSelectionResult {
        selected,
        fee_amount,
        excess,
    })
}

/// Trait for generalized coin selection algorithms
///
/// This trait can be implemented to make the [`Wallet`](super::Wallet) use a customized coin
/// selection algorithm when it creates transactions.
///
/// For an example see [this module](crate::wallet::coin_selection)'s documentation.
pub trait CoinSelectionAlgorithm<D: Database>: std::fmt::Debug {
    /// Perform the coin selection
    ///
    /// - `database`: a reference to the wallet's database that can be used to lookup additional
    ///               details for a specific UTXO
    /// - `optional_utxos`: the utxos available for selection to satisfy `target_amount` with their
    ///                     weight cost
    /// - `fee_rate`: fee rate to use
    /// - `target_amount`: the outgoing amount in satoshis and the fees already
    ///                    accumulated from added outputs and transaction’s header.
    /// - `available_value`: the total effective value of all the optional utxos
    fn coin_select(
        &self,
        database: &D,
        optional_utxos: Vec<OutputGroup>,
        fee_rate: FeeRate,
        target_amount: u64,
        available_value: i64,
    ) -> Result<Vec<OutputGroup>, Error>;
}

/// Simple and dumb coin selection
///
/// This coin selection algorithm sorts the available UTXOs by value and then picks them starting
/// from the largest ones until the required amount is reached.
#[derive(Debug, Default, Clone, Copy)]
pub struct LargestFirstCoinSelection;

impl<D: Database> CoinSelectionAlgorithm<D> for LargestFirstCoinSelection {
    fn coin_select(
        &self,
        _database: &D,
        mut optional_utxos: Vec<OutputGroup>,
        _fee_rate: FeeRate,
        target_amount: u64,
        _available_value: i64,
    ) -> Result<Vec<OutputGroup>, Error> {
        log::debug!("target_amount = `{}`", target_amount,);

        // We make sure the optional UTXOs are sorted, initially smallest to largest, before being
        // reversed with `.rev()`.
        let utxos = {
            optional_utxos.sort_unstable_by_key(|og| og.effective_value);
            optional_utxos.into_iter().rev()
        };

        Ok(select_sorted_utxos(utxos, target_amount))
    }
}

/// OldestFirstCoinSelection always picks the utxo with the smallest blockheight to add to the selected coins next
///
/// This coin selection algorithm sorts the available UTXOs by blockheight and then picks them starting
/// from the oldest ones until the required amount is reached.
#[derive(Debug, Default, Clone, Copy)]
pub struct OldestFirstCoinSelection;

impl<D: Database> CoinSelectionAlgorithm<D> for OldestFirstCoinSelection {
    fn coin_select(
        &self,
        database: &D,
        mut optional_utxos: Vec<OutputGroup>,
        _fee_rate: FeeRate,
        target_amount: u64,
        _available_value: i64,
    ) -> Result<Vec<OutputGroup>, Error> {
        // query db and create a blockheight lookup table
        let blockheights = optional_utxos
            .iter()
            .map(|og| og.weighted_utxo.utxo.outpoint().txid)
            // fold is used so we can skip db query for txid that already exist in hashmap acc
            .fold(Ok(HashMap::new()), |bh_result_acc, txid| {
                bh_result_acc.and_then(|mut bh_acc| {
                    if bh_acc.contains_key(&txid) {
                        Ok(bh_acc)
                    } else {
                        database.get_tx(&txid, false).map(|details| {
                            bh_acc.insert(
                                txid,
                                details.and_then(|d| d.confirmation_time.map(|ct| ct.height)),
                            );
                            bh_acc
                        })
                    }
                })
            })?;

        // We make sure the optional UTXOs are sorted from oldest to newest according to blocktime
        // For utxo that doesn't exist in DB, they will have lowest priority to be selected
        let utxos = {
            optional_utxos.sort_unstable_by_key(|og| {
                match blockheights.get(&og.weighted_utxo.utxo.outpoint().txid) {
                    Some(Some(blockheight)) => blockheight,
                    _ => &u32::MAX,
                }
            });

            optional_utxos.into_iter()
        };

        Ok(select_sorted_utxos(utxos, target_amount))
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
        let dust_threshold = drain_script.dust_value().as_sat();
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
    utxos: impl Iterator<Item = OutputGroup>,
    target_amount: u64,
) -> Vec<OutputGroup> {
    let mut selected_amount = 0_u64;
    utxos
        .scan(&mut selected_amount, |selected_amount, output_group| {
            if **selected_amount < target_amount {
                // all outputs have a positive effective value, so the conversion it's safe
                **selected_amount += output_group.effective_value as u64;

                log::debug!("Selected {}", output_group.weighted_utxo.utxo.outpoint(),);

                Some(output_group)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
}

#[derive(Debug, Clone)]
/// A [`WeightedUtxo`] with its associated fee and the real value after discounting that fee
/// from the carried value
pub struct OutputGroup {
    /// The weighted utxo
    pub weighted_utxo: WeightedUtxo,
    /// Amount of fees for spending a certain utxo, calculated using a certain FeeRate
    pub fee: u64,
    /// The effective value of the UTXO, i.e., the utxo value minus the fee for spending it
    pub effective_value: i64,
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
#[derive(Debug)]
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

impl<D: Database> CoinSelectionAlgorithm<D> for BranchAndBoundCoinSelection {
    fn coin_select(
        &self,
        _database: &D,
        optional_utxos: Vec<OutputGroup>,
        fee_rate: FeeRate,
        target_amount: u64,
        available_value: i64,
    ) -> Result<Vec<OutputGroup>, Error> {
        // convert target amount on i64 to use in comparisons and assignments
        let target_amount = target_amount
            .try_into()
            .expect("Bitcoin amount to fit into i64");

        let cost_of_change = self.size_of_change as f32 * fee_rate.as_sat_per_vb();

        Ok(self
            .bnb(
                optional_utxos.clone(),
                target_amount,
                cost_of_change,
                available_value,
            )
            .unwrap_or_else(|_| self.single_random_draw(optional_utxos, target_amount)))
    }
}

impl BranchAndBoundCoinSelection {
    fn bnb(
        &self,
        mut optional_utxos: Vec<OutputGroup>,
        target_amount: i64,
        cost_of_change: f32,
        mut available_value: i64,
    ) -> Result<Vec<OutputGroup>, Error> {
        // the value of the current selection
        let mut selected_value = 0;

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
            // Cannot possibly reach target with the amount remaining in the available_value,
            // or the selected value is out of range.
            // Go back and try other branch
            if selected_value + available_value < target_amount
                || selected_value > target_amount + cost_of_change as i64
            {
                backtrack = true;
            } else if selected_value >= target_amount {
                // Selected value is within range, there's no point in going forward. Start
                // backtracking
                backtrack = true;

                // If we found a solution better than the previous one, or if there wasn't previous
                // solution, update the best solution
                if best_selection_value.is_none() || selected_value < best_selection_value.unwrap()
                {
                    best_selection = current_selection.clone();
                    best_selection_value = Some(selected_value);
                }

                // If we found a perfect match, break here
                if selected_value == target_amount {
                    break;
                }
            }

            // Backtracking, moving backwards
            if backtrack {
                // Walk backwards to find the last included UTXO that still needs to have its omission branch traversed.
                while let Some(false) = current_selection.last() {
                    current_selection.pop();
                    available_value += optional_utxos[current_selection.len()].effective_value;
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
                selected_value -= utxo.effective_value;
            } else {
                // Moving forwards, continuing down this branch
                let utxo = &optional_utxos[current_selection.len()];

                // Remove this utxo from the available_value utxo amount
                available_value -= utxo.effective_value;

                // Inclusion branch first (Largest First Exploration)
                current_selection.push(true);
                selected_value += utxo.effective_value;
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

        Ok(selected_utxos)
    }

    fn single_random_draw(
        &self,
        mut optional_utxos: Vec<OutputGroup>,
        target_amount: i64,
    ) -> Vec<OutputGroup> {
        #[cfg(not(test))]
        optional_utxos.shuffle(&mut thread_rng());
        #[cfg(test)]
        {
            let seed = [0; 32];
            let mut rng: StdRng = SeedableRng::from_seed(seed);
            optional_utxos.shuffle(&mut rng);
        }

        optional_utxos
            .into_iter()
            .scan(0, |acc_value, utxo| {
                if *acc_value >= target_amount {
                    None
                } else {
                    *acc_value += utxo.effective_value;
                    Some(utxo)
                }
            })
            .collect::<Vec<_>>()
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use bitcoin::{Address, OutPoint, Script, TxOut};

    use super::*;
    use crate::database::{BatchOperations, MemoryDatabase};
    use crate::types::*;
    use crate::wallet::Vbytes;

    use rand::rngs::StdRng;
    use rand::seq::SliceRandom;
    use rand::{Rng, SeedableRng};

    // n. of items on witness (1WU) + signature len (1WU) + signature and sighash (72WU)
    // + pubkey len (1WU) + pubkey (33WU) + script sig len (1 byte, 4WU)
    const P2WPKH_SATISFACTION_SIZE: usize = 1 + 1 + 72 + 1 + 33 + 4;

    const FEE_AMOUNT: u64 = 50;

    fn utxo(value: u64, index: u32) -> WeightedUtxo {
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
            }),
        }
    }

    fn get_test_utxos() -> Vec<WeightedUtxo> {
        vec![
            utxo(100_000, 0),
            utxo(FEE_AMOUNT as u64 - 40, 1),
            utxo(200_000, 2),
        ]
    }

    fn get_test_weighted_drain_script() -> WeightedScriptPubkey {
        let script_p2wpkh = Address::from_str("bc1qxlh2mnc0yqwas76gqq665qkggee5m98t8yskd8")
            .unwrap()
            .script_pubkey();
        WeightedScriptPubkey {
            satisfaction_weight: P2WPKH_SATISFACTION_SIZE,
            script_pubkey: script_p2wpkh,
        }
    }

    fn setup_database_and_get_oldest_first_test_utxos<D: Database>(
        database: &mut D,
    ) -> Vec<WeightedUtxo> {
        // ensure utxos are from different tx
        let utxo1 = utxo(120_000, 1);
        let utxo2 = utxo(80_000, 2);
        let utxo3 = utxo(300_000, 3);

        // add tx to DB so utxos are sorted by blocktime asc
        // utxos will be selected by the following order
        // utxo1(blockheight 1) -> utxo2(blockheight 2), utxo3 (blockheight 3)
        // timestamp are all set as the same to ensure that only block height is used in sorting
        let utxo1_tx_details = TransactionDetails {
            transaction: None,
            txid: utxo1.utxo.outpoint().txid,
            received: 1,
            sent: 0,
            fee: None,
            confirmation_time: Some(BlockTime {
                height: 1,
                timestamp: 1231006505,
            }),
        };

        let utxo2_tx_details = TransactionDetails {
            transaction: None,
            txid: utxo2.utxo.outpoint().txid,
            received: 1,
            sent: 0,
            fee: None,
            confirmation_time: Some(BlockTime {
                height: 2,
                timestamp: 1231006505,
            }),
        };

        let utxo3_tx_details = TransactionDetails {
            transaction: None,
            txid: utxo3.utxo.outpoint().txid,
            received: 1,
            sent: 0,
            fee: None,
            confirmation_time: Some(BlockTime {
                height: 3,
                timestamp: 1231006505,
            }),
        };

        database.set_tx(&utxo1_tx_details).unwrap();
        database.set_tx(&utxo2_tx_details).unwrap();
        database.set_tx(&utxo3_tx_details).unwrap();

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
                        value: rng.gen_range(0, 200000000),
                        script_pubkey: Script::new(),
                    },
                    keychain: KeychainKind::External,
                    is_spent: false,
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
            }),
        };
        vec![utxo; utxos_number]
    }

    fn sum_random_utxos(mut rng: &mut StdRng, utxos: &mut Vec<WeightedUtxo>) -> u64 {
        let utxos_picked_len = rng.gen_range(2, utxos.len() / 2);
        utxos.shuffle(&mut rng);
        utxos[..utxos_picked_len]
            .iter()
            .map(|u| u.utxo.txout().value)
            .sum()
    }

    #[test]
    fn test_largest_first_coin_selection_success() {
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();
        let target_amount = 250_000 + FEE_AMOUNT;
        let weighted_drain_script = get_test_weighted_drain_script();

        let result = process_and_select_coins(
            LargestFirstCoinSelection::default(),
            &database,
            utxos,
            vec![],
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), 300_010);
        assert_eq!(result.fee_amount, 204);
    }

    #[test]
    fn test_largest_first_coin_selection_use_only_necessary() {
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();
        let target_amount = 20_000 + FEE_AMOUNT;
        let weighted_drain_script = get_test_weighted_drain_script();

        let result = process_and_select_coins(
            LargestFirstCoinSelection::default(),
            &database,
            vec![],
            utxos,
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap();

        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected_amount(), 200_000);
        assert_eq!(result.fee_amount, 68);
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_largest_first_coin_selection_insufficient_funds() {
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();
        let target_amount = 500_000 + FEE_AMOUNT;
        let weighted_drain_script = get_test_weighted_drain_script();

        process_and_select_coins(
            LargestFirstCoinSelection::default(),
            &database,
            vec![],
            utxos,
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap();
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_largest_first_coin_selection_insufficient_funds_high_fees() {
        let fee_rate = FeeRate::from_sat_per_vb(1000.0);
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();
        let target_amount = 250_000 + FEE_AMOUNT;
        let weighted_drain_script = get_test_weighted_drain_script();

        process_and_select_coins(
            LargestFirstCoinSelection::default(),
            &database,
            vec![],
            utxos,
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap();
    }

    #[test]
    fn test_oldest_first_coin_selection_success() {
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let mut database = MemoryDatabase::default();
        let utxos = setup_database_and_get_oldest_first_test_utxos(&mut database);
        let target_amount = 180_000 + FEE_AMOUNT;
        let weighted_drain_script = get_test_weighted_drain_script();

        let result = process_and_select_coins(
            OldestFirstCoinSelection::default(),
            &database,
            vec![],
            utxos,
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap();

        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected_amount(), 200_000);
        assert_eq!(result.fee_amount, 136);
    }

    #[test]
    fn test_oldest_first_coin_selection_utxo_not_in_db_will_be_selected_last() {
        // ensure utxos are from different tx
        let utxo1 = utxo(120_000, 1);
        let utxo2 = utxo(80_000, 2);
        let utxo3 = utxo(300_000, 3);

        let mut database = MemoryDatabase::default();

        // add tx to DB so utxos are sorted by blocktime asc
        // utxos will be selected by the following order
        // utxo1(blockheight 1) -> utxo2(blockheight 2), utxo3 (not exist in DB)
        // timestamp are all set as the same to ensure that only block height is used in sorting
        let utxo1_tx_details = TransactionDetails {
            transaction: None,
            txid: utxo1.utxo.outpoint().txid,
            received: 1,
            sent: 0,
            fee: None,
            confirmation_time: Some(BlockTime {
                height: 1,
                timestamp: 1231006505,
            }),
        };

        let utxo2_tx_details = TransactionDetails {
            transaction: None,
            txid: utxo2.utxo.outpoint().txid,
            received: 1,
            sent: 0,
            fee: None,
            confirmation_time: Some(BlockTime {
                height: 2,
                timestamp: 1231006505,
            }),
        };

        database.set_tx(&utxo1_tx_details).unwrap();
        database.set_tx(&utxo2_tx_details).unwrap();

        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let utxos = vec![utxo3, utxo1, utxo2];
        let target_amount = 180_000 + FEE_AMOUNT;
        let weighted_drain_script = get_test_weighted_drain_script();

        let result = process_and_select_coins(
            OldestFirstCoinSelection::default(),
            &database,
            vec![],
            utxos,
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap();

        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected_amount(), 200_000);
        assert_eq!(result.fee_amount, 136);
    }

    #[test]
    fn test_oldest_first_coin_selection_use_all() {
        let mut database = MemoryDatabase::default();
        let utxos = setup_database_and_get_oldest_first_test_utxos(&mut database);
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let target_amount = 20_000 + FEE_AMOUNT;
        let weighted_drain_script = get_test_weighted_drain_script();

        let result = process_and_select_coins(
            OldestFirstCoinSelection::default(),
            &database,
            utxos,
            vec![],
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), 500_000);
        assert_eq!(result.fee_amount, 204);
    }

    #[test]
    fn test_oldest_first_coin_selection_use_only_necessary() {
        let mut database = MemoryDatabase::default();
        let utxos = setup_database_and_get_oldest_first_test_utxos(&mut database);
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let target_amount = 20_000 + FEE_AMOUNT;
        let weighted_drain_script = get_test_weighted_drain_script();

        let result = process_and_select_coins(
            OldestFirstCoinSelection::default(),
            &database,
            vec![],
            utxos,
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap();

        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected_amount(), 120_000);
        assert_eq!(result.fee_amount, 68);
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_oldest_first_coin_selection_insufficient_funds() {
        let mut database = MemoryDatabase::default();
        let utxos = setup_database_and_get_oldest_first_test_utxos(&mut database);
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let target_amount = 600_000 + FEE_AMOUNT;
        let weighted_drain_script = get_test_weighted_drain_script();

        process_and_select_coins(
            OldestFirstCoinSelection::default(),
            &database,
            vec![],
            utxos,
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap();
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_oldest_first_coin_selection_insufficient_funds_high_fees() {
        let mut database = MemoryDatabase::default();
        let utxos = setup_database_and_get_oldest_first_test_utxos(&mut database);
        let target_amount: u64 = utxos.iter().map(|wu| wu.utxo.txout().value).sum::<u64>() - 50;
        let fee_rate = FeeRate::from_sat_per_vb(1000.0);
        let weighted_drain_script = get_test_weighted_drain_script();

        process_and_select_coins(
            OldestFirstCoinSelection::default(),
            &database,
            vec![],
            utxos,
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap();
    }

    #[test]
    fn test_bnb_coin_selection_success() {
        // In this case bnb won't find a suitable match and single random draw will
        // select three outputs
        let utxos = generate_same_value_utxos(100_000, 20);
        let database = MemoryDatabase::default();
        let target_amount = 250_000 + FEE_AMOUNT;
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let weighted_drain_script = get_test_weighted_drain_script();

        let result = process_and_select_coins(
            BranchAndBoundCoinSelection::default(),
            &database,
            vec![],
            utxos,
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), 300_000);
        assert_eq!(result.fee_amount, 204);
    }

    #[test]
    fn test_bnb_coin_selection_required_are_enough() {
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();
        let target_amount = 20_000 + FEE_AMOUNT;
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let weighted_drain_script = get_test_weighted_drain_script();

        let result = process_and_select_coins(
            BranchAndBoundCoinSelection::default(),
            &database,
            utxos.clone(),
            utxos,
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), 300_010);
        assert_eq!(result.fee_amount, 204);
    }

    #[test]
    fn test_bnb_coin_selection_optional_are_enough() {
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();
        let target_amount = 299756 + FEE_AMOUNT;
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let weighted_drain_script = get_test_weighted_drain_script();

        let result = process_and_select_coins(
            BranchAndBoundCoinSelection::default(),
            &database,
            vec![],
            utxos,
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap();

        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected_amount(), 300_000);
        assert_eq!(result.fee_amount, 136);
    }

    #[test]
    fn test_bnb_coin_selection_required_not_enough() {
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();

        let required = vec![utxos[0].clone()];
        let mut optional = utxos[1..].to_vec();
        optional.push(utxo(500_000, 3));

        // Defensive assertions, for sanity and in case someone changes the test utxos vector.
        let amount: u64 = required.iter().map(|u| u.utxo.txout().value).sum();
        assert_eq!(amount, 100_000);
        let amount: u64 = optional.iter().map(|u| u.utxo.txout().value).sum();
        assert!(amount > 150_000);

        let target_amount = 150_000 + FEE_AMOUNT;
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let weighted_drain_script = get_test_weighted_drain_script();

        let result = process_and_select_coins(
            BranchAndBoundCoinSelection::default(),
            &database,
            required,
            optional,
            fee_rate,
            target_amount,
            &weighted_drain_script,
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
        let database = MemoryDatabase::default();
        let target_amount = 500_000 + FEE_AMOUNT;
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let weighted_drain_script = get_test_weighted_drain_script();

        process_and_select_coins(
            BranchAndBoundCoinSelection::default(),
            &database,
            vec![],
            utxos,
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap();
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_bnb_coin_selection_insufficient_funds_high_fees() {
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();
        let target_amount = 250_000 + FEE_AMOUNT;
        let fee_rate = FeeRate::from_sat_per_vb(1000.0);
        let weighted_drain_script = get_test_weighted_drain_script();

        process_and_select_coins(
            BranchAndBoundCoinSelection::default(),
            &database,
            vec![],
            utxos,
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap();
    }

    #[test]
    fn test_bnb_coin_selection_check_fee_rate() {
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();
        let target_amount = 99932; // first utxo's effective value
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let weighted_drain_script = get_test_weighted_drain_script();

        let result = process_and_select_coins(
            BranchAndBoundCoinSelection::new(0),
            &database,
            vec![],
            utxos,
            fee_rate,
            target_amount,
            &weighted_drain_script,
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
        let database = MemoryDatabase::default();
        let weighted_drain_script = get_test_weighted_drain_script();

        for _i in 0..200 {
            let mut optional_utxos = generate_random_utxos(&mut rng, 16);
            let fee_rate = FeeRate::from_sat_per_vb(0.0);
            let target_amount = sum_random_utxos(&mut rng, &mut optional_utxos);

            let result = process_and_select_coins(
                BranchAndBoundCoinSelection::new(0),
                &database,
                vec![],
                optional_utxos,
                fee_rate,
                target_amount,
                &weighted_drain_script,
            )
            .unwrap();

            assert_eq!(result.selected_amount(), target_amount);
        }
    }

    #[test]
    #[should_panic(expected = "BnBNoExactMatch")]
    fn test_bnb_function_no_exact_match() {
        let fee_rate = FeeRate::from_sat_per_vb(10.0);
        let optional_utxos: Vec<OutputGroup> = get_test_utxos()
            .into_iter()
            .map(|u| OutputGroup::new(u, fee_rate))
            .collect();
        let available_value = optional_utxos.iter().map(|x| x.effective_value).sum();

        let size_of_change = 31;
        let cost_of_change = size_of_change as f32 * fee_rate.as_sat_per_vb();
        let target_amount = 20_000 + FEE_AMOUNT;

        BranchAndBoundCoinSelection::new(size_of_change)
            .bnb(
                optional_utxos,
                target_amount as i64,
                cost_of_change,
                available_value,
            )
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "BnBTotalTriesExceeded")]
    fn test_bnb_function_tries_exceeded() {
        let fee_rate = FeeRate::from_sat_per_vb(10.0);
        let optional_utxos: Vec<OutputGroup> = generate_same_value_utxos(100_000, 100_000)
            .into_iter()
            .map(|u| OutputGroup::new(u, fee_rate))
            .collect();
        let available_value = optional_utxos.iter().map(|x| x.effective_value).sum();

        let size_of_change = 31;
        let cost_of_change = size_of_change as f32 * fee_rate.as_sat_per_vb();
        let target_amount = 20_000 + FEE_AMOUNT;

        BranchAndBoundCoinSelection::new(size_of_change)
            .bnb(
                optional_utxos,
                target_amount as i64,
                cost_of_change,
                available_value,
            )
            .unwrap();
    }

    // The match won't be exact but still in the range
    #[test]
    fn test_bnb_function_almost_exact_match_with_fees() {
        let database = MemoryDatabase::default();
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let size_of_change = 31;
        let cost_of_change = size_of_change as f32 * fee_rate.as_sat_per_vb();

        let utxos: Vec<_> = generate_same_value_utxos(50_000, 10);

        // 2*(value of 1 utxo)  - 2*(1 utxo fees with 1.0sat/vbyte fee rate) -
        // cost_of_change + 5.
        let target_amount = 2 * 50_000 - 2 * 67 - cost_of_change.ceil() as i64 + 5;
        let weighted_drain_script = get_test_weighted_drain_script();

        let result = process_and_select_coins(
            BranchAndBoundCoinSelection::new(size_of_change),
            &database,
            vec![],
            utxos,
            fee_rate,
            target_amount as u64,
            &weighted_drain_script,
        )
        .unwrap();

        assert_eq!(result.fee_amount, 136);
        assert_eq!(result.selected_amount(), 100_000);
    }

    // TODO: bnb() function should be optimized, and this test should be done with more utxos
    #[test]
    fn test_bnb_function_exact_match_more_utxos() {
        let database = MemoryDatabase::default();
        let seed = [0; 32];
        let mut rng: StdRng = SeedableRng::from_seed(seed);
        let fee_rate = FeeRate::from_sat_per_vb(0.0);

        for _ in 0..200 {
            let optional_utxos: Vec<_> = generate_random_utxos(&mut rng, 40)
                .into_iter()
                .map(|u| OutputGroup::new(u, fee_rate))
                .collect();
            let available_value = optional_utxos.iter().map(|x| x.effective_value).sum();

            let target_amount =
                optional_utxos[3].effective_value + optional_utxos[23].effective_value;

            let selected_utxos = BranchAndBoundCoinSelection::new(0)
                .coin_select(
                    &database,
                    optional_utxos,
                    fee_rate,
                    // fee rate is zero so effective value can't be negative
                    target_amount as u64,
                    available_value,
                )
                .unwrap();

            let selected_amount: u64 = selected_utxos
                .iter()
                .map(|og| og.weighted_utxo.utxo.txout().value)
                .sum();

            assert_eq!(selected_amount, target_amount as u64);
        }
    }

    #[test]
    fn test_single_random_draw_function_success() {
        let seed = [0; 32];
        let mut rng: StdRng = SeedableRng::from_seed(seed);
        let mut utxos = generate_random_utxos(&mut rng, 300);
        let target_amount = sum_random_utxos(&mut rng, &mut utxos) + FEE_AMOUNT;
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let optional_utxos: Vec<_> = utxos
            .into_iter()
            .map(|u| OutputGroup::new(u, fee_rate))
            .collect();

        let selected_utxos = BranchAndBoundCoinSelection::default()
            .single_random_draw(optional_utxos, target_amount as i64);

        let selected_amount: u64 = selected_utxos
            .iter()
            .map(|og| og.weighted_utxo.utxo.txout().value)
            .sum();

        let fee_amount: u64 = selected_utxos.iter().map(|og| og.fee).sum();

        assert!(selected_amount > target_amount);
        assert_eq!(fee_amount, (selected_utxos.len() * 68) as u64);
    }

    #[test]
    fn test_bnb_exclude_negative_effective_value() {
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();
        let fee_rate = FeeRate::from_sat_per_vb(10.0);
        let weighted_drain_script = get_test_weighted_drain_script();
        let target_amount = 500_000;

        let err = process_and_select_coins(
            BranchAndBoundCoinSelection::default(),
            &database,
            vec![],
            utxos,
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            Error::InsufficientFunds {
                available: 300_000,
                ..
            }
        ));
    }

    #[test]
    fn test_bnb_include_negative_effective_value_when_required() {
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();
        let fee_rate = FeeRate::from_sat_per_vb(10.0);
        let target_amount = 500_000;
        let weighted_drain_script = get_test_weighted_drain_script();

        let (required, optional) = utxos
            .into_iter()
            .partition(|u| matches!(u, WeightedUtxo { utxo, .. } if utxo.txout().value < 1000));

        let err = process_and_select_coins(
            BranchAndBoundCoinSelection::default(),
            &database,
            required,
            optional,
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            Error::InsufficientFunds {
                available: 300_010,
                ..
            }
        ));
    }

    #[test]
    fn test_bnb_sum_of_effective_value_negative() {
        let utxos = get_test_utxos();
        let database = MemoryDatabase::default();
        let fee_rate = FeeRate::from_sat_per_vb(10.0);
        let target_amount = 500_000;
        let weighted_drain_script = get_test_weighted_drain_script();

        let err = process_and_select_coins(
            BranchAndBoundCoinSelection::default(),
            &database,
            utxos,
            vec![],
            fee_rate,
            target_amount,
            &weighted_drain_script,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            Error::InsufficientFunds {
                available: 300_010,
                ..
            }
        ));
    }
}
