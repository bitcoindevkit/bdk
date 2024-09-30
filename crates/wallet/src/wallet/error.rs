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

//! Errors that can be thrown by the [`Wallet`](crate::wallet::Wallet)

use crate::descriptor::policy::PolicyError;
use crate::descriptor::DescriptorError;
use crate::wallet::coin_selection;
use crate::{descriptor, KeychainKind};
use alloc::string::String;
use bitcoin::{absolute, psbt, Amount, OutPoint, Sequence, Txid};
use core::fmt;

/// Errors returned by miniscript when updating inconsistent PSBTs
#[derive(Debug, Clone)]
pub enum MiniscriptPsbtError {
    /// Descriptor key conversion error
    Conversion(miniscript::descriptor::ConversionError),
    /// Return error type for PsbtExt::update_input_with_descriptor
    UtxoUpdate(miniscript::psbt::UtxoUpdateError),
    /// Return error type for PsbtExt::update_output_with_descriptor
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

#[derive(Debug)]
/// Error returned from [`TxBuilder::finish`]
///
/// [`TxBuilder::finish`]: crate::wallet::tx_builder::TxBuilder::finish
pub enum CreateTxError {
    /// There was a problem with the descriptors passed in
    Descriptor(DescriptorError),
    /// There was a problem while extracting and manipulating policies
    Policy(PolicyError),
    /// Spending policy is not compatible with this [`KeychainKind`]
    SpendingPolicyRequired(KeychainKind),
    /// Requested invalid transaction version '0'
    Version0,
    /// Requested transaction version `1`, but at least `2` is needed to use OP_CSV
    Version1Csv,
    /// Requested `LockTime` is less than is required to spend from this script
    LockTime {
        /// Requested `LockTime`
        requested: absolute::LockTime,
        /// Required `LockTime`
        required: absolute::LockTime,
    },
    /// Cannot enable RBF with `Sequence` given a required OP_CSV
    RbfSequenceCsv {
        /// Given RBF `Sequence`
        sequence: Sequence,
        /// Required OP_CSV `Sequence`
        csv: Sequence,
    },
    /// When bumping a tx the absolute fee requested is lower than replaced tx absolute fee
    FeeTooLow {
        /// Required fee absolute value [`Amount`]
        required: Amount,
    },
    /// When bumping a tx the fee rate requested is lower than required
    FeeRateTooLow {
        /// Required fee rate
        required: bitcoin::FeeRate,
    },
    /// `manually_selected_only` option is selected but no utxo has been passed
    NoUtxosSelected,
    /// Output created is under the dust limit, 546 satoshis
    OutputBelowDustLimit(usize),
    /// There was an error with coin selection
    CoinSelection(coin_selection::InsufficientFunds),
    /// Cannot build a tx without recipients
    NoRecipients,
    /// Partially signed bitcoin transaction error
    Psbt(psbt::Error),
    /// In order to use the [`TxBuilder::add_global_xpubs`] option every extended
    /// key in the descriptor must either be a master key itself (having depth = 0) or have an
    /// explicit origin provided
    ///
    /// [`TxBuilder::add_global_xpubs`]: crate::wallet::tx_builder::TxBuilder::add_global_xpubs
    MissingKeyOrigin(String),
    /// Happens when trying to spend an UTXO that is not in the internal database
    UnknownUtxo,
    /// Missing non_witness_utxo on foreign utxo for given `OutPoint`
    MissingNonWitnessUtxo(OutPoint),
    /// Miniscript PSBT error
    MiniscriptPsbt(MiniscriptPsbtError),
}

impl fmt::Display for CreateTxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Descriptor(e) => e.fmt(f),
            Self::Policy(e) => e.fmt(f),
            CreateTxError::SpendingPolicyRequired(keychain_kind) => {
                write!(f, "Spending policy required: {:?}", keychain_kind)
            }
            CreateTxError::Version0 => {
                write!(f, "Invalid version `0`")
            }
            CreateTxError::Version1Csv => {
                write!(
                    f,
                    "TxBuilder requested version `1`, but at least `2` is needed to use OP_CSV"
                )
            }
            CreateTxError::LockTime {
                requested,
                required,
            } => {
                write!(f, "TxBuilder requested timelock of `{:?}`, but at least `{:?}` is required to spend from this script", required, requested)
            }
            CreateTxError::RbfSequenceCsv { sequence, csv } => {
                write!(
                    f,
                    "Cannot enable RBF with nSequence `{:?}` given a required OP_CSV of `{:?}`",
                    sequence, csv
                )
            }
            CreateTxError::FeeTooLow { required } => {
                write!(f, "Fee to low: required {}", required.display_dynamic())
            }
            CreateTxError::FeeRateTooLow { required } => {
                write!(
                    f,
                    // Note: alternate fmt as sat/vb (ceil) available in bitcoin-0.31
                    //"Fee rate too low: required {required:#}"
                    "Fee rate too low: required {} sat/vb",
                    crate::floating_rate!(required)
                )
            }
            CreateTxError::NoUtxosSelected => {
                write!(f, "No UTXO selected")
            }
            CreateTxError::OutputBelowDustLimit(limit) => {
                write!(f, "Output below the dust limit: {}", limit)
            }
            CreateTxError::CoinSelection(e) => e.fmt(f),
            CreateTxError::NoRecipients => {
                write!(f, "Cannot build tx without recipients")
            }
            CreateTxError::Psbt(e) => e.fmt(f),
            CreateTxError::MissingKeyOrigin(err) => {
                write!(f, "Missing key origin: {}", err)
            }
            CreateTxError::UnknownUtxo => {
                write!(f, "UTXO not found in the internal database")
            }
            CreateTxError::MissingNonWitnessUtxo(outpoint) => {
                write!(f, "Missing non_witness_utxo on foreign utxo {}", outpoint)
            }
            CreateTxError::MiniscriptPsbt(err) => {
                write!(f, "Miniscript PSBT error: {}", err)
            }
        }
    }
}

