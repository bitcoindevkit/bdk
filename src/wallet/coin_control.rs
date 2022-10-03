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

//! Coin control
//!
//! This module defines how coins are to be sorted and/or grouped before coin selection.

use std::collections::HashMap;

use bitcoin::OutPoint;

use crate::TransactionDetails;

use super::COINBASE_MATURITY;

/// Parameters to determine which coins/outputs to filter.
pub struct CoinFilterParams {
    /// Outpoints to manually keep (`true`) or skip (`false`). This overrides all other parameters.
    pub manual: HashMap<OutPoint, bool>,

    /// Whether we should filter out unconfirmed transactions.
    /// TODO: Use minimum confirmations instead.
    pub filter_unconfirmed: bool,

    /// Whether we should filter out immature coinbase outputs.
    /// Coinbase transaction outputs need to be at least 100 blocks deep before being spendable.
    /// If this is set, but `current_height == None`, all coinbase outputs will be filtered out.
    pub filter_immature_coinbase: bool,

    /// Current block height.
    pub current_height: Option<u32>,
}

impl CoinFilterParams {
    /// Returns true if coin is to be kept, false if coin is to be filtered out.
    pub(crate) fn keep(&self, tx: &TransactionDetails, outpoint: &OutPoint) -> bool {
        let raw_tx = tx.transaction.as_ref().expect("failed to obtain raw tx");

        if let Some(&keep) = self.manual.get(outpoint) {
            return keep;
        }

        if self.filter_unconfirmed && tx.confirmation_time.is_none() {
            return false;
        }

        // https://github.com/bitcoin/bitcoin/blob/c5e67be03bb06a5d7885c55db1f016fbf2333fe3/src/validation.cpp#L373-L375
        if self.filter_immature_coinbase
            && raw_tx.is_coin_base()
            && !matches!((self.current_height, &tx.confirmation_time), (Some(tip), Some(conf)) if tip.saturating_sub(conf.height) >= COINBASE_MATURITY)
        {
            return false;
        }

        true
    }
}
