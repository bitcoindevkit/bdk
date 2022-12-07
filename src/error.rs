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

use std::fmt;

use crate::bitcoin::Network;
use crate::{descriptor, wallet};
use bitcoin::{OutPoint, Txid};

/// Errors that can be thrown by the [`Wallet`](crate::wallet::Wallet)
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Wrong number of bytes found when trying to convert to u32
    #[error("Wrong number of bytes found when trying to convert to u32")]
    InvalidU32Bytes(Vec<u8>),
    /// Generic error
    #[error("Generic error: {0}")]
    Generic(String),
    /// This error is thrown when trying to convert Bare and Public key script to address
    #[error("Script doesn't have address form")]
    ScriptDoesntHaveAddressForm,
    /// Cannot build a tx without recipients
    #[error("Cannot build tx without recipients")]
    NoRecipients,
    /// `manually_selected_only` option is selected but no utxo has been passed
    #[error("No UTXO selected")]
    NoUtxosSelected,
    /// Output created is under the dust limit, 546 satoshis
    #[error("Output below the dust limit: {0}")]
    OutputBelowDustLimit(usize),
    /// Wallet's UTXO set is not enough to cover recipient's requested plus fee
    #[error("Insufficient funds: {available} sat available of {needed} sat needed")]
    InsufficientFunds {
        /// Sats needed for some transaction
        needed: u64,
        /// Sats available for spending
        available: u64,
    },
    /// Branch and bound coin selection possible attempts with sufficiently big UTXO set could grow
    /// exponentially, thus a limit is set, and when hit, this error is thrown
    #[error("Branch and bound coin selection: total tries exceeded")]
    BnBTotalTriesExceeded,
    /// Branch and bound coin selection tries to avoid needing a change by finding the right inputs for
    /// the desired outputs plus fee, if there is not such combination this error is thrown
    #[error("Branch and bound coin selection: not exact match")]
    BnBNoExactMatch,
    /// Happens when trying to spend an UTXO that is not in the internal database
    #[error("UTXO not found in the internal database")]
    UnknownUtxo,
    /// Thrown when a tx is not found in the internal database
    #[error("Transaction not found in the internal database")]
    TransactionNotFound,
    /// Happens when trying to bump a transaction that is already confirmed
    #[error("Transaction already confirmed")]
    TransactionConfirmed,
    /// Trying to replace a tx that has a sequence >= `0xFFFFFFFE`
    #[error("Transaction can't be replaced")]
    IrreplaceableTransaction,
    /// When bumping a tx the fee rate requested is lower than required
    #[error("Fee rate too low: required {} sat/vbyte", required.as_sat_per_vb())]
    FeeRateTooLow {
        /// Required fee rate (satoshi/vbyte)
        required: crate::types::FeeRate,
    },
    /// When bumping a tx the absolute fee requested is lower than replaced tx absolute fee
    #[error("Fee to low: required {required} sat")]
    FeeTooLow {
        /// Required fee absolute value (satoshi)
        required: u64,
    },
    /// Node doesn't have data to estimate a fee rate
    #[error("Fee rate unavailable")]
    FeeRateUnavailable,
    /// In order to use the [`TxBuilder::add_global_xpubs`] option every extended
    /// key in the descriptor must either be a master key itself (having depth = 0) or have an
    /// explicit origin provided
    ///
    /// [`TxBuilder::add_global_xpubs`]: crate::wallet::tx_builder::TxBuilder::add_global_xpubs
    #[error("Missing key origin: {0}")]
    MissingKeyOrigin(String),
    /// Error while working with [`keys`](crate::keys)
    #[error("Key error: {0}")]
    Key(crate::keys::KeyError),
    /// Descriptor checksum mismatch
    #[error("Descriptor checksum mismatch")]
    ChecksumMismatch,
    /// Spending policy is not compatible with this [`KeychainKind`](crate::types::KeychainKind)
    #[error("Spending policy required: {0:?}")]
    SpendingPolicyRequired(crate::types::KeychainKind),
    /// Error while extracting and manipulating policies
    #[error("Invalid policy path: {0}")]
    InvalidPolicyPathError(#[from] crate::descriptor::policy::PolicyError),
    /// Signing error
    #[error("Signer error: {0}")]
    Signer(#[from] crate::wallet::signer::SignerError),
    /// Invalid network
    #[error("Invalid network: requested {} but found {}", requested, found)]
    InvalidNetwork {
        /// requested network, for example what is given as bdk-cli option
        requested: Network,
        /// found network, for example the network of the bitcoin node
        found: Network,
    },
    #[cfg(feature = "verify")]
    /// Transaction verification error
    #[error("Verification error: {0}")]
    Verification(crate::wallet::verify::VerifyError),

    /// Progress value must be between `0.0` (included) and `100.0` (included)
    #[error("Invalid progress value: {0}")]
    InvalidProgressValue(f32),
    /// Progress update error (maybe the channel has been closed)
    #[error("Progress update error (maybe the channel has been closed)")]
    ProgressUpdateError,
    /// Requested outpoint doesn't exist in the tx (vout greater than available outputs)
    #[error("Requested outpoint doesn't exist in the tx: {0}")]
    InvalidOutpoint(OutPoint),

    /// Error related to the parsing and usage of descriptors
    #[error("Descriptor error: {0}")]
    Descriptor(#[from] crate::descriptor::error::Error),
    /// Encoding error
    #[error("Encoding error: {0}")]
    Encode(#[from] bitcoin::consensus::encode::Error),
    /// Miniscript error
    #[error("Miniscript error: {0}")]
    Miniscript(#[from] miniscript::Error),
    /// Miniscript PSBT error
    #[error("Miniscript PSBT error: {0}")]
    MiniscriptPsbt(#[from] MiniscriptPsbtError),
    /// BIP32 error
    #[error("BIP32 error: {0}")]
    Bip32(#[from] bitcoin::util::bip32::Error),
    /// An ECDSA error
    #[error("ECDSA error: {0}")]
    Secp256k1(#[from] bitcoin::secp256k1::Error),
    /// Error serializing or deserializing JSON data
    #[error("Serialize/Deserialize JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// Hex decoding error
    #[error("Hex decoding error: {0}")]
    Hex(#[from] bitcoin::hashes::hex::Error),
    /// Partially signed bitcoin transaction error
    #[error("PSBT error: {0}")]
    Psbt(#[from] bitcoin::util::psbt::Error),
    /// Partially signed bitcoin transaction parse error
    #[error("Impossible to parse PSBT: {0}")]
    PsbtParse(#[from] bitcoin::util::psbt::PsbtParseError),

    //KeyMismatch(bitcoin::secp256k1::PublicKey, bitcoin::secp256k1::PublicKey),
    //MissingInputUTXO(usize),
    //InvalidAddressNetwork(Address),
    //DifferentTransactions,
    //DifferentDescriptorStructure,
    //Uncapable(crate::blockchain::Capability),
    //MissingCachedAddresses,
    /// [`crate::blockchain::WalletSync`] sync attempt failed due to missing scripts in cache which
    /// are needed to satisfy `stop_gap`.
    #[error("Missing cached scripts: {0:?}")]
    MissingCachedScripts(MissingCachedScripts),

    #[cfg(feature = "electrum")]
    /// Electrum client error
    #[error("Electrum client error: {0}")]
    Electrum(#[from] electrum_client::Error),
    #[cfg(feature = "esplora")]
    /// Esplora client error
    #[error("Esplora client error: {0}")]
    Esplora(Box<crate::blockchain::esplora::EsploraError>),
    #[cfg(feature = "compact_filters")]
    /// Compact filters client error)
    #[error("Compact filters client error: {0}")]
    CompactFilters(crate::blockchain::compact_filters::CompactFiltersError),
    #[cfg(feature = "key-value-db")]
    /// Sled database error
    #[error("Sled database error: {0}")]
    Sled(#[from] sled::Error),
    #[cfg(feature = "rpc")]
    /// Rpc client error
    #[error("RPC client error: {0}")]
    Rpc(#[from] bitcoincore_rpc::Error),
    #[cfg(feature = "sqlite")]
    /// Rusqlite client error
    #[error("SQLite error: {0}")]
    Rusqlite(#[from] rusqlite::Error),
}

/// Errors returned by miniscript when updating inconsistent PSBTs
#[derive(Debug, Clone, thiserror::Error)]
pub enum MiniscriptPsbtError {
    #[error("Conversion error: {0}")]
    Conversion(#[from] miniscript::descriptor::ConversionError),
    #[error("UTXO update error: {0}")]
    UtxoUpdate(#[from] miniscript::psbt::UtxoUpdateError),
    #[error("Output update error: {0}")]
    OutputUpdate(#[from] miniscript::psbt::OutputUpdateError),
}

/// Represents the last failed [`crate::blockchain::WalletSync`] sync attempt in which we were short
/// on cached `scriptPubKey`s.
#[derive(Debug)]
pub struct MissingCachedScripts {
    /// Number of scripts in which txs were requested during last request.
    pub last_count: usize,
    /// Minimum number of scripts to cache more of in order to satisfy `stop_gap`.
    pub missing_count: usize,
}

impl From<crate::keys::KeyError> for Error {
    fn from(key_error: crate::keys::KeyError) -> Error {
        match key_error {
            crate::keys::KeyError::Miniscript(inner) => Error::Miniscript(inner),
            crate::keys::KeyError::Bip32(inner) => Error::Bip32(inner),
            crate::keys::KeyError::InvalidChecksum => Error::ChecksumMismatch,
            e => Self::Key(e),
        }
    }
}

#[cfg(feature = "compact_filters")]
impl From<crate::blockchain::compact_filters::CompactFiltersError> for Error {
    fn from(other: crate::blockchain::compact_filters::CompactFiltersError) -> Self {
        match other {
            crate::blockchain::compact_filters::CompactFiltersError::Global(e) => *e,
            err => Self::CompactFilters(err),
        }
    }
}

#[cfg(feature = "verify")]
impl From<crate::wallet::verify::VerifyError> for Error {
    fn from(other: crate::wallet::verify::VerifyError) -> Self {
        match other {
            crate::wallet::verify::VerifyError::Global(inner) => *inner,
            err => Self::Verification(err),
        }
    }
}

#[cfg(feature = "esplora")]
impl From<crate::blockchain::esplora::EsploraError> for Error {
    fn from(other: crate::blockchain::esplora::EsploraError) -> Self {
        Self::Esplora(Box::new(other))
    }
}
