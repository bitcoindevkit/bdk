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

use crate::bdk_core::{self, BnbLimit};
use crate::{database::Database, WeightedUtxo};
use crate::{error::Error, Utxo};

use rand::seq::SliceRandom;
#[cfg(not(test))]
use rand::thread_rng;
#[cfg(test)]
use rand::{rngs::StdRng, SeedableRng};
use std::collections::HashMap;
use std::fmt::Debug;

/// Default coin selection algorithm used by [`TxBuilder`](super::tx_builder::TxBuilder) if not
/// overridden
#[cfg(not(test))]
pub type DefaultCoinSelectionAlgorithm = BranchAndBoundCoinSelection;
#[cfg(test)]
pub type DefaultCoinSelectionAlgorithm = LargestFirstCoinSelection; // make the tests more predictable

// Base weight of a Txin, not counting the weight needed for satisfying it.
// prev_txid (32 bytes) + prev_vout (4 bytes) + sequence (4 bytes)
// pub(crate) const TXIN_BASE_WEIGHT: usize = (32 + 4 + 4) * 4;

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
pub trait CoinSelectionAlgorithm: std::fmt::Debug {
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
        raw_candidates: &[WeightedUtxo],
        selector: bdk_core::CoinSelector,
    ) -> Result<bdk_core::Selection, Error>;
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
        _raw_candidates: &[WeightedUtxo],
        mut selector: bdk_core::CoinSelector,
    ) -> Result<bdk_core::Selection, Error> {
        log::debug!(
            "target_amount = `{}`, fee_rate = `{:?}`",
            selector.opts.target_value.unwrap_or(0),
            selector.opts.target_feerate,
        );

        let pool = {
            let mut pool = selector.unselected().collect::<Vec<_>>();
            pool.sort_unstable_by_key(|(_, candidate)| candidate.value);
            pool.reverse();
            println!("pool: {:?}", pool);
            pool
        };

        let mut selection = selector.finish();

        for (index, _candidate) in pool {
            if selection.is_ok() {
                return selection.map_err(Error::from);
            }

            selector.select(index);
            println!("selected: index={}, value={}", index, _candidate.value);
            selection = selector.finish();
        }

        selection.map_err(Error::from)
    }
}

/// OldestFirstCoinSelection always picks the utxo with the smallest blockheight to add to the selected coins next
///
/// This coin selection algorithm sorts the available UTXOs by blockheight and then picks them starting
/// from the oldest ones until the required amount is reached.
#[derive(Clone, Copy)]
pub struct OldestFirstCoinSelection<'d, D> {
    database: &'d D,
}

impl<'d, D: Database> OldestFirstCoinSelection<'d, D> {
    /// Creates a new instance of [`OldestFirstCoinSelection`].
    pub fn new(database: &'d D) -> Self {
        Self { database }
    }
}

impl<'d, D> Debug for OldestFirstCoinSelection<'d, D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("OldestFirstCoinSelection").finish()
    }
}

impl<'d, D: Database> CoinSelectionAlgorithm for OldestFirstCoinSelection<'d, D> {
    fn coin_select(
        &self,
        raw_candidates: &[WeightedUtxo],
        mut selector: bdk_core::CoinSelector,
    ) -> Result<bdk_core::Selection, Error> {
        // query db and create a blockheight lookup table
        let blockheights = raw_candidates
            .iter()
            .map(|wu| wu.utxo.outpoint().txid)
            // fold is used so we can skip db query for txid that already exist in hashmap acc
            .fold(Ok(HashMap::new()), |bh_result_acc, txid| {
                bh_result_acc.and_then(|mut bh_acc| {
                    if bh_acc.contains_key(&txid) {
                        Ok(bh_acc)
                    } else {
                        self.database.get_tx(&txid, false).map(|details| {
                            bh_acc.insert(
                                txid,
                                details.and_then(|d| d.confirmation_time.map(|ct| ct.height)),
                            );
                            bh_acc
                        })
                    }
                })
            })?;

        let pool = {
            let mut pool = selector.unselected().collect::<Vec<_>>();
            pool.sort_unstable_by_key(|&(index, _candidate)| {
                let raw = &raw_candidates[index];
                let txid = &raw.utxo.outpoint().txid;
                match blockheights.get(txid) {
                    Some(Some(height)) => height,
                    _ => &u32::MAX,
                }
            });
            pool
        };

        let mut selection = selector.finish();

        for (index, _candidate) in pool {
            if selection.is_ok() {
                return selection.map_err(Error::from);
            }

            selector.select(index);
            selection = selector.finish();
        }

        selection.map_err(Error::from)
    }
}

