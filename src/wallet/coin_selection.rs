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
//! # use bdk::wallet::{self, coin_selection::*, multi_tracker::*};
//! # use bdk::*;
//! # const TXIN_BASE_WEIGHT: usize = (32 + 4 + 4 + 1) * 4;
//! #[derive(Debug)]
//! struct AlwaysSpendEverything;
//!
//! impl CoinSelectionAlgorithm for AlwaysSpendEverything {
//!     fn coin_select<D: MultiTracker>(
//!         &self,
//!         database: &D,
//!         required_utxos: Vec<WeightedUtxo>,
//!         optional_utxos: Vec<WeightedUtxo>,
//!         fee_rate: FeeRate,
//!         amount_needed: u64,
//!         fee_amount: u64,
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
//!         let amount_needed_with_fees = (fee_amount + additional_fees) + amount_needed;
//!         if amount_needed_with_fees > selected_amount {
//!             return Err(bdk::Error::InsufficientFunds {
//!                 needed: amount_needed_with_fees,
//!                 available: selected_amount,
//!             });
//!         }
//!
//!         Ok(CoinSelectionResult {
//!             selected: all_utxos_selected,
//!             fee_amount: fee_amount + additional_fees,
//!         })
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

use crate::types::FeeRate;
use crate::WeightedUtxo;
use crate::{error::Error, Utxo};

use rand::seq::SliceRandom;
#[cfg(not(test))]
use rand::thread_rng;
#[cfg(test)]
use rand::{rngs::StdRng, SeedableRng};
use std::collections::HashMap;
use std::convert::TryInto;

use super::multi_tracker::MultiTracker;

/// Default coin selection algorithm used by [`TxBuilder`](super::tx_builder::TxBuilder) if not
/// overridden
#[cfg(not(test))]
pub type DefaultCoinSelectionAlgorithm = BranchAndBoundCoinSelection;
#[cfg(test)]
pub type DefaultCoinSelectionAlgorithm = LargestFirstCoinSelection; // make the tests more predictable

// Base weight of a Txin, not counting the weight needed for satisfying it.
// prev_txid (32 bytes) + prev_vout (4 bytes) + sequence (4 bytes) + script_len (1 bytes)
pub(crate) const TXIN_BASE_WEIGHT: usize = (32 + 4 + 4 + 1) * 4;

