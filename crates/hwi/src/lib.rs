//! HWI Signer
//!
//! This crate contains HWISigner, an implementation of a [`TransactionSigner`] to be
//! used with hardware wallets.
//! ```no_run
//! # use bdk::bitcoin::Network;
//! # use bdk::signer::SignerOrdering;
//! # use bdk_hwi::HWISigner;
//! # use bdk::{KeychainKind, SignOptions, Wallet};
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
//!
//! [`TransactionSigner`]: bdk::wallet::signer::TransactionSigner

mod signer;
pub use signer::*;
