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

#[derive(Debug)]
pub enum Error {
    InternalError,
    InvalidPrefix(Vec<u8>),
    HardenedDerivationOnXpub,
    MalformedInput,
    KeyParsingError(String),

    Policy(crate::descriptor::policy::PolicyError),

    InputIndexDoesntExist,
    MissingPublicKey,
    MissingDetails,

    InvalidDescriptorCharacter(char),

    CantDeriveWithMiniscript,

    BIP32(bitcoin::util::bip32::Error),
    Base58(bitcoin::util::base58::Error),
    PK(bitcoin::util::key::Error),
    Miniscript(miniscript::Error),
    Hex(bitcoin::hashes::hex::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for Error {}

impl_error!(bitcoin::util::bip32::Error, BIP32);
impl_error!(bitcoin::util::base58::Error, Base58);
impl_error!(bitcoin::util::key::Error, PK);
impl_error!(miniscript::Error, Miniscript);
impl_error!(bitcoin::hashes::hex::Error, Hex);
impl_error!(crate::descriptor::policy::PolicyError, Policy);
