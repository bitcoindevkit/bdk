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

use crate::bitcoin::Network;
use crate::{descriptor, wallet};
use alloc::{string::String, vec::Vec};
use bitcoin::{OutPoint, Txid};
use core::fmt;

/// Errors that can be thrown by the [`Wallet`](crate::wallet::Wallet)
#[derive(Debug)]
pub enum Error {
    /// Generic error
    Generic(String),
    /// Cannot build a tx without recipients
    NoRecipients,
    /// `manually_selected_only` option is selected but no utxo has been passed
    NoUtxosSelected,
    /// Output created is under the dust limit, 546 satoshis
    OutputBelowDustLimit(usize),
    /// Wallet's UTXO set is not enough to cover recipient's requested plus fee
    InsufficientFunds {
        /// Sats needed for some transaction
        needed: u64,
        /// Sats available for spending
        available: u64,
    },
    /// Branch and bound coin selection possible attempts with sufficiently big UTXO set could grow
    /// exponentially, thus a limit is set, and when hit, this error is thrown
    BnBTotalTriesExceeded,
    /// Branch and bound coin selection tries to avoid needing a change by finding the right inputs for
    /// the desired outputs plus fee, if there is not such combination this error is thrown
    BnBNoExactMatch,
    /// Happens when trying to spend an UTXO that is not in the internal database
    UnknownUtxo,
    /// Thrown when a tx is not found in the internal database
    TransactionNotFound,
    /// Happens when trying to bump a transaction that is already confirmed
    TransactionConfirmed,
    /// Trying to replace a tx that has a sequence >= `0xFFFFFFFE`
    IrreplaceableTransaction,
    /// When bumping a tx the fee rate requested is lower than required
    FeeRateTooLow {
        /// Required fee rate (satoshi/vbyte)
        required: crate::types::FeeRate,
    },
    /// When bumping a tx the absolute fee requested is lower than replaced tx absolute fee
    FeeTooLow {
        /// Required fee absolute value (satoshi)
        required: u64,
    },
    /// Node doesn't have data to estimate a fee rate
    FeeRateUnavailable,
    /// In order to use the [`TxBuilder::add_global_xpubs`] option every extended
    /// key in the descriptor must either be a master key itself (having depth = 0) or have an
    /// explicit origin provided
    ///
    /// [`TxBuilder::add_global_xpubs`]: crate::wallet::tx_builder::TxBuilder::add_global_xpubs
    MissingKeyOrigin(String),
    /// Error while working with [`keys`](crate::keys)
    Key(crate::keys::KeyError),
    /// Descriptor checksum mismatch
    ChecksumMismatch,
    /// Spending policy is not compatible with this [`KeychainKind`](crate::types::KeychainKind)
    SpendingPolicyRequired(crate::types::KeychainKind),
    /// Error while extracting and manipulating policies
    InvalidPolicyPathError(crate::descriptor::policy::PolicyError),
    /// Signing error
    Signer(crate::wallet::signer::SignerError),
    /// Requested outpoint doesn't exist in the tx (vout greater than available outputs)
    InvalidOutpoint(OutPoint),
    /// Error related to the parsing and usage of descriptors
    Descriptor(crate::descriptor::error::Error),
    /// Miniscript error
    Miniscript(miniscript::Error),
    /// Miniscript PSBT error
    MiniscriptPsbt(MiniscriptPsbtError),
    /// BIP32 error
    Bip32(bitcoin::util::bip32::Error),
    /// Partially signed bitcoin transaction error
    Psbt(bitcoin::util::psbt::Error),
}

/// Errors returned by miniscript when updating inconsistent PSBTs
#[derive(Debug, Clone)]
pub enum MiniscriptPsbtError {
    Conversion(miniscript::descriptor::ConversionError),
    UtxoUpdate(miniscript::psbt::UtxoUpdateError),
    OutputUpdate(miniscript::psbt::OutputUpdateError),
}

