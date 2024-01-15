use crate::{
    alloc::{string::ToString, vec::Vec},
    keychain::DescriptorId,
    miniscript::{Descriptor, DescriptorPublicKey},
};
use bitcoin::hashes::{sha256, Hash};

/// A trait to extend the functionality of a miniscript descriptor.
pub trait DescriptorExt {
    /// Returns the minimum value (in satoshis) at which an output is broadcastable.
    /// Panics if the descriptor wildcard is hardened.
    fn dust_value(&self) -> u64;

    /// Returns the descriptor id, calculated as the sha256 of the descriptor.
    /// TODO: includiamo checksum?
    fn descriptor_id(&self) -> DescriptorId;
}

impl DescriptorExt for Descriptor<DescriptorPublicKey> {
    fn dust_value(&self) -> u64 {
        self.at_derivation_index(0)
            .expect("descriptor can't have hardened derivation")
            .script_pubkey()
            .dust_value()
            .to_sat()
    }

    fn descriptor_id(&self) -> DescriptorId {
        let descriptor_bytes = <Vec<u8>>::from(self.to_string().as_bytes());
        DescriptorId(sha256::Hash::hash(&descriptor_bytes))
    }
}
