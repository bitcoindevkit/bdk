use crate::miniscript::{Descriptor, DescriptorPublicKey};
use bitcoin::hashes::{hash_newtype, sha256, Hash};
use bitcoin::Amount;

hash_newtype! {
    /// Represents the unique ID of a descriptor.
    ///
    /// This is useful for having a fixed-length unique representation of a descriptor,
    /// in particular, we use it to persist application state changes related to the
    /// descriptor without having to re-write the whole descriptor each time.
    ///
    pub struct DescriptorId(pub sha256::Hash);
}

/// A trait to extend the functionality of a miniscript descriptor.
pub trait DescriptorExt {
    /// Returns the minimum [`Amount`] at which an output is broadcast-able.
    /// Panics if the descriptor wildcard is hardened.
    fn dust_value(&self) -> Amount;

    /// Returns the descriptor ID, calculated as the sha256 hash of the spk derived from the
    /// descriptor at index 0.
    fn descriptor_id(&self) -> DescriptorId;
}

impl DescriptorExt for Descriptor<DescriptorPublicKey> {
    fn dust_value(&self) -> Amount {
        self.at_derivation_index(0)
            .expect("descriptor can't have hardened derivation")
            .script_pubkey()
            .minimal_non_dust()
    }

    fn descriptor_id(&self) -> DescriptorId {
        let spk = self.at_derivation_index(0).unwrap().script_pubkey();
        DescriptorId(sha256::Hash::hash(spk.as_bytes()))
    }
}