impl fmt::Display for MiniscriptPsbtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conversion(err) => write!(f, "Conversion error: {}", err),
            Self::UtxoUpdate(err) => write!(f, "UTXO update error: {}", err),
            Self::OutputUpdate(err) => write!(f, "Output update error: {}", err),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for MiniscriptPsbtError {}

#[cfg(feature = "std")]
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Generic(err) => write!(f, "Generic error: {}", err),
            Self::NoRecipients => write!(f, "Cannot build tx without recipients"),
            Self::NoUtxosSelected => write!(f, "No UTXO selected"),
            Self::OutputBelowDustLimit(limit) => {
                write!(f, "Output below the dust limit: {}", limit)
            }
            Self::InsufficientFunds { needed, available } => write!(
                f,
                "Insufficient funds: {} sat available of {} sat needed",
                available, needed
            ),
            Self::BnBTotalTriesExceeded => {
                write!(f, "Branch and bound coin selection: total tries exceeded")
            }
            Self::BnBNoExactMatch => write!(f, "Branch and bound coin selection: not exact match"),
            Self::UnknownUtxo => write!(f, "UTXO not found in the internal database"),
            Self::TransactionNotFound => {
                write!(f, "Transaction not found in the internal database")
            }
            Self::TransactionConfirmed => write!(f, "Transaction already confirmed"),
            Self::IrreplaceableTransaction => write!(f, "Transaction can't be replaced"),
            Self::FeeRateTooLow { required } => write!(
                f,
                "Fee rate too low: required {} sat/vbyte",
                required.as_sat_per_vb()
            ),
            Self::FeeTooLow { required } => write!(f, "Fee to low: required {} sat", required),
            Self::FeeRateUnavailable => write!(f, "Fee rate unavailable"),
            Self::MissingKeyOrigin(err) => write!(f, "Missing key origin: {}", err),
            Self::Key(err) => write!(f, "Key error: {}", err),
            Self::ChecksumMismatch => write!(f, "Descriptor checksum mismatch"),
            Self::SpendingPolicyRequired(keychain_kind) => {
                write!(f, "Spending policy required: {:?}", keychain_kind)
            }
            Self::InvalidPolicyPathError(err) => write!(f, "Invalid policy path: {}", err),
            Self::Signer(err) => write!(f, "Signer error: {}", err),
            Self::InvalidOutpoint(outpoint) => write!(
                f,
                "Requested outpoint doesn't exist in the tx: {}",
                outpoint
            ),
            Self::Descriptor(err) => write!(f, "Descriptor error: {}", err),
            Self::Miniscript(err) => write!(f, "Miniscript error: {}", err),
            Self::MiniscriptPsbt(err) => write!(f, "Miniscript PSBT error: {}", err),
            Self::Bip32(err) => write!(f, "BIP32 error: {}", err),
            Self::Psbt(err) => write!(f, "PSBT error: {}", err),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

macro_rules! impl_error {
    ( $from:ty, $to:ident ) => {
        impl_error!($from, $to, Error);
    };
    ( $from:ty, $to:ident, $impl_for:ty ) => {
        impl core::convert::From<$from> for $impl_for {
            fn from(err: $from) -> Self {
                <$impl_for>::$to(err)
            }
        }
    };
}

impl_error!(descriptor::error::Error, Descriptor);
impl_error!(descriptor::policy::PolicyError, InvalidPolicyPathError);
impl_error!(wallet::signer::SignerError, Signer);

impl From<crate::keys::KeyError> for Error {
    fn from(key_error: crate::keys::KeyError) -> Error {
        match key_error {
            crate::keys::KeyError::Miniscript(inner) => Error::Miniscript(inner),
            crate::keys::KeyError::Bip32(inner) => Error::Bip32(inner),
            crate::keys::KeyError::InvalidChecksum => Error::ChecksumMismatch,
            e => Error::Key(e),
        }
    }
}

impl_error!(miniscript::Error, Miniscript);
impl_error!(MiniscriptPsbtError, MiniscriptPsbt);
impl_error!(bitcoin::util::bip32::Error, Bip32);
impl_error!(bitcoin::util::psbt::Error, Psbt);
