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
use anyhow::Result;
use bitcoin::Network;

use bdk::blockchain::{ElectrumBlockchain, NoopProgress};
use bdk::database::MemoryDatabase;
use bdk::electrum_client::Client;
use bdk::Wallet;

const ELECTRUM_URL: &str = "ssl://electrum.blockstream.info:60002";
const DESC: &str = "wpkh(tprv8ZgxMBicQKsPdT8dRdm7Ae7ZxLTCKNPaZwt7aBWNRyxUCMvY7xhjRG4iBLerk2FTBv6zrzMMw18M3LwJEvn9QhbzsiYJefwUmzcUXcAPDmt/0/*)";
const CHANGE_DESC: &str = "wpkh(tprv8ZgxMBicQKsPdT8dRdm7Ae7ZxLTCKNPaZwt7aBWNRyxUCMvY7xhjRG4iBLerk2FTBv6zrzMMw18M3LwJEvn9QhbzsiYJefwUmzcUXcAPDmt/1/*)";

/// This demonstrates simple initialisation of an online wallet.
fn main() -> Result<()> {
    let wallet = wallet()?;
    wallet.sync(NoopProgress, None)?;
    Ok(())
}

fn wallet() -> Result<Wallet<ElectrumBlockchain, MemoryDatabase>> {
    let client = Client::new(ELECTRUM_URL)?;
    let wallet = Wallet::new(
        DESC,
        Some(CHANGE_DESC),
        Network::Testnet,
        MemoryDatabase::default(),
        ElectrumBlockchain::from(client),
    )?;

    Ok(wallet)
}