impl From<descriptor::error::Error> for CreateTxError {
    fn from(err: descriptor::error::Error) -> Self {
        CreateTxError::Descriptor(err)
    }
}

impl From<PolicyError> for CreateTxError {
    fn from(err: PolicyError) -> Self {
        CreateTxError::Policy(err)
    }
}

impl From<MiniscriptPsbtError> for CreateTxError {
    fn from(err: MiniscriptPsbtError) -> Self {
        CreateTxError::MiniscriptPsbt(err)
    }
}

impl From<psbt::Error> for CreateTxError {
    fn from(err: psbt::Error) -> Self {
        CreateTxError::Psbt(err)
    }
}

impl From<coin_selection::InsufficientFunds> for CreateTxError {
    fn from(err: coin_selection::InsufficientFunds) -> Self {
        CreateTxError::CoinSelection(err)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for CreateTxError {}

#[derive(Debug)]
/// Error returned from [`Wallet::build_fee_bump`]
///
/// [`Wallet::build_fee_bump`]: super::Wallet::build_fee_bump
pub enum BuildFeeBumpError {
    /// Happens when trying to spend an UTXO that is not in the internal database
    UnknownUtxo(OutPoint),
    /// Thrown when a tx is not found in the internal database
    TransactionNotFound(Txid),
    /// Happens when trying to bump a transaction that is already confirmed
    TransactionConfirmed(Txid),
    /// Trying to replace a tx that has a sequence >= `0xFFFFFFFE`
    IrreplaceableTransaction(Txid),
    /// Node doesn't have data to estimate a fee rate
    FeeRateUnavailable,
}

impl fmt::Display for BuildFeeBumpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownUtxo(outpoint) => write!(
                f,
                "UTXO not found in the internal database with txid: {}, vout: {}",
                outpoint.txid, outpoint.vout
            ),
            Self::TransactionNotFound(txid) => {
                write!(
                    f,
                    "Transaction not found in the internal database with txid: {}",
                    txid
                )
            }
            Self::TransactionConfirmed(txid) => {
                write!(f, "Transaction already confirmed with txid: {}", txid)
            }
            Self::IrreplaceableTransaction(txid) => {
                write!(f, "Transaction can't be replaced with txid: {}", txid)
            }
            Self::FeeRateUnavailable => write!(f, "Fee rate unavailable"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for BuildFeeBumpError {}
