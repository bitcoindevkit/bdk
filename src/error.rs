// Magical Bitcoin Library
// Written in 2020 by
//     Alekos Filini <alekos.filini@gmail.com>
//
// Copyright (c) 2020 Magical Bitcoin
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::fmt;

use crate::{descriptor, wallet, wallet::address_validator};
use bitcoin::OutPoint;

/// Errors that can be thrown by the [`Wallet`](crate::wallet::Wallet)
#[derive(Debug)]
pub enum Error {
    /// Wrong number of bytes found when trying to convert to u32
    InvalidU32Bytes(Vec<u8>),
    /// Generic error
    Generic(String),
    /// This error is thrown when trying to convert Bare and Public key script to address
    ScriptDoesntHaveAddressForm,
    /// Found multiple outputs when `single_recipient` option has been specified
    SingleRecipientMultipleOutputs,
    /// `single_recipient` option is selected but neither `drain_wallet` nor `manually_selected_only` are
    SingleRecipientNoInputs,
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
    UnknownUTXO,
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

    /// Progress value must be between `0.0` (included) and `100.0` (included)
    InvalidProgressValue(f32),
    /// Progress update error (maybe the channel has been closed)
    ProgressUpdateError,
    /// Requested outpoint doesn't exist in the tx (vout greater than available outputs)
    InvalidOutpoint(OutPoint),

    /// Error related to the parsing and usage of descriptors
    Descriptor(crate::descriptor::error::Error),
    /// Error that can be returned to fail the validation of an address
    AddressValidator(crate::wallet::address_validator::AddressValidatorError),
    /// Encoding error
    Encode(bitcoin::consensus::encode::Error),
    /// Miniscript error
    Miniscript(miniscript::Error),
    /// BIP32 error
    BIP32(bitcoin::util::bip32::Error),
    /// An ECDSA error
    Secp256k1(bitcoin::secp256k1::Error),
    /// Error serializing or deserializing JSON data
    JSON(serde_json::Error),
    /// Hex decoding error
    Hex(bitcoin::hashes::hex::Error),
    /// Partially signed bitcoin transaction error
    PSBT(bitcoin::util::psbt::Error),

    //KeyMismatch(bitcoin::secp256k1::PublicKey, bitcoin::secp256k1::PublicKey),
    //MissingInputUTXO(usize),
    //InvalidAddressNetwork(Address),
    //DifferentTransactions,
    //DifferentDescriptorStructure,
    //Uncapable(crate::blockchain::Capability),
    //MissingCachedAddresses,
    #[cfg(feature = "electrum")]
    /// Electrum client error
    Electrum(electrum_client::Error),
    #[cfg(feature = "esplora")]
    /// Esplora client error
    Esplora(crate::blockchain::esplora::EsploraError),
    #[cfg(feature = "compact_filters")]
    /// Compact filters client error)
    CompactFilters(crate::blockchain::compact_filters::CompactFiltersError),
    #[cfg(feature = "key-value-db")]
    /// Sled database error
    Sled(sled::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for Error {}

macro_rules! impl_error {
    ( $from:ty, $to:ident ) => {
        impl_error!($from, $to, Error);
    };
    ( $from:ty, $to:ident, $impl_for:ty ) => {
        impl std::convert::From<$from> for $impl_for {
            fn from(err: $from) -> Self {
                <$impl_for>::$to(err)
            }
        }
    };
}

impl_error!(descriptor::error::Error, Descriptor);
impl_error!(address_validator::AddressValidatorError, AddressValidator);
impl_error!(descriptor::policy::PolicyError, InvalidPolicyPathError);
impl_error!(wallet::signer::SignerError, Signer);

impl From<crate::keys::KeyError> for Error {
    fn from(key_error: crate::keys::KeyError) -> Error {
        match key_error {
            crate::keys::KeyError::Miniscript(inner) => Error::Miniscript(inner),
            crate::keys::KeyError::BIP32(inner) => Error::BIP32(inner),
            crate::keys::KeyError::InvalidChecksum => Error::ChecksumMismatch,
            e => Error::Key(e),
        }
    }
}

impl_error!(bitcoin::consensus::encode::Error, Encode);
impl_error!(miniscript::Error, Miniscript);
impl_error!(bitcoin::util::bip32::Error, BIP32);
impl_error!(bitcoin::secp256k1::Error, Secp256k1);
impl_error!(serde_json::Error, JSON);
impl_error!(bitcoin::hashes::hex::Error, Hex);
impl_error!(bitcoin::util::psbt::Error, PSBT);

#[cfg(feature = "electrum")]
impl_error!(electrum_client::Error, Electrum);
#[cfg(feature = "esplora")]
impl_error!(crate::blockchain::esplora::EsploraError, Esplora);
#[cfg(feature = "key-value-db")]
impl_error!(sled::Error, Sled);

#[cfg(feature = "compact_filters")]
impl From<crate::blockchain::compact_filters::CompactFiltersError> for Error {
    fn from(other: crate::blockchain::compact_filters::CompactFiltersError) -> Self {
        match other {
            crate::blockchain::compact_filters::CompactFiltersError::Global(e) => *e,
            err => Error::CompactFilters(err),
        }
    }
}
