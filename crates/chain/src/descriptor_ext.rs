use crate::{
    alloc::{string::ToString, vec::Vec},
    miniscript::{Descriptor, DescriptorPublicKey},
};
use bitcoin::hashes::{hash_newtype, sha256, Hash};

hash_newtype! {
    /// Represents the ID of a descriptor, defined as the sha256 hash of
    /// the descriptor string, checksum excluded.
    ///
    /// This is useful for having a fixed-length unique representation of a descriptor,
    /// in particular, we use it to persist application state changes related to the
    /// descriptor without having to re-write the whole descriptor each time.
    ///
    pub struct DescriptorId(pub sha256::Hash);
}

/// A trait to extend the functionality of a miniscript descriptor.
pub trait DescriptorExt {
    /// Returns the minimum value (in satoshis) at which an output is broadcastable.
    /// Panics if the descriptor wildcard is hardened.
    fn dust_value(&self) -> u64;

    /// Returns the descriptor id, calculated as the sha256 of the descriptor, checksum included.
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
        let desc = self.to_string();
        let desc_without_checksum = desc.split('#').next().expect("Must be here");
        let descriptor_bytes = <Vec<u8>>::from(desc_without_checksum.as_bytes());
        DescriptorId(sha256::Hash::hash(&descriptor_bytes))
    }
}
