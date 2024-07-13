//! HWI Signer
//!
//! This crate contains HWISigner, an implementation of a [`TransactionSigner`] to be
//! used with hardware wallets.
//! ```no_run
//! # use bdk_wallet::bitcoin::Network;
//! # use bdk_wallet::signer::SignerOrdering;
//! # use bdk_hwi::HWISigner;
//! # use bdk_wallet::{KeychainKind, SignOptions, Wallet};
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
//! # let mut wallet = Wallet::new(
//! #     "",
//! #     Some(""),
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
//! [`TransactionSigner`]: bdk_wallet::wallet::signer::TransactionSigner

mod signer;
pub use signer::*;
