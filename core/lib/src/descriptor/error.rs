#[derive(Debug)]
pub enum Error {
    InternalError,
    InvalidPrefix(Vec<u8>),
    HardenedDerivationOnXpub,
    MissingClosingBracket,
    KeyParsingError(String),

    BIP32(bitcoin::util::bip32::Error),
    Base58(bitcoin::util::base58::Error),
    PK(bitcoin::util::key::Error),
    Miniscript(miniscript::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl_error!(bitcoin::util::bip32::Error, BIP32);
impl_error!(bitcoin::util::base58::Error, Base58);
impl_error!(bitcoin::util::key::Error, PK);
impl_error!(miniscript::Error, Miniscript);
