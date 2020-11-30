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

use bitcoin::{Address, OutPoint};

/// Errors that can be thrown by the [`Wallet`](crate::wallet::Wallet)
#[derive(Debug)]
pub enum Error {
    KeyMismatch(bitcoin::secp256k1::PublicKey, bitcoin::secp256k1::PublicKey),
    MissingInputUTXO(usize),
    InvalidU32Bytes(Vec<u8>),
    Generic(String),
    ScriptDoesntHaveAddressForm,
    SingleRecipientMultipleOutputs,
    SingleRecipientNoInputs,
    NoRecipients,
    NoUtxosSelected,
    OutputBelowDustLimit(usize),
    InsufficientFunds,
    BnBTotalTriesExceeded,
    BnBNoExactMatch,
    InvalidAddressNetwork(Address),
    UnknownUTXO,
    DifferentTransactions,
    TransactionNotFound,
    TransactionConfirmed,
    IrreplaceableTransaction,
    FeeRateTooLow {
        required: crate::types::FeeRate,
    },
    FeeTooLow {
        required: u64,
    },
    /// In order to use the [`TxBuilder::add_global_xpubs`] option every extended
    /// key in the descriptor must either be a master key itself (having depth = 0) or have an
    /// explicit origin provided
    ///
    /// [`TxBuilder::add_global_xpubs`]: crate::wallet::tx_builder::TxBuilder::add_global_xpubs
    MissingKeyOrigin(String),

    Key(crate::keys::KeyError),

    ChecksumMismatch,
    DifferentDescriptorStructure,

    SpendingPolicyRequired(crate::types::ScriptType),
    InvalidPolicyPathError(crate::descriptor::policy::PolicyError),

    Signer(crate::wallet::signer::SignerError),

    // Blockchain interface errors
    Uncapable(crate::blockchain::Capability),
    OfflineClient,
    InvalidProgressValue(f32),
    ProgressUpdateError,
    MissingCachedAddresses,
    InvalidOutpoint(OutPoint),

    Descriptor(crate::descriptor::error::Error),
    AddressValidator(crate::wallet::address_validator::AddressValidatorError),

    Encode(bitcoin::consensus::encode::Error),
    Miniscript(miniscript::Error),
    BIP32(bitcoin::util::bip32::Error),
    Secp256k1(bitcoin::secp256k1::Error),
    JSON(serde_json::Error),
    Hex(bitcoin::hashes::hex::Error),
    PSBT(bitcoin::util::psbt::Error),

    #[cfg(feature = "electrum")]
    Electrum(electrum_client::Error),
    #[cfg(feature = "esplora")]
    Esplora(crate::blockchain::esplora::EsploraError),
    #[cfg(feature = "compact_filters")]
    CompactFilters(crate::blockchain::compact_filters::CompactFiltersError),
    #[cfg(feature = "key-value-db")]
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
        impl std::convert::From<$from> for Error {
            fn from(err: $from) -> Self {
                Error::$to(err)
            }
        }
    };
}

impl_error!(crate::descriptor::error::Error, Descriptor);
impl_error!(
    crate::wallet::address_validator::AddressValidatorError,
    AddressValidator
);
impl_error!(
    crate::descriptor::policy::PolicyError,
    InvalidPolicyPathError
);
impl_error!(crate::wallet::signer::SignerError, Signer);

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
            err @ _ => Error::CompactFilters(err),
        }
    }
}