/// Result of a successful coin selection
#[derive(Debug)]
pub struct CoinSelectionResult {
    /// List of outputs selected for use as inputs
    pub selected: Vec<Utxo>,
    /// Total fee amount in satoshi
    pub fee_amount: u64,
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
pub trait CoinSelectionAlgorithm: std::fmt::Debug {
    /// Perform the coin selection
    ///
    /// - `database`: a reference to the wallet's database that can be used to lookup additional
    ///               details for a specific UTXO
    /// - `required_utxos`: the utxos that must be spent regardless of `amount_needed` with their
    ///                     weight cost
    /// - `optional_utxos`: the remaining available utxos to satisfy `amount_needed` with their
    ///                     weight cost
    /// - `fee_rate`: fee rate to use
    /// - `amount_needed`: the amount in satoshi to select
    /// - `fee_amount`: the amount of fees in satoshi already accumulated from adding outputs and
    ///                 the transaction's header
    fn coin_select<Sc: MultiTracker>(
        &self,
        database: &Sc,
        required_utxos: Vec<WeightedUtxo>,
        optional_utxos: Vec<WeightedUtxo>,
        fee_rate: FeeRate,
        amount_needed: u64,
        fee_amount: u64,
    ) -> Result<CoinSelectionResult, Error>;
}

/// Simple and dumb coin selection
///
/// This coin selection algorithm sorts the available UTXOs by value and then picks them starting
/// from the largest ones until the required amount is reached.
#[derive(Debug, Default, Clone, Copy)]
pub struct LargestFirstCoinSelection;

impl CoinSelectionAlgorithm for LargestFirstCoinSelection {
    fn coin_select<D: MultiTracker>(
        &self,
        _database: &D,
        required_utxos: Vec<WeightedUtxo>,
        mut optional_utxos: Vec<WeightedUtxo>,
        fee_rate: FeeRate,
        amount_needed: u64,
        fee_amount: u64,
    ) -> Result<CoinSelectionResult, Error> {
        log::debug!(
            "amount_needed = `{}`, fee_amount = `{}`, fee_rate = `{:?}`",
            amount_needed,
            fee_amount,
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

        select_sorted_utxos(utxos, fee_rate, amount_needed, fee_amount)
    }
}

/// OldestFirstCoinSelection always picks the utxo with the smallest blockheight to add to the selected coins next
///
/// This coin selection algorithm sorts the available UTXOs by blockheight and then picks them starting
/// from the oldest ones until the required amount is reached.
#[derive(Debug, Default, Clone, Copy)]
pub struct OldestFirstCoinSelection;

impl CoinSelectionAlgorithm for OldestFirstCoinSelection {
    fn coin_select<D: MultiTracker>(
        &self,
        database: &D,
        required_utxos: Vec<WeightedUtxo>,
        mut optional_utxos: Vec<WeightedUtxo>,
        fee_rate: FeeRate,
        amount_needed: u64,
        fee_amount: u64,
    ) -> Result<CoinSelectionResult, Error> {
        // query db and create a blockheight lookup table
        let blockheights = optional_utxos
            .iter()
            .map(|wu| wu.utxo.outpoint().txid)
            // fold is used so we can skip db query for txid that already exist in hashmap acc
            .fold(Ok(HashMap::new()), |bh_result_acc, txid| {
                bh_result_acc.and_then(|mut bh_acc| {
                    if bh_acc.contains_key(&txid) {
                        Ok(bh_acc)
                    } else {
                        database.get_tx(&txid).map(|details| {
                            let conf_time =
                                details.and_then(|tx| tx.confirmed.map(|conf| conf.height));
                            bh_acc.insert(txid, conf_time);
                            println!(
                                "OldestFirstCoinSelection: insert({}, {:?})",
                                txid, conf_time
                            );
                            bh_acc
                        })
                    }
                })
            })?;

        // We put the "required UTXOs" first and make sure the optional UTXOs are sorted from
        // oldest to newest according to blocktime
        // For utxo that doesn't exist in DB, they will have lowest priority to be selected
        let utxos = {
            optional_utxos.sort_unstable_by_key(|wu| {
                match blockheights.get(&wu.utxo.outpoint().txid) {
                    Some(Some(blockheight)) => blockheight,
                    _ => &u32::MAX,
                }
            });

            required_utxos
                .into_iter()
                .map(|utxo| (true, utxo))
                .chain(optional_utxos.into_iter().map(|utxo| (false, utxo)))
        };

        select_sorted_utxos(utxos, fee_rate, amount_needed, fee_amount)
    }
}

fn select_sorted_utxos(
    utxos: impl Iterator<Item = (bool, WeightedUtxo)>,
    fee_rate: FeeRate,
    amount_needed: u64,
    mut fee_amount: u64,
) -> Result<CoinSelectionResult, Error> {
    let mut selected_amount = 0;
    let selected = utxos
        .scan(
            (&mut selected_amount, &mut fee_amount),
            |(selected_amount, fee_amount), (must_use, weighted_utxo)| {
                if must_use || **selected_amount < amount_needed + **fee_amount {
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

    let amount_needed_with_fees = amount_needed + fee_amount;
    if selected_amount < amount_needed_with_fees {
        return Err(Error::InsufficientFunds {
            needed: amount_needed_with_fees,
            available: selected_amount,
        });
    }

    Ok(CoinSelectionResult {
        selected,
        fee_amount,
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

impl CoinSelectionAlgorithm for BranchAndBoundCoinSelection {
    fn coin_select<D: MultiTracker>(
        &self,
        _database: &D,
        required_utxos: Vec<WeightedUtxo>,
        optional_utxos: Vec<WeightedUtxo>,
        fee_rate: FeeRate,
        amount_needed: u64,
        fee_amount: u64,
    ) -> Result<CoinSelectionResult, Error> {
        // Mapping every (UTXO, usize) to an output group
        let required_utxos: Vec<OutputGroup> = required_utxos
            .into_iter()
            .map(|u| OutputGroup::new(u, fee_rate))
            .collect();

        // Mapping every (UTXO, usize) to an output group.
        let optional_utxos: Vec<OutputGroup> = optional_utxos
            .into_iter()
            .map(|u| OutputGroup::new(u, fee_rate))
            .collect();

        let curr_value = required_utxos
            .iter()
            .fold(0, |acc, x| acc + x.effective_value);

        let curr_available_value = optional_utxos
            .iter()
            .fold(0, |acc, x| acc + x.effective_value);

        let actual_target = fee_amount + amount_needed;
        let cost_of_change = self.size_of_change as f32 * fee_rate.as_sat_vb();

        let expected = (curr_available_value + curr_value)
            .try_into()
            .map_err(|_| {
                Error::Generic("Sum of UTXO spendable values does not fit into u64".to_string())
            })?;

        if expected < actual_target {
            return Err(Error::InsufficientFunds {
                needed: actual_target,
                available: expected,
            });
        }

        let actual_target = actual_target
            .try_into()
            .expect("Bitcoin amount to fit into i64");

        if curr_value > actual_target {
            return Ok(BranchAndBoundCoinSelection::calculate_cs_result(
                vec![],
                required_utxos,
                fee_amount,
            ));
        }

        Ok(self
            .bnb(
                required_utxos.clone(),
                optional_utxos.clone(),
                curr_value,
                curr_available_value,
                actual_target,
                fee_amount,
                cost_of_change,
            )
            .unwrap_or_else(|_| {
                self.single_random_draw(
                    required_utxos,
                    optional_utxos,
                    curr_value,
                    actual_target,
                    fee_amount,
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
        actual_target: i64,
        fee_amount: u64,
        cost_of_change: f32,
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
            if curr_value + curr_available_value < actual_target
                || curr_value > actual_target + cost_of_change as i64
            {
                backtrack = true;
            } else if curr_value >= actual_target {
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
                if curr_value == actual_target {
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
            .collect();

        Ok(BranchAndBoundCoinSelection::calculate_cs_result(
            selected_utxos,
            required_utxos,
            fee_amount,
        ))
    }

    fn single_random_draw(
        &self,
        required_utxos: Vec<OutputGroup>,
        mut optional_utxos: Vec<OutputGroup>,
        curr_value: i64,
        actual_target: i64,
        fee_amount: u64,
    ) -> CoinSelectionResult {
        #[cfg(not(test))]
        optional_utxos.shuffle(&mut thread_rng());
        #[cfg(test)]
        {
            let seed = [0; 32];
            let mut rng: StdRng = SeedableRng::from_seed(seed);
            optional_utxos.shuffle(&mut rng);
        }

        let selected_utxos = optional_utxos
            .into_iter()
            .scan(curr_value, |curr_value, utxo| {
                if *curr_value >= actual_target {
                    None
                } else {
                    *curr_value += utxo.effective_value;
                    Some(utxo)
                }
            })
            .collect::<Vec<_>>();

        BranchAndBoundCoinSelection::calculate_cs_result(selected_utxos, required_utxos, fee_amount)
    }

    fn calculate_cs_result(
        mut selected_utxos: Vec<OutputGroup>,
        mut required_utxos: Vec<OutputGroup>,
        mut fee_amount: u64,
    ) -> CoinSelectionResult {
        selected_utxos.append(&mut required_utxos);
        fee_amount += selected_utxos.iter().map(|u| u.fee).sum::<u64>();
        let selected = selected_utxos
            .into_iter()
            .map(|u| u.weighted_utxo.utxo)
            .collect::<Vec<_>>();

        CoinSelectionResult {
            selected,
            fee_amount,
        }
    }
}

#[cfg(test)]
mod test {
    use std::cell::RefCell;
    use std::str::FromStr;

    use bitcoin::{Network, OutPoint, Script, Transaction, TxOut, Txid};

    use super::*;
    use crate::database::{BatchOperations, Database, MemoryDatabase};
    use crate::descriptor::{ExtendedDescriptor, IntoWalletDescriptor};
    use crate::types::*;
    use crate::wallet::multi_tracker::LegacyTracker;
    use crate::wallet::utils::SecpCtx;
    use crate::wallet::Vbytes;

    use rand::rngs::StdRng;
    use rand::seq::SliceRandom;
    use rand::{Rng, SeedableRng};

    const P2WPKH_WITNESS_SIZE: usize = 73 + 33 + 2;

    const FEE_AMOUNT: u64 = 50;

    fn new_desc() -> ExtendedDescriptor {
        "wsh(pk(cRYhyJQzA92kLQUtJZbVVQrBoQwSm4WT23sanTXYrKoUMjX8ph1Z))"
            .into_wallet_descriptor(&SecpCtx::new(), Network::Regtest)
            .unwrap()
            .0
    }

    fn new_db() -> RefCell<MemoryDatabase> {
        RefCell::new(MemoryDatabase::new())
    }

    fn new_collection<'a>(
        desc: &'a ExtendedDescriptor,
        db: &'a RefCell<MemoryDatabase>,
    ) -> LegacyTracker<'a, MemoryDatabase> {
        LegacyTracker::new(desc, None, Network::Regtest, db, Vec::new(), Vec::new())
    }

    fn utxo(value: u64, index: u32) -> WeightedUtxo {
        assert!(index < 10);
        let outpoint = OutPoint::from_str(&format!(
            "000000000000000000000000000000000000000000000000000000000000000{}:0",
            index
        ))
        .unwrap();
        WeightedUtxo {
            satisfaction_weight: P2WPKH_WITNESS_SIZE,
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

    // creates a tx with given utxo as single output.
    // updates utxo to contain correct txid in outpoint.
    fn tx(utxo: &mut WeightedUtxo, lock_time: u32) -> (Transaction, Txid) {
        let local = match &mut utxo.utxo {
            Utxo::Local(local) => local,
            _ => panic!(),
        };
        let tx = Transaction {
            version: 2,
            lock_time,
            input: Vec::new(),
            output: vec![local.txout.clone()],
        };
        let txid = tx.txid();
        local.outpoint.txid = txid;
        (tx, txid)
    }

    fn setup_database_and_get_oldest_first_test_utxos<D: Database>(
        database: &mut D,
    ) -> Vec<WeightedUtxo> {
        // ensure utxos are from different tx
        let mut utxo1 = utxo(120_000, 1);
        let mut utxo2 = utxo(80_000, 2);
        let mut utxo3 = utxo(300_000, 3);

        // let tx = |utxo: &WeightedUtxo, index: u32| {
        //     let local = match &utxo.utxo {
        //         Utxo::Local(local) => local,
        //         _ => panic!(),
        //     };
        //     Transaction{ version: 2, lock_time: index, input: Vec::new(), output: vec![local.txout.clone()] }
        // };

        let (tx1, txid1) = tx(&mut utxo1, 1);
        let (tx2, txid2) = tx(&mut utxo2, 2);
        let (tx3, txid3) = tx(&mut utxo3, 3);

        // add tx to DB so utxos are sorted by blocktime asc
        // utxos will be selected by the following order
        // utxo1(blockheight 1) -> utxo2(blockheight 2), utxo3 (blockheight 3)
        // timestamp are all set as the same to ensure that only block height is used in sorting
        let utxo1_tx_details = TransactionDetails {
            transaction: Some(tx1),
            txid: txid1,
            // txid: utxo1.utxo.outpoint().txid,
            received: 1,
            sent: 0,
            fee: None,
            confirmation_time: Some(BlockTime {
                height: 1,
                timestamp: 1231006505,
            }),
        };

        let utxo2_tx_details = TransactionDetails {
            transaction: Some(tx2),
            txid: txid2,
            // txid: utxo2.utxo.outpoint().txid,
            received: 1,
            sent: 0,
            fee: None,
            confirmation_time: Some(BlockTime {
                height: 2,
                timestamp: 1231006505,
            }),
        };

        let utxo3_tx_details = TransactionDetails {
            transaction: Some(tx3),
            txid: txid3,
            // txid: utxo3.utxo.outpoint().txid,
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
                satisfaction_weight: P2WPKH_WITNESS_SIZE,
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
            satisfaction_weight: P2WPKH_WITNESS_SIZE,
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
        let utxos = get_test_utxos();
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);

        let result = LargestFirstCoinSelection::default()
            .coin_select(
                &cache,
                utxos,
                vec![],
                FeeRate::from_sat_per_vb(1.0),
                250_000,
                FEE_AMOUNT,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), 300_010);
        assert_eq!(result.fee_amount, 254)
    }

    #[test]
    fn test_largest_first_coin_selection_use_all() {
        let utxos = get_test_utxos();
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);

        let result = LargestFirstCoinSelection::default()
            .coin_select(
                &cache,
                utxos,
                vec![],
                FeeRate::from_sat_per_vb(1.0),
                20_000,
                FEE_AMOUNT,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), 300_010);
        assert_eq!(result.fee_amount, 254);
    }

    #[test]
    fn test_largest_first_coin_selection_use_only_necessary() {
        let utxos = get_test_utxos();
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);

        let result = LargestFirstCoinSelection::default()
            .coin_select(
                &cache,
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                20_000,
                FEE_AMOUNT,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected_amount(), 200_000);
        assert_eq!(result.fee_amount, 118);
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_largest_first_coin_selection_insufficient_funds() {
        let utxos = get_test_utxos();
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);

        LargestFirstCoinSelection::default()
            .coin_select(
                &cache,
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                500_000,
                FEE_AMOUNT,
            )
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_largest_first_coin_selection_insufficient_funds_high_fees() {
        let utxos = get_test_utxos();
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);

        LargestFirstCoinSelection::default()
            .coin_select(
                &cache,
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1000.0),
                250_000,
                FEE_AMOUNT,
            )
            .unwrap();
    }

    #[test]
    fn test_oldest_first_coin_selection_success() {
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);
        let utxos = { setup_database_and_get_oldest_first_test_utxos(&mut *db.borrow_mut()) };

        let result = OldestFirstCoinSelection::default()
            .coin_select(
                &cache,
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                180_000,
                FEE_AMOUNT,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected_amount(), 200_000);
        assert_eq!(result.fee_amount, 186)
    }

    #[test]
    fn test_oldest_first_coin_selection_utxo_not_in_db_will_be_selected_last() {
        // ensure utxos are from different tx
        let mut utxo1 = utxo(120_000, 1);
        let mut utxo2 = utxo(80_000, 2);
        let mut utxo3 = utxo(300_000, 3);

        let (tx1, txid1) = tx(&mut utxo1, 1);
        let (tx2, txid2) = tx(&mut utxo2, 2);
        let _ = tx(&mut utxo3, 3);

        // let txid1 = tx1.txid();
        // let txid2 = tx2.txid();

        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);

        // add tx to DB so utxos are sorted by blocktime asc
        // utxos will be selected by the following order
        // utxo1(blockheight 1) -> utxo2(blockheight 2), utxo3 (not exist in DB)
        // timestamp are all set as the same to ensure that only block height is used in sorting
        let utxo1_tx_details = TransactionDetails {
            transaction: Some(tx1),
            txid: txid1,
            // txid: utxo1.utxo.outpoint().txid,
            received: 1,
            sent: 0,
            fee: None,
            confirmation_time: Some(BlockTime {
                height: 1,
                timestamp: 1231006505,
            }),
        };

        let utxo2_tx_details = TransactionDetails {
            transaction: Some(tx2),
            txid: txid2,
            // txid: utxo2.utxo.outpoint().txid,
            received: 1,
            sent: 0,
            fee: None,
            confirmation_time: Some(BlockTime {
                height: 2,
                timestamp: 1231006505,
            }),
        };

        {
            let db = &mut *db.borrow_mut();
            db.set_tx(&utxo1_tx_details).unwrap();
            db.set_tx(&utxo2_tx_details).unwrap();
        }

        let result = OldestFirstCoinSelection::default()
            .coin_select(
                &cache,
                vec![],
                vec![utxo3, utxo1, utxo2],
                FeeRate::from_sat_per_vb(1.0),
                180_000,
                FEE_AMOUNT,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 2);
        assert_eq!(result.selected_amount(), 200_000);
        assert_eq!(result.fee_amount, 186)
    }

    #[test]
    fn test_oldest_first_coin_selection_use_all() {
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);
        let utxos = { setup_database_and_get_oldest_first_test_utxos(&mut *db.borrow_mut()) };

        let result = OldestFirstCoinSelection::default()
            .coin_select(
                &cache,
                utxos,
                vec![],
                FeeRate::from_sat_per_vb(1.0),
                20_000,
                FEE_AMOUNT,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), 500_000);
        assert_eq!(result.fee_amount, 254);
    }

    #[test]
    fn test_oldest_first_coin_selection_use_only_necessary() {
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);
        let utxos = { setup_database_and_get_oldest_first_test_utxos(&mut *db.borrow_mut()) };

        let result = OldestFirstCoinSelection::default()
            .coin_select(
                &cache,
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                20_000,
                FEE_AMOUNT,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected_amount(), 120_000);
        assert_eq!(result.fee_amount, 118);
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_oldest_first_coin_selection_insufficient_funds() {
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);
        let utxos = { setup_database_and_get_oldest_first_test_utxos(&mut *db.borrow_mut()) };

        OldestFirstCoinSelection::default()
            .coin_select(
                &cache,
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                600_000,
                FEE_AMOUNT,
            )
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_oldest_first_coin_selection_insufficient_funds_high_fees() {
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);
        let utxos = { setup_database_and_get_oldest_first_test_utxos(&mut *db.borrow_mut()) };

        let amount_needed: u64 =
            utxos.iter().map(|wu| wu.utxo.txout().value).sum::<u64>() - (FEE_AMOUNT + 50);

        OldestFirstCoinSelection::default()
            .coin_select(
                &cache,
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1000.0),
                amount_needed,
                FEE_AMOUNT,
            )
            .unwrap();
    }

    #[test]
    fn test_bnb_coin_selection_success() {
        // In this case bnb won't find a suitable match and single random draw will
        // select three outputs
        let utxos = generate_same_value_utxos(100_000, 20);

        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);

        let result = BranchAndBoundCoinSelection::default()
            .coin_select(
                &cache,
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                250_000,
                FEE_AMOUNT,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), 300_000);
        assert_eq!(result.fee_amount, 254);
    }

