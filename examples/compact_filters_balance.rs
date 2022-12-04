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

use bdk::blockchain::compact_filters::*;
use bdk::database::MemoryDatabase;
use bdk::*;
use bitcoin::*;
use blockchain::compact_filters::CompactFiltersBlockchain;
use blockchain::compact_filters::CompactFiltersError;
use log::info;
use std::sync::Arc;

pub mod utils;
use crate::utils::tor::{start_tor, use_tor};

/// This will return wallet balance using compact filters
/// Requires a synced local bitcoin node 0.21 running on testnet with blockfilterindex=1 and peerblockfilters=1
fn main() -> Result<(), CompactFiltersError> {
    env_logger::init();
    info!("start");

    let tor_addrs = if use_tor() {
        Some(start_tor(Some(18333)))
    } else {
        None
    };

    let num_threads = 4;
    let mempool = Arc::new(Mempool::default());
    let peers = (0..num_threads)
        .map(|_| {
            if use_tor() {
                let addr = &tor_addrs.as_ref().unwrap().hidden_service.as_ref().unwrap();
                let proxy = &tor_addrs.as_ref().unwrap().socks;
                info!("conecting to {} via {}", &addr, &proxy);
                Peer::connect_proxy(
                    addr.as_str(),
                    &proxy,
                    None,
                    Arc::clone(&mempool),
                    Network::Testnet,
                )
            } else {
                Peer::connect("localhost:18333", Arc::clone(&mempool), Network::Testnet)
            }
        })
        .collect::<Result<_, _>>()?;
    let blockchain = CompactFiltersBlockchain::new(peers, "./wallet-filters", Some(500_000))?;
    info!("done {:?}", blockchain);
    let descriptor = "wpkh(tpubD6NzVbkrYhZ4X2yy78HWrr1M9NT8dKeWfzNiQqDdMqqa9UmmGztGGz6TaLFGsLfdft5iu32gxq1T4eMNxExNNWzVCpf9Y6JZi5TnqoC9wJq/*)";

    let database = MemoryDatabase::default();
    let wallet = Arc::new(Wallet::new(descriptor, None, Network::Testnet, database).unwrap());
    wallet.sync(&blockchain, SyncOptions::default()).unwrap();
    info!("balance: {}", wallet.get_balance()?);
    Ok(())
}
