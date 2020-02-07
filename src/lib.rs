pub extern crate bitcoin;
extern crate log;
pub extern crate miniscript;
extern crate serde;
#[macro_use]
extern crate serde_json;

#[cfg(test)]
#[macro_use]
extern crate lazy_static;

#[cfg(any(feature = "electrum", feature = "default"))]
pub extern crate electrum_client;
#[cfg(any(feature = "electrum", feature = "default"))]
pub use electrum_client::client::Client;
#[cfg(any(feature = "key-value-db", feature = "default"))]
pub extern crate sled;

#[macro_use]
pub mod error;
pub mod database;
pub mod descriptor;
pub mod psbt;
pub mod signer;
pub mod types;
pub mod wallet;

pub use descriptor::ExtendedDescriptor;
pub use wallet::Wallet;