/// Branch and bound coin selection
///
/// Code adapted from Bitcoin Core's implementation and from Mark Erhardt Master's Thesis: <http://murch.one/wp-content/uploads/2016/11/erhardt2016coinselection.pdf>
#[derive(Debug)]
pub struct BranchAndBoundCoinSelection {
    limit: BnbLimit,
}

impl Default for BranchAndBoundCoinSelection {
    fn default() -> Self {
        Self {
            limit: BnbLimit::Rounds(100_000),
        }
    }
}

impl BranchAndBoundCoinSelection {
    /// Create new instance with target size for change output
    pub fn new<L: Into<BnbLimit>>(limit: L) -> Self {
        Self {
            limit: limit.into(),
        }
    }
}

impl CoinSelectionAlgorithm for BranchAndBoundCoinSelection {
    fn coin_select(
        &self,
        _raw_candidates: &[WeightedUtxo],
        mut selector: bdk_core::CoinSelector,
    ) -> Result<bdk_core::Selection, Error> {
        if let Some(final_selector) = bdk_core::coin_select_bnb(self.limit, selector.clone()) {
            return final_selector.finish().map_err(Error::from);
        }

        // FALLBACK: single random draw
        let pool = {
            let mut pool = selector
                .unselected()
                .filter(|(_, c)| c.effective_value(selector.opts.target_feerate) > 0)
                .collect::<Vec<_>>();

            #[cfg(not(test))]
            pool.shuffle(&mut thread_rng());
            #[cfg(test)]
            pool.shuffle(&mut StdRng::from_seed([0; 32]));

            pool
        };

        let mut selection = selector.finish();

        for (index, _candidate) in pool {
            if selection.is_ok() {
                return selection.map_err(Error::from);
            }

            selector.select(index);
            selection = selector.finish();
        }

        selection.map_err(Error::from)
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use bitcoin::{OutPoint, Script, TxOut};

    use super::*;
    use crate::bdk_core::{CoinSelector, CoinSelectorOpt, WeightedValue};
    use crate::database::{BatchOperations, MemoryDatabase};
    // use crate::database::{BatchOperations, MemoryDatabase};
    use crate::types::*;
    // use crate::wallet::WeightUnits;

    use rand::rngs::StdRng;
    use rand::seq::SliceRandom;
    use rand::Rng;

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

    fn get_test_utxos() -> (Vec<WeightedUtxo>, Vec<bdk_core::WeightedValue>) {
        let utxos = vec![
            utxo(100_000, 0),
            utxo(FEE_AMOUNT as u64 - 40, 1),
            utxo(200_000, 2),
        ];

        let candidates = utxos
            .iter()
            .map(|utxo| {
                bdk_core::WeightedValue::new(
                    utxo.utxo.txout().value,
                    utxo.satisfaction_weight as u32,
                    utxo.utxo.txout().script_pubkey.is_witness_program(),
                )
            })
            .collect::<Vec<_>>();

        (utxos, candidates)
    }

    fn get_test_candidates(utxos: &[WeightedUtxo]) -> Vec<WeightedValue> {
        utxos
            .iter()
            .map(|utxo| {
                bdk_core::WeightedValue::new(
                    utxo.utxo.txout().value,
                    utxo.satisfaction_weight as u32,
                    utxo.utxo.txout().script_pubkey.is_witness_program(),
                )
            })
            .collect::<Vec<_>>()
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

    fn new_opts(target_value: u64, feerate: f32, drain_script: Script) -> CoinSelectorOpt {
        CoinSelectorOpt {
            target_feerate: feerate,
            ..CoinSelectorOpt::fund_outputs(
                &[TxOut {
                    value: target_value,
                    script_pubkey: Script::default(),
                }],
                &TxOut {
                    value: 0,
                    script_pubkey: drain_script,
                },
                0,
            )
        }
    }

    #[test]
    fn test_largest_first_coin_selection_success() {
        let (utxos, candidates) = get_test_utxos();
        let target_amount = 250_000 + FEE_AMOUNT;

        let opts = new_opts(target_amount, 0.25, Script::default());
        let selector = bdk_core::CoinSelector::new(&candidates, &opts);

        let result = LargestFirstCoinSelection::default()
            .coin_select(&utxos, selector)
            .unwrap();
        let (strategy_kind, strategy) = result.best_strategy();
        println!("strategy_kind: {}", strategy_kind);

        assert_eq!(result.selected.len(), 2);
        assert_eq!(strategy.fee, 164);
    }

    #[test]
    fn test_largest_first_coin_selection_use_all() {
        let (utxos, candidates) = get_test_utxos();
        let target_amount = 20_000 + FEE_AMOUNT;

        let opts = new_opts(target_amount, 0.25, Script::default());
        let mut selector = bdk_core::CoinSelector::new(&candidates, &opts);
        selector.select_all();

        let result = LargestFirstCoinSelection::default()
            .coin_select(&utxos, selector)
            .unwrap();
        let (_, strategy) = result.best_strategy();

        assert_eq!(result.selected.len(), 3);
        // assert_eq!(result.selected_amount(), 300_010);
        assert_eq!(strategy.fee, 232);
    }

    #[test]
    fn test_largest_first_coin_selection_use_only_necessary() {
        let (utxos, candidates) = get_test_utxos();
        let drain_script = Script::default();
        let target_amount = 20_000 + FEE_AMOUNT;

        let opts = new_opts(target_amount, 0.25, drain_script);
        let selector = CoinSelector::new(&candidates, &opts);

        let result = LargestFirstCoinSelection::default()
            .coin_select(&utxos, selector)
            .unwrap();
        let (_, strategy) = result.best_strategy();

        assert_eq!(result.selected.len(), 1);
        // assert_eq!(result.selected_amount(), 200_000);
        assert_eq!(strategy.fee, 96);
    }

    #[test]
    fn test_largest_first_coin_selection_insufficient_funds() {
        let (utxos, candidates) = get_test_utxos();
        let drain_script = Script::default();
        let target_amount = 500_000 + FEE_AMOUNT;

        let opts = new_opts(target_amount, 0.25, drain_script);
        let selector = CoinSelector::new(&candidates, &opts);

        let err = LargestFirstCoinSelection::default()
            .coin_select(&utxos, selector)
            .expect_err("should fail");
        assert!(matches!(err, Error::Generic(s) if s.contains("insufficient coins")));
    }

    #[test]
    fn test_largest_first_coin_selection_insufficient_funds_high_fees() {
        let (utxos, candidates) = get_test_utxos();
        let drain_script = Script::default();
        let target_amount = 250_000 + FEE_AMOUNT;

        let opts = new_opts(target_amount, 1000.0 / 4.0, drain_script);
        let selector = CoinSelector::new(&candidates, &opts);

        let err = LargestFirstCoinSelection::default()
            .coin_select(&utxos, selector)
            .expect_err("should fail");
        assert!(matches!(err, Error::Generic(s) if s.contains("insufficient coins")));
    }

    #[test]
    fn test_oldest_first_coin_selection_success() {
        let mut database = MemoryDatabase::default();
        let utxos = setup_database_and_get_oldest_first_test_utxos(&mut database);
        let candidates = get_test_candidates(&utxos);
        let drain_script = Script::default();
        let target_amount = 180_000 + FEE_AMOUNT;

        let opts = new_opts(target_amount, 0.25, drain_script);
        let selector = CoinSelector::new(&candidates, &opts);

        let result = OldestFirstCoinSelection::new(&database)
            .coin_select(&utxos, selector)
            .unwrap();
        let (_, strategy) = result.best_strategy();

        assert_eq!(result.selected.len(), 2);
        // assert_eq!(result.selected_amount(), 200_000);
        assert!(strategy.feerate() >= 0.25);
    }

    #[test]
    fn test_oldest_first_coin_selection_utxo_not_in_db_will_be_selected_last() {
        // ensure utxos are from different tx
        let utxo1 = utxo(120_000, 1);
        let utxo2 = utxo(80_000, 2);
        let utxo3 = utxo(300_000, 3);
        let drain_script = Script::default();

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

        let target_amount = 180_000 + FEE_AMOUNT;

        let opts = new_opts(target_amount, 0.25, drain_script);
        let utxos = vec![utxo3, utxo1, utxo2];
        let candidates = get_test_candidates(&utxos);
        let selector = CoinSelector::new(&candidates, &opts);

        let result = OldestFirstCoinSelection::new(&database)
            .coin_select(&utxos, selector)
            .unwrap();
        let (_, strategy) = result.best_strategy();

        assert_eq!(result.selected.len(), 2);
        let selected_sum = result
            .apply_selection(&utxos)
            .map(|u| u.utxo.txout().value)
            .sum::<u64>();
        assert_eq!(selected_sum, 200_000);
        assert!(strategy.feerate() >= 0.25);
    }

    // #[test]
    // fn test_oldest_first_coin_selection_use_all() {
    //     let mut database = MemoryDatabase::default();
    //     let utxos = setup_database_and_get_oldest_first_test_utxos(&mut database);
    //     let drain_script = Script::default();
    //     let target_amount = 20_000 + FEE_AMOUNT;

    //     let result = OldestFirstCoinSelection::new(&database)
    //         .coin_select(
    //             utxos,
    //             vec![],
    //             FeeRate::from_sat_per_vb(1.0),
    //             target_amount,
    //             &drain_script,
    //         )
    //         .unwrap();

    //     assert_eq!(result.selected.len(), 3);
    //     assert_eq!(result.selected_amount(), 500_000);
    //     assert_eq!(result.fee_amount, 204);
    // }

    // #[test]
    // fn test_oldest_first_coin_selection_use_only_necessary() {
    //     let mut database = MemoryDatabase::default();
    //     let utxos = setup_database_and_get_oldest_first_test_utxos(&mut database);
    //     let drain_script = Script::default();
    //     let target_amount = 20_000 + FEE_AMOUNT;

    //     let result = OldestFirstCoinSelection::new(&database)
    //         .coin_select(
    //             vec![],
    //             utxos,
    //             FeeRate::from_sat_per_vb(1.0),
    //             target_amount,
    //             &drain_script,
    //         )
    //         .unwrap();

    //     assert_eq!(result.selected.len(), 1);
    //     assert_eq!(result.selected_amount(), 120_000);
    //     assert_eq!(result.fee_amount, 68);
    // }

    // #[test]
    // #[should_panic(expected = "InsufficientFunds")]
    // fn test_oldest_first_coin_selection_insufficient_funds() {
    //     let mut database = MemoryDatabase::default();
    //     let utxos = setup_database_and_get_oldest_first_test_utxos(&mut database);
    //     let drain_script = Script::default();
    //     let target_amount = 600_000 + FEE_AMOUNT;

    //     OldestFirstCoinSelection::new(&database)
    //         .coin_select(
    //             vec![],
    //             utxos,
    //             FeeRate::from_sat_per_vb(1.0),
    //             target_amount,
    //             &drain_script,
    //         )
    //         .unwrap();
    // }

    // #[test]
    // #[should_panic(expected = "InsufficientFunds")]
    // fn test_oldest_first_coin_selection_insufficient_funds_high_fees() {
    //     let mut database = MemoryDatabase::default();
    //     let utxos = setup_database_and_get_oldest_first_test_utxos(&mut database);

    //     let target_amount: u64 = utxos.iter().map(|wu| wu.utxo.txout().value).sum::<u64>() - 50;
    //     let drain_script = Script::default();

    //     OldestFirstCoinSelection::new(&database)
    //         .coin_select(
    //             vec![],
    //             utxos,
    //             FeeRate::from_sat_per_vb(1000.0),
    //             target_amount,
    //             &drain_script,
    //         )
    //         .unwrap();
    // }

    // #[test]
    // fn test_bnb_coin_selection_success() {
    //     // In this case bnb won't find a suitable match and single random draw will
    //     // select three outputs
    //     let utxos = generate_same_value_utxos(100_000, 20);

    //     let drain_script = Script::default();

    //     let target_amount = 250_000 + FEE_AMOUNT;

    //     let result = BranchAndBoundCoinSelection::default()
    //         .coin_select(
    //             vec![],
    //             utxos,
    //             FeeRate::from_sat_per_vb(1.0),
    //             target_amount,
    //             &drain_script,
    //         )
    //         .unwrap();

    //     assert_eq!(result.selected.len(), 3);
    //     assert_eq!(result.selected_amount(), 300_000);
    //     assert_eq!(result.fee_amount, 204);
    // }

    // #[test]
    // fn test_bnb_coin_selection_required_are_enough() {
    //     let utxos = get_test_utxos();
    //     let drain_script = Script::default();
    //     let target_amount = 20_000 + FEE_AMOUNT;

    //     let result = BranchAndBoundCoinSelection::default()
    //         .coin_select(
    //             utxos.clone(),
    //             utxos,
    //             FeeRate::from_sat_per_vb(1.0),
    //             target_amount,
    //             &drain_script,
    //         )
    //         .unwrap();

    //     assert_eq!(result.selected.len(), 3);
    //     assert_eq!(result.selected_amount(), 300_010);
    //     assert_eq!(result.fee_amount, 204);
    // }

    // #[test]
    // fn test_bnb_coin_selection_optional_are_enough() {
    //     let utxos = get_test_utxos();
    //     let drain_script = Script::default();
    //     let target_amount = 299756 + FEE_AMOUNT;

    //     let result = BranchAndBoundCoinSelection::default()
    //         .coin_select(
    //             vec![],
    //             utxos,
    //             FeeRate::from_sat_per_vb(1.0),
    //             target_amount,
    //             &drain_script,
    //         )
    //         .unwrap();

    //     assert_eq!(result.selected.len(), 2);
    //     assert_eq!(result.selected_amount(), 300000);
    //     assert_eq!(result.fee_amount, 136);
    // }

    // #[test]
    // fn test_bnb_coin_selection_required_not_enough() {
    //     let utxos = get_test_utxos();

    //     let required = vec![utxos[0].clone()];
    //     let mut optional = utxos[1..].to_vec();
    //     optional.push(utxo(500_000, 3));

    //     // Defensive assertions, for sanity and in case someone changes the test utxos vector.
    //     let amount: u64 = required.iter().map(|u| u.utxo.txout().value).sum();
    //     assert_eq!(amount, 100_000);
    //     let amount: u64 = optional.iter().map(|u| u.utxo.txout().value).sum();
    //     assert!(amount > 150_000);
    //     let drain_script = Script::default();

    //     let target_amount = 150_000 + FEE_AMOUNT;

    //     let result = BranchAndBoundCoinSelection::default()
    //         .coin_select(
    //             required,
    //             optional,
    //             FeeRate::from_sat_per_vb(1.0),
    //             target_amount,
    //             &drain_script,
    //         )
    //         .unwrap();

    //     assert_eq!(result.selected.len(), 2);
    //     assert_eq!(result.selected_amount(), 300_000);
    //     assert_eq!(result.fee_amount, 136);
    // }

    // #[test]
    // #[should_panic(expected = "InsufficientFunds")]
    // fn test_bnb_coin_selection_insufficient_funds() {
    //     let utxos = get_test_utxos();
    //     let drain_script = Script::default();
    //     let target_amount = 500_000 + FEE_AMOUNT;

    //     BranchAndBoundCoinSelection::default()
    //         .coin_select(
    //             vec![],
    //             utxos,
    //             FeeRate::from_sat_per_vb(1.0),
    //             target_amount,
    //             &drain_script,
    //         )
    //         .unwrap();
    // }

    // #[test]
    // #[should_panic(expected = "InsufficientFunds")]
    // fn test_bnb_coin_selection_insufficient_funds_high_fees() {
    //     let utxos = get_test_utxos();
    //     let drain_script = Script::default();
    //     let target_amount = 250_000 + FEE_AMOUNT;

    //     BranchAndBoundCoinSelection::default()
    //         .coin_select(
    //             vec![],
    //             utxos,
    //             FeeRate::from_sat_per_vb(1000.0),
    //             target_amount,
    //             &drain_script,
    //         )
    //         .unwrap();
    // }

    // #[test]
    // fn test_bnb_coin_selection_check_fee_rate() {
    //     let utxos = get_test_utxos();
    //     let drain_script = Script::default();
    //     let target_amount = 99932; // first utxo's effective value

    //     let result = BranchAndBoundCoinSelection::new(0)
    //         .coin_select(
    //             vec![],
    //             utxos,
    //             FeeRate::from_sat_per_vb(1.0),
    //             target_amount,
    //             &drain_script,
    //         )
    //         .unwrap();

    //     assert_eq!(result.selected.len(), 1);
    //     assert_eq!(result.selected_amount(), 100_000);
    //     let input_size = (TXIN_BASE_WEIGHT + P2WPKH_SATISFACTION_SIZE).vbytes();
    //     // the final fee rate should be exactly the same as the fee rate given
    //     assert!((1.0 - (result.fee_amount as f32 / input_size as f32)).abs() < f32::EPSILON);
    // }

    // #[test]
    // fn test_bnb_coin_selection_exact_match() {
    //     let seed = [0; 32];
    //     let mut rng: StdRng = SeedableRng::from_seed(seed);

    //     for _i in 0..200 {
    //         let mut optional_utxos = generate_random_utxos(&mut rng, 16);
    //         let target_amount = sum_random_utxos(&mut rng, &mut optional_utxos);
    //         let drain_script = Script::default();
    //         let result = BranchAndBoundCoinSelection::new(0)
    //             .coin_select(
    //                 vec![],
    //                 optional_utxos,
    //                 FeeRate::from_sat_per_vb(0.0),
    //                 target_amount,
    //                 &drain_script,
    //             )
    //             .unwrap();
    //         assert_eq!(result.selected_amount(), target_amount);
    //     }
    // }

    // #[test]
    // #[should_panic(expected = "BnBNoExactMatch")]
    // fn test_bnb_function_no_exact_match() {
    //     let fee_rate = FeeRate::from_sat_per_vb(10.0);
    //     let utxos: Vec<OutputGroup> = get_test_utxos()
    //         .into_iter()
    //         .map(|u| OutputGroup::new(u, fee_rate))
    //         .collect();

    //     let curr_available_value = utxos.iter().fold(0, |acc, x| acc + x.effective_value);

    //     let size_of_change = 31;
    //     let cost_of_change = size_of_change as f32 * fee_rate.as_sat_per_vb();

    //     let drain_script = Script::default();
    //     let target_amount = 20_000 + FEE_AMOUNT;
    //     BranchAndBoundCoinSelection::new(size_of_change)
    //         .bnb(
    //             vec![],
    //             utxos,
    //             0,
    //             curr_available_value,
    //             target_amount as i64,
    //             cost_of_change,
    //             &drain_script,
    //             fee_rate,
    //         )
    //         .unwrap();
    // }

    // #[test]
    // #[should_panic(expected = "BnBTotalTriesExceeded")]
    // fn test_bnb_function_tries_exceeded() {
    //     let fee_rate = FeeRate::from_sat_per_vb(10.0);
    //     let utxos: Vec<OutputGroup> = generate_same_value_utxos(100_000, 100_000)
    //         .into_iter()
    //         .map(|u| OutputGroup::new(u, fee_rate))
    //         .collect();

    //     let curr_available_value = utxos.iter().fold(0, |acc, x| acc + x.effective_value);

    //     let size_of_change = 31;
    //     let cost_of_change = size_of_change as f32 * fee_rate.as_sat_per_vb();
    //     let target_amount = 20_000 + FEE_AMOUNT;

    //     let drain_script = Script::default();

    //     BranchAndBoundCoinSelection::new(size_of_change)
    //         .bnb(
    //             vec![],
    //             utxos,
    //             0,
    //             curr_available_value,
    //             target_amount as i64,
    //             cost_of_change,
    //             &drain_script,
    //             fee_rate,
    //         )
    //         .unwrap();
    // }

    // // The match won't be exact but still in the range
    // #[test]
    // fn test_bnb_function_almost_exact_match_with_fees() {
    //     let fee_rate = FeeRate::from_sat_per_vb(1.0);
    //     let size_of_change = 31;
    //     let cost_of_change = size_of_change as f32 * fee_rate.as_sat_per_vb();

    //     let utxos: Vec<_> = generate_same_value_utxos(50_000, 10)
    //         .into_iter()
    //         .map(|u| OutputGroup::new(u, fee_rate))
    //         .collect();

    //     let curr_value = 0;

    //     let curr_available_value = utxos.iter().fold(0, |acc, x| acc + x.effective_value);

    //     // 2*(value of 1 utxo)  - 2*(1 utxo fees with 1.0sat/vbyte fee rate) -
    //     // cost_of_change + 5.
    //     let target_amount = 2 * 50_000 - 2 * 67 - cost_of_change.ceil() as i64 + 5;

    //     let drain_script = Script::default();

    //     let result = BranchAndBoundCoinSelection::new(size_of_change)
    //         .bnb(
    //             vec![],
    //             utxos,
    //             curr_value,
    //             curr_available_value,
    //             target_amount,
    //             cost_of_change,
    //             &drain_script,
    //             fee_rate,
    //         )
    //         .unwrap();
    //     assert_eq!(result.selected_amount(), 100_000);
    //     assert_eq!(result.fee_amount, 136);
    // }

    // // TODO: bnb() function should be optimized, and this test should be done with more utxos
    // #[test]
    // fn test_bnb_function_exact_match_more_utxos() {
    //     let seed = [0; 32];
    //     let mut rng: StdRng = SeedableRng::from_seed(seed);
    //     let fee_rate = FeeRate::from_sat_per_vb(0.0);

    //     for _ in 0..200 {
    //         let optional_utxos: Vec<_> = generate_random_utxos(&mut rng, 40)
    //             .into_iter()
    //             .map(|u| OutputGroup::new(u, fee_rate))
    //             .collect();

    //         let curr_value = 0;

    //         let curr_available_value = optional_utxos
    //             .iter()
    //             .fold(0, |acc, x| acc + x.effective_value);

    //         let target_amount =
    //             optional_utxos[3].effective_value + optional_utxos[23].effective_value;

    //         let drain_script = Script::default();

    //         let result = BranchAndBoundCoinSelection::new(0)
    //             .bnb(
    //                 vec![],
    //                 optional_utxos,
    //                 curr_value,
    //                 curr_available_value,
    //                 target_amount,
    //                 0.0,
    //                 &drain_script,
    //                 fee_rate,
    //             )
    //             .unwrap();
    //         assert_eq!(result.selected_amount(), target_amount as u64);
    //     }
    // }

    // #[test]
    // fn test_single_random_draw_function_success() {
    //     let seed = [0; 32];
    //     let mut rng: StdRng = SeedableRng::from_seed(seed);
    //     let mut utxos = generate_random_utxos(&mut rng, 300);
    //     let target_amount = sum_random_utxos(&mut rng, &mut utxos) + FEE_AMOUNT;

    //     let fee_rate = FeeRate::from_sat_per_vb(1.0);
    //     let utxos: Vec<OutputGroup> = utxos
    //         .into_iter()
    //         .map(|u| OutputGroup::new(u, fee_rate))
    //         .collect();

    //     let drain_script = Script::default();

    //     let result = BranchAndBoundCoinSelection::default().single_random_draw(
    //         vec![],
    //         utxos,
    //         0,
    //         target_amount as i64,
    //         &drain_script,
    //         fee_rate,
    //     );

    //     assert!(result.selected_amount() > target_amount);
    //     assert_eq!(result.fee_amount, (result.selected.len() * 68) as u64);
    // }

    // #[test]
    // fn test_bnb_exclude_negative_effective_value() {
    //     let utxos = get_test_utxos();
    //     let drain_script = Script::default();

    //     let err = BranchAndBoundCoinSelection::default()
    //         .coin_select(
    //             vec![],
    //             utxos,
    //             FeeRate::from_sat_per_vb(10.0),
    //             500_000,
    //             &drain_script,
    //         )
    //         .unwrap_err();

    //     assert!(matches!(
    //         err,
    //         Error::InsufficientFunds {
    //             available: 300_000,
    //             ..
    //         }
    //     ));
    // }

    // #[test]
    // fn test_bnb_include_negative_effective_value_when_required() {
    //     let utxos = get_test_utxos();
    //     let drain_script = Script::default();

    //     let (required, optional) = utxos
    //         .into_iter()
    //         .partition(|u| matches!(u, WeightedUtxo { utxo, .. } if utxo.txout().value < 1000));

    //     let err = BranchAndBoundCoinSelection::default()
    //         .coin_select(
    //             required,
    //             optional,
    //             FeeRate::from_sat_per_vb(10.0),
    //             500_000,
    //             &drain_script,
    //         )
    //         .unwrap_err();

    //     assert!(matches!(
    //         err,
    //         Error::InsufficientFunds {
    //             available: 300_010,
    //             ..
    //         }
    //     ));
    // }

    // #[test]
    // fn test_bnb_sum_of_effective_value_negative() {
    //     let utxos = get_test_utxos();
    //     let drain_script = Script::default();

    //     let err = BranchAndBoundCoinSelection::default()
    //         .coin_select(
    //             utxos,
    //             vec![],
    //             FeeRate::from_sat_per_vb(10_000.0),
    //             500_000,
    //             &drain_script,
    //         )
    //         .unwrap_err();

    //     assert!(matches!(
    //         err,
    //         Error::InsufficientFunds {
    //             available: 300_010,
    //             ..
    //         }
    //     ));
    // }
}
