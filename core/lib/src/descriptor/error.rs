#[derive(Debug)]
pub enum Error {
    InternalError,
    InvalidPrefix(Vec<u8>),
    HardenedDerivationOnXpub,
    MissingClosingBracket,

    BIP32(bitcoin::util::bip32::Error),
    Base58(bitcoin::util::base58::Error),
    PK(bitcoin::util::key::Error),
}

impl From<bitcoin::util::bip32::Error> for Error {
    fn from(err: bitcoin::util::bip32::Error) -> Self {
        Error::BIP32(err)
    }
}

impl From<bitcoin::util::base58::Error> for Error {
    fn from(err: bitcoin::util::base58::Error) -> Self {
        Error::Base58(err)
    }
}

impl From<bitcoin::util::key::Error> for Error {
    fn from(err: bitcoin::util::key::Error) -> Self {
        Error::PK(err)
    }
}