    #[test]
    fn test_bnb_coin_selection_required_are_enough() {
        let utxos = get_test_utxos();
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);

        let result = BranchAndBoundCoinSelection::default()
            .coin_select(
                &cache,
                utxos.clone(),
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                20_000,
                FEE_AMOUNT,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), 300_010);
        assert_eq!(result.fee_amount, 254);
    }

    #[test]
    fn test_bnb_coin_selection_optional_are_enough() {
        let utxos = get_test_utxos();
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);

        let result = BranchAndBoundCoinSelection::default()
            .coin_select(
                &cache,
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                299756,
                FEE_AMOUNT,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), 300010);
        assert_eq!(result.fee_amount, 254);
    }

    #[test]
    fn test_bnb_coin_selection_required_not_enough() {
        let utxos = get_test_utxos();
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);

        let required = vec![utxos[0].clone()];
        let mut optional = utxos[1..].to_vec();
        optional.push(utxo(500_000, 3));

        // Defensive assertions, for sanity and in case someone changes the test utxos vector.
        let amount: u64 = required.iter().map(|u| u.utxo.txout().value).sum();
        assert_eq!(amount, 100_000);
        let amount: u64 = optional.iter().map(|u| u.utxo.txout().value).sum();
        assert!(amount > 150_000);

        let result = BranchAndBoundCoinSelection::default()
            .coin_select(
                &cache,
                required,
                optional,
                FeeRate::from_sat_per_vb(1.0),
                150_000,
                FEE_AMOUNT,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 3);
        assert_eq!(result.selected_amount(), 300_010);
        assert!((result.fee_amount as f32 - 254.0).abs() < f32::EPSILON);
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_bnb_coin_selection_insufficient_funds() {
        let utxos = get_test_utxos();
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);

        BranchAndBoundCoinSelection::default()
            .coin_select(
                &cache,
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                500_000,
                FEE_AMOUNT,
            )
            .unwrap();
    }

    #[test]
    #[should_panic(expected = "InsufficientFunds")]
    fn test_bnb_coin_selection_insufficient_funds_high_fees() {
        let utxos = get_test_utxos();
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);

        BranchAndBoundCoinSelection::default()
            .coin_select(
                &cache,
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1000.0),
                250_000,
                FEE_AMOUNT,
            )
            .unwrap();
    }

    #[test]
    fn test_bnb_coin_selection_check_fee_rate() {
        let utxos = get_test_utxos();
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);
        let result = BranchAndBoundCoinSelection::new(0)
            .coin_select(
                &cache,
                vec![],
                utxos,
                FeeRate::from_sat_per_vb(1.0),
                99932, // first utxo's effective value
                0,
            )
            .unwrap();

        assert_eq!(result.selected.len(), 1);
        assert_eq!(result.selected_amount(), 100_000);
        let input_size = (TXIN_BASE_WEIGHT + P2WPKH_WITNESS_SIZE).vbytes();
        let epsilon = 0.5;
        assert!((1.0 - (result.fee_amount as f32 / input_size as f32)).abs() < epsilon);
    }

    #[test]
    fn test_bnb_coin_selection_exact_match() {
        let seed = [0; 32];
        let mut rng: StdRng = SeedableRng::from_seed(seed);
        let desc = new_desc();
        let db = new_db();
        let cache = new_collection(&desc, &db);

        for _i in 0..200 {
            let mut optional_utxos = generate_random_utxos(&mut rng, 16);
            let target_amount = sum_random_utxos(&mut rng, &mut optional_utxos);
            let result = BranchAndBoundCoinSelection::new(0)
                .coin_select(
                    &cache,
                    vec![],
                    optional_utxos,
                    FeeRate::from_sat_per_vb(0.0),
                    target_amount,
                    0,
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
        let cost_of_change = size_of_change as f32 * fee_rate.as_sat_vb();
        BranchAndBoundCoinSelection::new(size_of_change)
            .bnb(
                vec![],
                utxos,
                0,
                curr_available_value,
                20_000,
                FEE_AMOUNT,
                cost_of_change,
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
        let cost_of_change = size_of_change as f32 * fee_rate.as_sat_vb();

        BranchAndBoundCoinSelection::new(size_of_change)
            .bnb(
                vec![],
                utxos,
                0,
                curr_available_value,
                20_000,
                FEE_AMOUNT,
                cost_of_change,
            )
            .unwrap();
    }

    // The match won't be exact but still in the range
    #[test]
    fn test_bnb_function_almost_exact_match_with_fees() {
        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let size_of_change = 31;
        let cost_of_change = size_of_change as f32 * fee_rate.as_sat_vb();

        let utxos: Vec<_> = generate_same_value_utxos(50_000, 10)
            .into_iter()
            .map(|u| OutputGroup::new(u, fee_rate))
            .collect();

        let curr_value = 0;

        let curr_available_value = utxos.iter().fold(0, |acc, x| acc + x.effective_value);

        // 2*(value of 1 utxo)  - 2*(1 utxo fees with 1.0sat/vbyte fee rate) -
        // cost_of_change + 5.
        let target_amount = 2 * 50_000 - 2 * 67 - cost_of_change.ceil() as i64 + 5;

        let result = BranchAndBoundCoinSelection::new(size_of_change)
            .bnb(
                vec![],
                utxos,
                curr_value,
                curr_available_value,
                target_amount,
                FEE_AMOUNT,
                cost_of_change,
            )
            .unwrap();
        assert_eq!(result.selected_amount(), 100_000);
        assert_eq!(result.fee_amount, 186);
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

            let result = BranchAndBoundCoinSelection::new(0)
                .bnb(
                    vec![],
                    optional_utxos,
                    curr_value,
                    curr_available_value,
                    target_amount,
                    0,
                    0.0,
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
        let target_amount = sum_random_utxos(&mut rng, &mut utxos);

        let fee_rate = FeeRate::from_sat_per_vb(1.0);
        let utxos: Vec<OutputGroup> = utxos
            .into_iter()
            .map(|u| OutputGroup::new(u, fee_rate))
            .collect();

        let result = BranchAndBoundCoinSelection::default().single_random_draw(
            vec![],
            utxos,
            0,
            target_amount as i64,
            FEE_AMOUNT,
        );

        assert!(result.selected_amount() > target_amount);
        assert_eq!(result.fee_amount, (50 + result.selected.len() * 68) as u64);
    }
}
