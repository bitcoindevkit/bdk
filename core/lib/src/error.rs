#[derive(Debug)]
pub enum Error {
    KeyMismatch(bitcoin::secp256k1::PublicKey, bitcoin::secp256k1::PublicKey),
    MissingInputUTXO(usize),
    Generic(String),

    Encode(bitcoin::consensus::encode::Error),
    BIP32(bitcoin::util::bip32::Error),
    Secp256k1(bitcoin::secp256k1::Error),
    JSON(serde_json::Error),

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

impl_error!(bitcoin::consensus::encode::Error, Encode);
impl_error!(bitcoin::util::bip32::Error, BIP32);
impl_error!(bitcoin::secp256k1::Error, Secp256k1);
impl_error!(serde_json::Error, JSON);

#[cfg(any(feature = "key-value-db", feature = "default"))]
impl_error!(sled::Error, Sled);
