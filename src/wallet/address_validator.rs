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
            .create_tx(TxBuilder::from_addressees(vec![(addr, 25_000)]))
            .unwrap();
    }
}
