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

//! HWI Signer
//!
//! This module contains HWISigner, an implementation of a [TransactionSigner] to be
//! used with hardware wallets.
//! ```no_run
//! # use bdk::bitcoin::Network;
//! # use bdk::signer::SignerOrdering;
//! # use bdk::wallet::hardwaresigner::HWISigner;
//! # use bdk::wallet::AddressIndex::New;
//! # use bdk::{FeeRate, KeychainKind, SignOptions, Wallet};
//! # use hwi::HWIClient;
//! # use std::sync::Arc;
//! #
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut devices = HWIClient::enumerate()?;
//! if devices.is_empty() {
//!     panic!("No devices found!");
//! }
//! let first_device = devices.remove(0)?;
//! let custom_signer = HWISigner::from_device(&first_device, Network::Testnet.into())?;
//!
//! # let mut wallet = Wallet::new_no_persist(
//! #     "",
//! #     None,
//! #     Network::Testnet,
//! # )?;
//! #
//! // Adding the hardware signer to the BDK wallet
//! wallet.add_signer(
//!     KeychainKind::External,
//!     SignerOrdering(200),
//!     Arc::new(custom_signer),
//! );
//!
//! # Ok(())
//! # }
//! ```

use bitcoin::bip32::Fingerprint;
use bitcoin::psbt::Psbt;
use bitcoin::secp256k1::{All, Secp256k1};

use hwi::error::Error;
use hwi::types::{HWIChain, HWIDevice};
use hwi::HWIClient;

use crate::signer::{SignerCommon, SignerError, SignerId, TransactionSigner};

#[derive(Debug)]
/// Custom signer for Hardware Wallets
///
/// This ignores `sign_options` and leaves the decisions up to the hardware wallet.
pub struct HWISigner {
    fingerprint: Fingerprint,
    client: HWIClient,
}

impl HWISigner {
    /// Create a instance from the specified device and chain
    pub fn from_device(device: &HWIDevice, chain: HWIChain) -> Result<HWISigner, Error> {
        let client = HWIClient::get_client(device, false, chain)?;
        Ok(HWISigner {
            fingerprint: device.fingerprint,
            client,
        })
    }
}

impl SignerCommon for HWISigner {
    fn id(&self, _secp: &Secp256k1<All>) -> SignerId {
        SignerId::Fingerprint(self.fingerprint)
    }
}

/// This implementation ignores `sign_options`
impl TransactionSigner for HWISigner {
    fn sign_transaction(
        &self,
        psbt: &mut Psbt,
        _sign_options: &crate::SignOptions,
        _secp: &crate::wallet::utils::SecpCtx,
    ) -> Result<(), SignerError> {
        psbt.combine(self.client.sign_tx(psbt)?.psbt)
            .expect("Failed to combine HW signed psbt with passed PSBT");
        Ok(())
    }
}
