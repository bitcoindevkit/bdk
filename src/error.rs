use bitcoin::{OutPoint, Script, Txid};

#[derive(Debug)]
pub enum Error {
    KeyMismatch(bitcoin::secp256k1::PublicKey, bitcoin::secp256k1::PublicKey),
    MissingInputUTXO(usize),
    InvalidU32Bytes(Vec<u8>),
    Generic(String),
    ScriptDoesntHaveAddressForm,
    SendAllMultipleOutputs,
    OutputBelowDustLimit(usize),
    InsufficientFunds,
    UnknownUTXO,
    DifferentTransactions,

    ChecksumMismatch,

    SpendingPolicyRequired,
    InvalidPolicyPathError(crate::descriptor::policy::PolicyError),

    // Signing errors (expected, received)
    InputTxidMismatch((Txid, OutPoint)),
    InputRedeemScriptMismatch((Script, Script)), // scriptPubKey, redeemScript
    InputWitnessScriptMismatch((Script, Script)), // scriptPubKey, redeemScript
    InputUnknownSegwitScript(Script),
    InputMissingWitnessScript(usize),
    MissingUTXO,

    Descriptor(crate::descriptor::error::Error),

    Encode(bitcoin::consensus::encode::Error),
    BIP32(bitcoin::util::bip32::Error),
    Secp256k1(bitcoin::secp256k1::Error),
    JSON(serde_json::Error),
    Hex(bitcoin::hashes::hex::Error),
    PSBT(bitcoin::util::psbt::Error),

    #[cfg(any(feature = "electrum", feature = "default"))]
    Electrum(electrum_client::Error),
    #[cfg(any(feature = "key-value-db", feature = "default"))]
    Sled(sled::Error),
}

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
    crate::descriptor::policy::PolicyError,
    InvalidPolicyPathError
);

impl_error!(bitcoin::consensus::encode::Error, Encode);
impl_error!(bitcoin::util::bip32::Error, BIP32);
impl_error!(bitcoin::secp256k1::Error, Secp256k1);
impl_error!(serde_json::Error, JSON);
impl_error!(bitcoin::hashes::hex::Error, Hex);
impl_error!(bitcoin::util::psbt::Error, PSBT);

#[cfg(any(feature = "electrum", feature = "default"))]
impl_error!(electrum_client::Error, Electrum);
#[cfg(any(feature = "key-value-db", feature = "default"))]
impl_error!(sled::Error, Sled);
