use std::sync::Arc;

use magical_bitcoin_wallet::bitcoin;
use magical_bitcoin_wallet::database::MemoryDatabase;
use magical_bitcoin_wallet::descriptor::HDKeyPaths;
use magical_bitcoin_wallet::types::ScriptType;
use magical_bitcoin_wallet::wallet::address_validator::{AddressValidator, AddressValidatorError};
use magical_bitcoin_wallet::{OfflineWallet, Wallet};

use bitcoin::hashes::hex::FromHex;
use bitcoin::util::bip32::Fingerprint;
use bitcoin::{Network, Script};

struct DummyValidator;
impl AddressValidator for DummyValidator {
    fn validate(
        &self,
        script_type: ScriptType,
        hd_keypaths: &HDKeyPaths,
        script: &Script,
    ) -> Result<(), AddressValidatorError> {
        let (_, path) = hd_keypaths
            .values()
            .find(|(fing, _)| fing == &Fingerprint::from_hex("bc123c3e").unwrap())
            .ok_or(AddressValidatorError::InvalidScript)?;

        println!(
            "Validating `{:?}` {} address, script: {}",
            script_type, path, script
        );

        Ok(())
    }
}

fn main() -> Result<(), magical_bitcoin_wallet::error::Error> {
    let descriptor = "sh(and_v(v:pk(tpubDDpWvmUrPZrhSPmUzCMBHffvC3HyMAPnWDSAQNBTnj1iZeJa7BZQEttFiP4DS4GCcXQHezdXhn86Hj6LHX5EDstXPWrMaSneRWM8yUf6NFd/*),after(630000)))";
    let mut wallet: OfflineWallet<_> =
        Wallet::new_offline(descriptor, None, Network::Regtest, MemoryDatabase::new())?;

    wallet.add_address_validator(Arc::new(Box::new(DummyValidator)));

    wallet.get_new_address()?;
    wallet.get_new_address()?;
    wallet.get_new_address()?;

    Ok(())
}
