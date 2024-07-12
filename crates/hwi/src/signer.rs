use bdk_wallet::bitcoin::bip32::Fingerprint;
use bdk_wallet::bitcoin::secp256k1::{All, Secp256k1};
use bdk_wallet::bitcoin::Psbt;
use bdk_wallet::signer::{SignerCommon, SignerError, SignerId, TransactionSigner};
use hwi::error::Error;
use hwi::types::{HWIChain, HWIDevice};
use hwi::HWIClient;

#[derive(Debug)]
/// Custom signer for Hardware Wallets
///
/// This ignores `sign_options` and leaves the decisions up to the hardware wallet.
pub struct HWISigner {
    fingerprint: Fingerprint,
    client: HWIClient,
}

impl HWISigner {
    /// Create a instance from the specified device and chain
    pub fn from_device(device: &HWIDevice, chain: HWIChain) -> Result<HWISigner, Error> {
        let client = HWIClient::get_client(device, false, chain)?;
        Ok(HWISigner {
            fingerprint: device.fingerprint,
            client,
        })
    }
}

impl SignerCommon for HWISigner {
    fn id(&self, _secp: &Secp256k1<All>) -> SignerId {
        SignerId::Fingerprint(self.fingerprint)
    }
}

impl TransactionSigner for HWISigner {
    fn sign_transaction(
        &self,
        psbt: &mut Psbt,
        _sign_options: &bdk_wallet::SignOptions,
        _secp: &Secp256k1<All>,
    ) -> Result<(), SignerError> {
        let signed_psbt = self
            .client
            .sign_tx(psbt)
            .map_err(|e| {
                SignerError::External(format!("While signing with hardware wallet: {}", e))
            })?
            .psbt;

        psbt.combine(signed_psbt).map_err(|e| {
            SignerError::External(format!(
                "Failed to combine HW signed PSBT with passed PSBT: {}",
                e
            ))
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bdk_wallet::bitcoin::Network;
    use bdk_wallet::signer::SignerOrdering;
    // use bdk_wallet::wallet::common::get_funded_wallet;

    use bdk_wallet::wallet::test_util::get_funded_wallet;
    // use bdk_wallet::wallet::AddressIndex;
    use bdk_wallet::KeychainKind;
    use std::sync::Arc;

    #[test]
    fn test_hardware_signer() {
        let mut devices = match HWIClient::enumerate() {
            Ok(devices) => devices,
            Err(e) => panic!("Failed to enumerate devices: {}", e),
        };

        if devices.is_empty() {
            panic!("No devices found!");
        }

        let device = match devices.remove(0) {
            Ok(device) => device,
            Err(e) => panic!("Failed to remove device: {}", e),
        };

        let client = match HWIClient::get_client(&device, true, Network::Regtest.into()) {
            Ok(client) => client,
            Err(e) => panic!("Failed to get client: {}", e),
        };

        let descriptors = match client.get_descriptors::<String>(None) {
            Ok(descriptors) => descriptors,
            Err(e) => panic!("Failed to get descriptors: {}", e),
        };

        let custom_signer = match HWISigner::from_device(&device, Network::Regtest.into()) {
            Ok(signer) => signer,
            Err(e) => panic!("Failed to create HWISigner: {}", e),
        };

        let (mut wallet, _) = get_funded_wallet(&descriptors.internal[0]);

        wallet.add_signer(
            KeychainKind::External,
            SignerOrdering(200),
            Arc::new(custom_signer),
        );

        let addr = wallet
            // ,(AddressIndex::LastUnused)
            .peek_address(KeychainKind::External, 0);

        let mut builder = wallet.build_tx();
        builder.drain_to(addr.script_pubkey()).drain_wallet();
        let mut psbt = builder.finish().expect("Failed to build transaction");

        let finalized = wallet
            .sign(&mut psbt, Default::default())
            .expect("Failed to sign transaction");
        assert!(finalized);
    }
}
