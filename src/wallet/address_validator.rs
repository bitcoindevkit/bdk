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

use bitcoin::Script;

use crate::descriptor::HDKeyPaths;
use crate::types::ScriptType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressValidatorError {
    UserRejected,
    ConnectionError,
    TimeoutError,
    InvalidScript,
}

impl fmt::Display for AddressValidatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for AddressValidatorError {}

pub trait AddressValidator {
    fn validate(
        &self,
        script_type: ScriptType,
        hd_keypaths: &HDKeyPaths,
        script: &Script,
    ) -> Result<(), AddressValidatorError>;
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use super::*;
    use crate::wallet::test::{get_funded_wallet, get_test_wpkh};
    use crate::wallet::TxBuilder;

    struct TestValidator;
    impl AddressValidator for TestValidator {
        fn validate(
            &self,
            _script_type: ScriptType,
            _hd_keypaths: &HDKeyPaths,
            _script: &bitcoin::Script,
        ) -> Result<(), AddressValidatorError> {
            Err(AddressValidatorError::InvalidScript)
        }
    }

    #[test]
    #[should_panic(expected = "InvalidScript")]
    fn test_address_validator_external() {
        let (mut wallet, _, _) = get_funded_wallet(get_test_wpkh());
        wallet.add_address_validator(Arc::new(Box::new(TestValidator)));

        wallet.get_new_address().unwrap();
    }

    #[test]
    #[should_panic(expected = "InvalidScript")]
    fn test_address_validator_internal() {
        let (mut wallet, descriptors, _) = get_funded_wallet(get_test_wpkh());
        wallet.add_address_validator(Arc::new(Box::new(TestValidator)));

        let addr = testutils!(@external descriptors, 10);
        wallet
            .create_tx(TxBuilder::with_recipients(vec![(addr, 25_000)]))
            .unwrap();
    }
}
